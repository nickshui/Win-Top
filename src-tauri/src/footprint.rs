//! 产物解析器：给定一个已装应用，推导它在磁盘/注册表/服务/启动项上的产物（footprint），
//! 每项带分类与置信度。这是"智能卸载"的核心，也是未来"安装监控"要插入的同一接缝。
//!
//! 精度分层（对应置信度）：
//!   High   —— 安装目录（InstallLocation / DisplayIcon / UninstallString 推导）及其下内容；
//!             ImagePath 落在安装目录内的服务；命令落在安装目录内的启动项。
//!   Medium —— 在标准根目录（ProgramData/AppData/…）下、名字强匹配产品/发行商的残留目录或注册表键。
//!   Low    —— 名字弱匹配（仅含某个较长 token）的残留候选。
//!
//! 安全：本模块只读、只“发现”，不删除。删除由 uninstall.rs 在用户确认后执行。
//! v1 不做全盘 MFT 深扫（改用“已知根目录的直接子项精确探测”，更快、无需管理员、误报更低）；
//! 计划任务 / 防火墙关联留待后续。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::Serialize;

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ,
};

use crate::app_inventory::{self, AppEntry};

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    Dir,
    RegKey,
    Service,
    Startup,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    UserData,
    AppRuntime,
    Cache,
    Config,
    Binary,
    Unknown,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Serialize, Clone)]
pub struct Artifact {
    pub path: String, // 文件/目录路径、注册表键路径，或服务名/启动项名
    pub kind: ArtifactKind,
    pub category: Category,
    pub size: u64,
    pub confidence: Confidence,
    pub source: String,     // install-dir | residue-fs | residue-reg | service | startup
    pub reversible: bool,   // 能否回收站/.reg 备份还原
    pub match_reason: String, // 为什么认为它属于该应用（给用户看，尤其低置信）
}

#[derive(Serialize, Clone)]
pub struct Footprint {
    pub app_id: String,
    pub app_name: String,
    pub known_roots: Vec<String>,
    pub artifacts: Vec<Artifact>,
    pub total_size: u64,
    pub high_conf_size: u64,
    pub residue_size: u64,
    pub warnings: Vec<String>,
    pub app_removed: bool, // 应用已从注册表消失（多半被厂商卸载），当前为残留复扫结果
}

// ─── 纯函数：路径 / 身份 / 匹配 / 分类 ────────────────────────────────────

/// 去掉首尾成对的双引号。
fn strip_quotes(s: &str) -> &str {
    let t = s.trim();
    t.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(t)
}

/// 从命令行里取出第一个 .exe 路径（支持带引号 / 不带引号）。
fn first_exe_path(cmd: &str) -> Option<PathBuf> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return None;
    }
    // 带引号：取引号内内容
    if let Some(rest) = cmd.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            let p = &rest[..end];
            return Some(PathBuf::from(p));
        }
    }
    // 不带引号：找 ".exe" 结束位置（大小写不敏感）
    let lower = cmd.to_lowercase();
    if let Some(pos) = lower.find(".exe") {
        let p = &cmd[..pos + 4];
        return Some(PathBuf::from(p));
    }
    // 兜底：第一个空格前
    let p = cmd.split_whitespace().next().unwrap_or(cmd);
    Some(PathBuf::from(p))
}

/// 从 DisplayIcon（形如 "C:\App\app.exe,0"）取安装目录。
pub(crate) fn dir_from_icon(icon: &str) -> Option<PathBuf> {
    let s = strip_quotes(icon);
    // 去掉尾部 ",<index>"
    let s = match s.rsplit_once(',') {
        Some((left, right)) if right.trim().parse::<i32>().is_ok() => left,
        _ => s,
    };
    let p = PathBuf::from(s);
    p.parent().map(|x| x.to_path_buf())
}

/// 发行商停用词 + 过于通用的产品词：不作为匹配 token。
const GENERIC: &[&str] = &[
    "inc", "ltd", "llc", "corp", "corporation", "company", "co", "team", "software",
    "technologies", "technology", "limited", "gmbh", "srl", "systems", "solutions",
    "group", "the", "app", "apps", "application", "data", "client", "desktop", "update",
    "updater", "helper", "service", "services", "common", "shared", "temp", "cache",
    "logs", "bin", "core", "tools", "tool", "setup", "install", "installer", "program",
    "programs", "microsoft", "windows", "user", "users", "public",
];

