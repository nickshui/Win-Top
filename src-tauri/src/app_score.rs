//! 价值 / 代价评分：给每个已装应用一个「推荐卸载度」(0-100) + 可解释的理由。
//!
//! 原则（与卸载还原点同样的诚实底线）：
//!   - 分数只用于排序与浮现候选，**绝不自动勾选卸载**。
//!   - "很少用 ≠ 没用"：识别到疑似安全/驱动/系统/硬件类，打「谨慎」标并给分数封顶。
//!   - reasons 才是主角，数字是次要的。
//!
//! 只用便宜信号（注册表体积 + UserAssist 使用度 + 启动项），不做目录遍历，
//! 因此可对整份清单批量打分。更重的「实时资源代价」留到详情页（有 footprint 根目录时）再算。

use serde::Serialize;

use crate::app_inventory::AppEntry;
use crate::usage_stats::{self, UsageIndex};

#[derive(Serialize, Clone)]
pub struct AppScore {
    pub app_id: String,
    pub removal_recommendation: u8, // 0-100，越高越建议卸载
    pub reasons: Vec<String>,
    pub last_used_secs: Option<i64>,
    pub last_used_label: String,
    pub run_count: u32,
    pub autostart: bool,
    pub estimated_size_kb: u64,
    pub caution: bool, // 疑似安全/驱动/系统，卸载需谨慎
}

const DAY: i64 = 86_400;

/// 谨慎关键词：命中则打谨慎标并封顶分数（中英）。
const CAUTION_KW: &[&str] = &[
    "antivirus", "anti-virus", "defender", "security", "firewall", "driver", "drivers",
    "nvidia", "intel", "amd", "realtek", "vpn", "backup", "bitlocker", "encryption",
    "runtime", "redistributable", "framework", ".net", "visual c++", "directx",
    "杀毒", "安全", "防火墙", "驱动", "备份", "加密", "运行库", "组件",
];

fn is_caution(name: &str, publisher: &str) -> bool {
    let hay = format!("{} {}", name, publisher).to_lowercase();
    CAUTION_KW.iter().any(|k| hay.contains(k))
}

fn last_used_label(last: Option<i64>, now: i64) -> String {
    match last {
        None => "无使用记录".into(),
        Some(t) => {
            let days = (now - t) / DAY;
            if days <= 0 {
                "今天用过".into()
            } else if days == 1 {
                "昨天用过".into()
            } else if days < 30 {
                format!("{} 天前", days)
            } else if days < 365 {
                format!("约 {} 个月前", days / 30)
            } else {
                format!("超过 {} 年", days / 365)
            }
        }
    }
}

/// 判断某应用是否有开机自启（便宜匹配：启动项命令落在安装目录内，或命令含产品紧凑名）。
fn has_autostart(app: &AppEntry, startups: &[crate::startup::StartupItem]) -> bool {
    let loc = app.install_location.trim_matches('"').to_lowercase();
    let compact_name: String = app
        .name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    startups.iter().any(|s| {
        let cmd = s.command.to_lowercase();
        (loc.len() >= 4 && cmd.contains(&loc))
            || (compact_name.len() >= 5 && {
                let ccmd: String = cmd.chars().filter(|c| c.is_alphanumeric()).collect();
                ccmd.contains(&compact_name)
            })
    })
}