/// 只保留字母数字并小写（"CC Switch" -> "ccswitch"）。
fn compact(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// 把名字拆成有区分度的 token（小写、长度≥4、非通用词、非纯数字）。
fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_lowercase())
        .filter(|t| t.len() >= 4 && !t.chars().all(|c| c.is_ascii_digit()) && !GENERIC.contains(&t.as_str()))
        .collect()
}

/// 应用身份：产品紧凑名、产品 token、发行商紧凑名、发行商 token。
struct Identity {
    prod_compact: String,
    prod_tokens: Vec<String>,
    pub_compact: String,
    pub_tokens: Vec<String>,
}

fn identity(name: &str, publisher: &str) -> Identity {
    Identity {
        prod_compact: compact(name),
        prod_tokens: tokens(name),
        pub_compact: compact(publisher),
        pub_tokens: tokens(publisher),
    }
}

/// 候选名字与应用身份的匹配强度。None=不匹配。
fn name_match(candidate: &str, id: &Identity) -> Option<(Confidence, String)> {
    let cnorm = compact(candidate);
    if cnorm.is_empty() {
        return None;
    }
    // 与产品紧凑名相等 / 相关 —— 强匹配
    if !id.prod_compact.is_empty() && id.prod_compact.len() >= 4 {
        if cnorm == id.prod_compact {
            return Some((Confidence::Medium, format!("目录名与产品名「{}」一致", candidate)));
        }
        // 候选名完整包含产品名作为前缀（如 "ccswitchsetup"）—— 仍算中等
        if cnorm.starts_with(&id.prod_compact) {
            return Some((Confidence::Medium, "目录名以产品名开头".to_string()));
        }
        // 候选名是产品名的前缀：需足够长(≥4)才算中等，避免 "360" 这类短前缀误匹配
        if id.prod_compact.starts_with(&cnorm) && cnorm.len() >= 4 {
            return Some((Confidence::Medium, format!("目录名「{}」是产品名前缀", candidate)));
        }
    }
    // 命中某个产品 token —— 中/弱
    let cand_toks: HashSet<String> = candidate
        .split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_lowercase())
        .collect();
    for t in &id.prod_tokens {
        if cand_toks.contains(t) {
            return Some((Confidence::Medium, format!("目录名含产品关键词「{}」", t)));
        }
        if t.len() >= 6 && cnorm.contains(t.as_str()) {
            return Some((Confidence::Low, format!("目录名疑似含产品关键词「{}」", t)));
        }
    }
    // 与发行商相关 —— 弱（发行商名易撞车，仅作低置信提示）
    if !id.pub_compact.is_empty() && id.pub_compact.len() >= 4 && cnorm == id.pub_compact {
        return Some((Confidence::Low, format!("目录名与发行商「{}」一致", candidate)));
    }
    for t in &id.pub_tokens {
        if t.len() >= 5 && cand_toks.contains(t) {
            return Some((Confidence::Low, format!("目录名含发行商关键词「{}」", t)));
        }
    }
    None
}

/// 按路径关键词粗分类。保守：拿不准归 Unknown/AppRuntime，绝不轻易标 UserData 之外可删。
fn classify(path: &str) -> Category {
    let p = path.to_lowercase();
    let has = |kw: &str| p.contains(kw);
    if has("\\cache") || has("\\code cache") || has("\\gpucache") || has("\\crashpad")
        || has("\\crashes") || has("\\crash reports") || has("\\service worker")
        || has("\\blob_storage") || has("\\grshadercache") || has("\\dawncache")
        || has("\\logs") || has("\\log\\") || p.ends_with("\\log") || has("\\temp") || has("\\tmp")
    {
        return Category::Cache;
    }
    if has("\\documents") || has("\\projects") || has("\\workspaces") || has("\\saved")
        || has("\\saves") || has("\\backups") || has("\\my ")
    {
        return Category::UserData;
    }
    if has("\\settings") || has("\\config") || has("\\preferences") || p.ends_with(".ini")
        || p.ends_with(".cfg") || p.ends_with(".conf")
    {
        return Category::Config;
    }
    if p.ends_with(".exe") || p.ends_with(".dll") || has("program files") {
        return Category::Binary;
    }
    if has("appdata") || has("programdata") {
        return Category::AppRuntime;
    }
    Category::Unknown
}