fn score_one(
    app: &AppEntry,
    idx: &UsageIndex,
    startups: &[crate::startup::StartupItem],
    now: i64,
) -> AppScore {
    // 匹配根：优先 InstallLocation；为空时用 DisplayIcon 推导目录（很多应用 InstallLocation 空）。
    let mut roots: Vec<String> = Vec::new();
    let loc = app.install_location.trim_matches('"').to_string();
    if !loc.is_empty() {
        roots.push(loc);
    } else if let Some(d) = crate::footprint::dir_from_icon(&app.display_icon) {
        roots.push(d.to_string_lossy().to_string());
    }
    let usage = idx.lookup(&roots, &app.name, &app.publisher);
    let autostart = has_autostart(app, startups);
    let caution = is_caution(&app.name, &app.publisher);

    let mut score: i32 = 0;
    let mut reasons: Vec<String> = Vec::new();

    let rarely_used = match usage.last_used_secs {
        Some(t) => {
            let days = (now - t) / DAY;
            if days > 365 {
                score += 45;
                reasons.push("超过一年未使用".into());
                true
            } else if days > 180 {
                score += 35;
                reasons.push("半年以上未使用".into());
                true
            } else if days > 90 {
                score += 20;
                reasons.push("三个月以上未使用".into());
                true
            } else if days > 30 {
                score += 8;
                reasons.push("一个月以上未使用".into());
                false
            } else if days < 7 {
                score -= 25;
                reasons.push("最近一周用过".into());
                false
            } else {
                false
            }
        }
        None => {
            reasons.push("无使用记录（可能极少启动）".into());
            true
        }
    };

    if autostart {
        score += 15;
        if rarely_used {
            score += 10;
            reasons.push("开机自启却很少使用".into());
        } else {
            reasons.push("开机自启".into());
        }
    }

    // 磁盘占用（弱乘数）
    let gb = app.estimated_size_kb as f64 / 1_048_576.0;
    if gb >= 5.0 {
        score += 10;
        reasons.push(format!("占用磁盘约 {:.1} GB", gb));
    } else if gb >= 1.0 {
        score += 6;
        reasons.push(format!("占用磁盘约 {:.1} GB", gb));
    } else if app.estimated_size_kb >= 100 * 1024 {
        score += 3;
    }

    // 谨慎护栏：疑似安全/驱动/系统 -> 封顶 40 + 打标
    if caution {
        reasons.push("疑似安全/驱动/系统组件，卸载需谨慎".into());
        if score > 40 {
            score = 40;
        }
    }

    let clamped = score.clamp(0, 100) as u8;

    AppScore {
        app_id: app.id.clone(),
        removal_recommendation: clamped,
        reasons,
        last_used_secs: usage.last_used_secs,
        last_used_label: last_used_label(usage.last_used_secs, now),
        run_count: usage.run_count,
        autostart,
        estimated_size_kb: app.estimated_size_kb,
        caution,
    }
}

/// 对整份应用清单批量打分（构建 UserAssist 索引 + 启动项列表各一次）。
pub fn score_all(apps: &[AppEntry]) -> Vec<AppScore> {
    let idx = usage_stats::build_index();
    let startups = crate::startup::list_startup();
    let now = usage_stats::now_unix();
    apps.iter().map(|a| score_one(a, &idx, &startups, now)).collect()
}

/// 应用 + 评分打包，供前端一次取回。
#[derive(Serialize, Clone)]
pub struct ScoredApp {
    pub entry: AppEntry,
    pub score: AppScore,
}

/// 列出全部应用并附带评分，按推荐卸载度降序。
pub fn list_scored() -> Vec<ScoredApp> {
    let apps = crate::app_inventory::list_installed_apps();
    let scores = score_all(&apps);
    let mut out: Vec<ScoredApp> = apps
        .into_iter()
        .zip(scores)
        .map(|(entry, score)| ScoredApp { entry, score })
        .collect();
    out.sort_by(|a, b| b.score.removal_recommendation.cmp(&a.score.removal_recommendation));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caution_detection() {
        assert!(is_caution("360 杀毒", "360"));
        assert!(is_caution("NVIDIA Graphics Driver", "NVIDIA"));
        assert!(is_caution("Microsoft Visual C++ 2022 Redistributable", "Microsoft"));
        assert!(!is_caution("Notepad++", "Don Ho"));
    }

    #[test]
    fn label_formatting() {
        let now = 1_000_000_000i64;
        assert_eq!(last_used_label(None, now), "无使用记录");
        assert_eq!(last_used_label(Some(now), now), "今天用过");
        assert_eq!(last_used_label(Some(now - 1 * DAY), now), "昨天用过");
        assert_eq!(last_used_label(Some(now - 5 * DAY), now), "5 天前");
        assert!(last_used_label(Some(now - 400 * DAY), now).contains("年"));
    }

    #[test]
    fn scored_ids_are_unique() {
        // 前端 {#each ... (entry.id)} 依赖 id 唯一；重复会导致 Svelte keyed-each 卡死。
        let scored = list_scored();
        let mut ids = std::collections::HashSet::new();
        for s in &scored {
            assert!(ids.insert(s.entry.id.clone()), "存在重复应用 id：{}", s.entry.id);
        }
    }

    #[test]
    fn caution_caps_score() {
        // 造一个“很久没用 + 自启”的安全软件，分数应被封顶到 ≤40
        let app = AppEntry {
            id: "HKLM|test".into(),
            name: "SomeAntivirus".into(),
            publisher: "SecCorp".into(),
            version: "1.0".into(),
            install_date: String::new(),
            install_location: String::new(),
            estimated_size_kb: 6 * 1024 * 1024, // 6GB
            uninstall_string: String::new(),
            quiet_uninstall_string: String::new(),
            display_icon: String::new(),
            is_msi: false,
            product_code: String::new(),
            arch: "x64".into(),
            source: "HKLM".into(),
        };
        // 用空索引（无使用记录 -> rarely_used=true）
        let idx = usage_stats::index_from(Vec::new());
        let s = score_one(&app, &idx, &[], 2_000_000_000);
        assert!(s.caution);
        assert!(s.removal_recommendation <= 40);
    }
}