/// b 是否在 a 目录之内（含相等），大小写不敏感、按规范化前缀判断。
fn is_under(root: &str, child: &str) -> bool {
    let r = root.to_lowercase().replace('/', "\\");
    let r = r.trim_end_matches('\\');
    let c = child.to_lowercase().replace('/', "\\");
    if r.is_empty() {
        return false;
    }
    c == r || c.starts_with(&format!("{}\\", r))
}

// ─── 目录体积 ─────────────────────────────────────────────────────────

/// 递归统计目录体积（跳过符号链接/junction，容错）。
fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let rd = match fs::read_dir(path) {
        Ok(r) => r,
        Err(_) => return 0,
    };
    for e in rd.flatten() {
        let p = e.path();
        let md = match fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let ft = md.file_type();
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            total += dir_size(&p);
        } else {
            total += md.len();
        }
    }
    total
}

// ─── 已知安装根推导 ───────────────────────────────────────────────────

fn known_roots(app: &AppEntry) -> (Vec<PathBuf>, Vec<String>) {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let push = |p: PathBuf, roots: &mut Vec<PathBuf>| {
        if p.as_os_str().is_empty() {
            return;
        }
        if p.is_dir() && !roots.iter().any(|r| r == &p) {
            roots.push(p);
        }
    };

    let loc = strip_quotes(&app.install_location);
    if !loc.is_empty() {
        push(PathBuf::from(loc), &mut roots);
    }
    if roots.is_empty() && !app.display_icon.is_empty() {
        if let Some(d) = dir_from_icon(&app.display_icon) {
            push(d, &mut roots);
            if !roots.is_empty() {
                warnings.push("InstallLocation 为空，安装目录由 DisplayIcon 推导".into());
            }
        }
    }
    if roots.is_empty() {
        let candidate = if !app.quiet_uninstall_string.is_empty() {
            &app.quiet_uninstall_string
        } else {
            &app.uninstall_string
        };
        if !candidate.to_lowercase().contains("msiexec") {
            if let Some(exe) = first_exe_path(candidate) {
                if let Some(dir) = exe.parent() {
                    push(dir.to_path_buf(), &mut roots);
                    if !roots.is_empty() {
                        warnings.push("InstallLocation 为空，安装目录由卸载命令推导（置信略低）".into());
                    }
                }
            }
        }
    }
    if roots.is_empty() {
        warnings.push("未能定位安装目录，仅能列出名字匹配的残留候选，请谨慎核对".into());
    }
    (roots, warnings)
}

// ─── 残留扫描：文件系统 ───────────────────────────────────────────────

fn residue_fs_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    let mut add = |key: &str, tail: &str| {
        if let Some(base) = std::env::var_os(key) {
            let p = if tail.is_empty() {
                PathBuf::from(&base)
            } else {
                PathBuf::from(&base).join(tail)
            };
            v.push(p);
        }
    };
    add("ProgramFiles", "");
    add("ProgramFiles(x86)", "");
    add("ProgramData", "");
    add("APPDATA", "");
    add("LOCALAPPDATA", "");
    add("LOCALAPPDATA", "Programs");
    add("PUBLIC", "");
    v
}

fn scan_fs_residue(id: &Identity, known: &[PathBuf], out: &mut Vec<Artifact>) {
    let known_lower: Vec<String> = known.iter().map(|p| p.to_string_lossy().to_lowercase()).collect();
    for root in residue_fs_roots() {
        let rd = match fs::read_dir(&root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let md = match e.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !md.is_dir() {
                continue;
            }
            let path = e.path();
            let path_str = path.to_string_lossy().to_string();
            // 已在安装目录内的，不重复计为残留
            if known_lower.iter().any(|k| is_under(k, &path_str.to_lowercase())) {
                continue;
            }
            let dname = e.file_name().to_string_lossy().to_string();
            if let Some((conf, reason)) = name_match(&dname, id) {
                out.push(Artifact {
                    size: dir_size(&path),
                    category: classify(&path_str),
                    path: path_str,
                    kind: ArtifactKind::Dir,
                    confidence: conf,
                    source: "residue-fs".into(),
                    reversible: true,
                    match_reason: reason,
                });
            }
        }
    }
}

// ─── 残留扫描：注册表 ─────────────────────────────────────────────────

fn scan_reg_residue(id: &Identity, out: &mut Vec<Artifact>) {
    let roots: [(HKEY, &str, &str); 3] = [
        (HKEY_CURRENT_USER, "Software", "HKCU\\Software"),
        (HKEY_LOCAL_MACHINE, "Software", "HKLM\\Software"),
        (HKEY_LOCAL_MACHINE, "Software\\WOW6432Node", "HKLM\\Software\\WOW6432Node"),
    ];
    for (hive, sub, disp) in roots {
        unsafe {
            let subw = app_inventory::to_wide(sub);
            let mut hkey = HKEY::default();
            if RegOpenKeyExW(hive, PCWSTR(subw.as_ptr()), 0, KEY_READ, &mut hkey) != ERROR_SUCCESS {
                continue;
            }
            for child in app_inventory::enum_subkey_names(hkey) {
                if let Some((conf, reason)) = name_match(&child, id) {
                    // 注册表残留只在"强匹配"时上报，降低误报（键名撞车比目录更常见）
                    if conf == Confidence::Low {
                        continue;
                    }
                    out.push(Artifact {
                        path: format!("{}\\{}", disp, child),
                        kind: ArtifactKind::RegKey,
                        category: Category::Config,
                        size: 0,
                        confidence: conf,
                        source: "residue-reg".into(),
                        reversible: true,
                        match_reason: reason,
                    });
                }
            }
            let _ = RegCloseKey(hkey);
        }
    }
}

// ─── 关联：服务 / 启动项 ──────────────────────────────────────────────

fn scan_related_services(known: &[PathBuf], out: &mut Vec<Artifact>) {
    if known.is_empty() {
        return;
    }
    let known_lower: Vec<String> = known.iter().map(|p| p.to_string_lossy().to_lowercase()).collect();
    unsafe {
        let subw = app_inventory::to_wide("SYSTEM\\CurrentControlSet\\Services");
        let mut root = HKEY::default();
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(subw.as_ptr()), 0, KEY_READ, &mut root)
            != ERROR_SUCCESS
        {
            return;
        }
        for name in app_inventory::enum_subkey_names(root) {
            let sw = app_inventory::to_wide(&name);
            let mut svc = HKEY::default();
            if RegOpenKeyExW(root, PCWSTR(sw.as_ptr()), 0, KEY_READ, &mut svc) != ERROR_SUCCESS {
                continue;
            }
            let image = app_inventory::expand_env(&app_inventory::read_string(svc, "ImagePath"));
            let _ = RegCloseKey(svc);
            if let Some(exe) = first_exe_path(&image) {
                let exe_str = exe.to_string_lossy().to_lowercase();
                if known_lower.iter().any(|k| is_under(k, &exe_str)) {
                    out.push(Artifact {
                        path: name.clone(),
                        kind: ArtifactKind::Service,
                        category: Category::AppRuntime,
                        size: 0,
                        confidence: Confidence::High,
                        source: "service".into(),
                        reversible: true,
                        match_reason: format!("服务映像路径位于安装目录内：{}", image),
                    });
                }
            }
        }
        let _ = RegCloseKey(root);
    }
}

fn scan_related_startup(known: &[PathBuf], out: &mut Vec<Artifact>) {
    if known.is_empty() {
        return;
    }
    let known_lower: Vec<String> = known.iter().map(|p| p.to_string_lossy().to_lowercase()).collect();
    for item in crate::startup::list_startup() {
        let cmd = item.command.to_lowercase();
        let under = known_lower.iter().any(|k| is_under(k, &cmd))
            || first_exe_path(&item.command)
                .map(|e| {
                    let s = e.to_string_lossy().to_lowercase();
                    known_lower.iter().any(|k| is_under(k, &s))
                })
                .unwrap_or(false);
        if under {
            out.push(Artifact {
                path: format!("{}|{}", item.location, item.name),
                kind: ArtifactKind::Startup,
                category: Category::AppRuntime,
                size: 0,
                confidence: Confidence::High,
                source: "startup".into(),
                reversible: true,
                match_reason: format!("启动项命令指向安装目录：{}", item.command),
            });
        }
    }
}

// ─── 组装 ─────────────────────────────────────────────────────────────

pub fn resolve(app: &AppEntry) -> Footprint {
    let (roots, warnings) = known_roots(app);
    let id = identity(&app.name, &app.publisher);
    let mut artifacts: Vec<Artifact> = Vec::new();

    // 安装目录（High）
    for root in &roots {
        let path_str = root.to_string_lossy().to_string();
        artifacts.push(Artifact {
            size: dir_size(root),
            category: Category::Binary,
            path: path_str,
            kind: ArtifactKind::Dir,
            confidence: Confidence::High,
            source: "install-dir".into(),
            reversible: true,
            match_reason: "应用安装目录".into(),
        });
    }

    scan_related_services(&roots, &mut artifacts);
    scan_related_startup(&roots, &mut artifacts);
    scan_fs_residue(&id, &roots, &mut artifacts);
    scan_reg_residue(&id, &mut artifacts);

    let total_size: u64 = artifacts.iter().map(|a| a.size).sum();
    let high_conf_size: u64 = artifacts
        .iter()
        .filter(|a| a.confidence == Confidence::High)
        .map(|a| a.size)
        .sum();
    let residue_size: u64 = artifacts
        .iter()
        .filter(|a| a.source.starts_with("residue"))
        .map(|a| a.size)
        .sum();

    Footprint {
        app_id: app.id.clone(),
        app_name: app.name.clone(),
        known_roots: roots.iter().map(|p| p.to_string_lossy().to_string()).collect(),
        artifacts,
        total_size,
        high_conf_size,
        residue_size,
        warnings,
        app_removed: false,
    }
}

// ─── 产物缓存 + 存在性复扫 ────────────────────────────────────────────
// 支撑"厂商卸载器把应用从注册表删掉之后，仍能复扫/清理其残留"。

static FOOTPRINT_CACHE: LazyLock<Mutex<HashMap<String, Footprint>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn cache_put(fp: &Footprint) {
    if let Ok(mut c) = FOOTPRINT_CACHE.lock() {
        c.insert(fp.app_id.clone(), fp.clone());
    }
}

fn cache_get(app_id: &str) -> Option<Footprint> {
    FOOTPRINT_CACHE.lock().ok().and_then(|c| c.get(app_id).cloned())
}

/// 注册表键是否仍存在。path 形如 "HKCU\\Software\\Foo"。
fn reg_key_exists(path: &str) -> bool {
    let (hive, sub) = match path.split_once('\\') {
        Some(("HKCU", rest)) => (HKEY_CURRENT_USER, rest),
        Some(("HKLM", rest)) => (HKEY_LOCAL_MACHINE, rest),
        _ => return false,
    };
    let subw = app_inventory::to_wide(sub);
    unsafe {
        let mut h = HKEY::default();
        if RegOpenKeyExW(hive, PCWSTR(subw.as_ptr()), 0, KEY_READ, &mut h) == ERROR_SUCCESS {
            let _ = RegCloseKey(h);
            true
        } else {
            false
        }
    }
}

/// 某产物是否仍存在于磁盘/注册表。
fn artifact_exists(a: &Artifact) -> bool {
    match a.kind {
        ArtifactKind::Dir => Path::new(&a.path).exists(),
        ArtifactKind::RegKey => reg_key_exists(&a.path),
        // 服务/启动项：卸载后多半已被清；不做存在性过滤，仍展示供参考
        ArtifactKind::Service | ArtifactKind::Startup => true,
    }
}

/// 应用已从注册表消失时：用缓存产物 + 逐项存在性复扫，只保留仍残留的产物。
fn rescan_cached(app_id: &str) -> Option<Footprint> {
    let cached = cache_get(app_id)?;
    let mut artifacts: Vec<Artifact> = Vec::new();
    for mut a in cached.artifacts {
        if !artifact_exists(&a) {
            continue; // 已被删除（厂商卸载或我们已清理）
        }
        if a.kind == ArtifactKind::Dir {
            a.size = dir_size(Path::new(&a.path)); // 卸载后目录可能变小，重算
        }
        artifacts.push(a);
    }
    let total_size: u64 = artifacts.iter().map(|x| x.size).sum();
    let high_conf_size: u64 = artifacts
        .iter()
        .filter(|x| x.confidence == Confidence::High)
        .map(|x| x.size)
        .sum();
    let residue_size: u64 = artifacts
        .iter()
        .filter(|x| x.source.starts_with("residue"))
        .map(|x| x.size)
        .sum();
    let fp = Footprint {
        app_id: cached.app_id,
        app_name: cached.app_name,
        known_roots: cached.known_roots,
        artifacts,
        total_size,
        high_conf_size,
        residue_size,
        warnings: cached.warnings.clone(),
        app_removed: true,
    };
    cache_put(&fp);
    Some(fp)
}

/// 按 app_id 解析（后端独立从注册表重取应用，绝不信任前端传来的路径）。
/// 应用仍在注册表：正常解析并缓存。已从注册表消失（多半被厂商卸载）：回退到
/// 缓存 + 逐项存在性复扫，只返回仍残留的产物——这样"卸载后清理残留"才成立。
pub fn resolve_by_id(app_id: &str) -> Result<Footprint, String> {
    let apps = app_inventory::list_installed_apps();
    match apps.into_iter().find(|a| a.id == app_id) {
        Some(app) => {
            let fp = resolve(&app);
            cache_put(&fp);
            Ok(fp)
        }
        None => rescan_cached(app_id)
            .ok_or_else(|| "应用已卸载且无缓存产物记录（请在卸载前先在本工具里扫描一次产物）".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_and_exe_parsing() {
        assert_eq!(strip_quotes("\"C:\\A B\\x\""), "C:\\A B\\x");
        assert_eq!(strip_quotes("C:\\A\\x"), "C:\\A\\x");
        assert_eq!(
            first_exe_path("\"C:\\Program Files\\App\\u.exe\" /S").unwrap(),
            PathBuf::from("C:\\Program Files\\App\\u.exe")
        );
        assert_eq!(
            first_exe_path("C:\\App\\uninst.exe --mode").unwrap(),
            PathBuf::from("C:\\App\\uninst.exe")
        );
    }

    #[test]
    fn icon_dir_extraction() {
        assert_eq!(
            dir_from_icon("C:\\App\\app.exe,0").unwrap(),
            PathBuf::from("C:\\App")
        );
        assert_eq!(
            dir_from_icon("\"C:\\App\\app.exe\"").unwrap(),
            PathBuf::from("C:\\App")
        );
    }

    #[test]
    fn identity_and_matching() {
        let id = identity("CC Switch", "ccswitch");
        assert_eq!(id.prod_compact, "ccswitch");
        // 精确/相关目录名 -> Medium
        assert!(matches!(name_match("CC Switch", &id), Some((Confidence::Medium, _))));
        assert!(matches!(name_match("ccswitch", &id), Some((Confidence::Medium, _))));
        // 无关目录 -> None
        assert!(name_match("Google", &id).is_none());
        assert!(name_match("NVIDIA Corporation", &id).is_none());
    }

    #[test]
    fn generic_tokens_dropped() {
        // "Cockpit Tools" 的 "tools" 是通用词，应被丢弃，避免撞任意 Tools 目录
        let id = identity("Cockpit Tools", "jlcodes");
        assert!(id.prod_tokens.contains(&"cockpit".to_string()));
        assert!(!id.prod_tokens.contains(&"tools".to_string()));
        assert!(name_match("Tools", &id).is_none());
        assert!(matches!(name_match("Cockpit Tools", &id), Some((Confidence::Medium, _))));
    }

    #[test]
    fn classify_paths() {
        assert_eq!(classify("C:\\Users\\x\\AppData\\Local\\Foo\\Cache"), Category::Cache);
        assert_eq!(classify("C:\\Users\\x\\Documents\\FooProjects"), Category::UserData);
        assert_eq!(classify("C:\\Program Files\\Foo"), Category::Binary);
    }

    #[test]
    fn under_check() {
        assert!(is_under("C:\\App", "c:\\app\\sub\\x"));
        assert!(is_under("C:\\App", "C:\\App"));
        assert!(!is_under("C:\\App", "C:\\Application\\x")); // 不能前缀误判
        assert!(!is_under("", "C:\\anything"));
    }

    #[test]
    fn resolve_real_app_smoke() {
        let apps = crate::app_inventory::list_installed_apps();
        let app = match apps.iter().find(|a| !a.install_location.is_empty()) {
            Some(a) => a.clone(),
            None => return, // 该机器无带安装目录的应用则跳过
        };
        let fp = resolve(&app);
        // 安装目录应作为 High 置信产物出现
        assert_eq!(fp.app_id, app.id);
        assert!(fp.artifacts.iter().any(|a| a.confidence == Confidence::High));
    }
}
