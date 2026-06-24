# 设计：优化中心（一键清理 / 内存加速 / 启动项）

日期：2026-06-15
状态：已确认，待写实现计划

## 目标

把当前占位的「工具箱」改造成**优化中心**，提供三类系统优化能力，组织为两个子标签：
- **一键优化**：垃圾清理（标准档）+ 内存释放，合成一个扫描→确认→执行→结果的流程；执行后给出「建议关闭的后台进程」候选。
- **启动项**：列出开机自启项并可启用/禁用（可逆，与任务管理器一致）。

三块是**三个独立模块**（cleanup / memboost / startup），边界清晰、可独立实现与测试，统一在一个前端视图里呈现。

## 约束与姿态（如实）

- **垃圾清理是破坏性操作**：必须「扫描→列分类+体积→用户勾选→确认弹窗→执行」，逐文件容错（被占用就跳过计数），只在各分类固定绝对根目录内操作，不越界、不跟符号链接。
- **内存加速是非破坏但半安慰剂**：`EmptyWorkingSet` 裁剪工作集让「已用内存」数字下降，系统本会按需回收、被裁页之后访问会重新换入。做，但 UI 文案不夸大。
- **关闭后台进程绝不自动执行**：只给「占内存大的后台进程」候选列表（已排除关键进程），用户勾选 + 确认弹窗才结束。
- **管理员**：系统级清理（系统 Temp / Windows Update 缓存 / Prefetch）、HKLM 启动项、裁剪其它用户/系统进程都需管理员。非提权时这些项标注「需管理员」并提供「以管理员重启」（复用 `relaunchAdmin`），用户级操作（用户 Temp / 回收站 / 浏览器缓存 / HKCU 启动项 / 自身进程裁剪）照常可用。

## 信息架构

- `App.svelte` 把 `toolbox` 路由从 `Placeholder` 改指向新视图 `Optimize.svelte`。
- Sidebar 项 `toolbox` 标签由「工具箱」改为「优化加速」（id 不变，避免改路由键）。
- `Optimize.svelte` 两个子标签（复用网络视图的标签页交互/样式，本组件内自带样式）：「一键优化」/「启动项」。顶部非提权时显示「部分项需管理员 · 以管理员重启」横幅。

---

## 模块 1：垃圾清理 `src-tauri/src/cleanup.rs`

### 分类（标准档）

| id | 标签 | 根目录 | 需管理员 |
|---|---|---|---|
| user_temp | 用户临时文件 | `%TEMP%` 与 `%LOCALAPPDATA%\Temp` | 否 |
| system_temp | 系统临时文件 | `C:\Windows\Temp` | 是 |
| recycle_bin | 回收站 | `SHQueryRecycleBin`/`SHEmptyRecycleBin` | 否 |
| thumbnails | 缩略图缓存 | `%LOCALAPPDATA%\Microsoft\Windows\Explorer\thumbcache_*.db` `iconcache_*.db` | 否 |
| windows_update | Windows Update 缓存 | `C:\Windows\SoftwareDistribution\Download` | 是 |
| prefetch | Prefetch | `C:\Windows\Prefetch` | 是 |
| edge_cache | Edge 缓存 | `%LOCALAPPDATA%\Microsoft\Edge\User Data\*\Cache` | 否 |
| chrome_cache | Chrome 缓存 | `%LOCALAPPDATA%\Google\Chrome\User Data\*\Cache` | 否 |

`*` 表示遍历 `Default` 与 `Profile N` 配置目录。删除时清空根目录内容，保留根目录本身。

### 接口

- `scan_junk() -> CleanupReport`：对每个分类递归求体积+文件数（不删）。回收站走 `SHQueryRecycleBinW`（直接拿体积+项数，不枚举）。在 `spawn_blocking` 跑。
- `clean_junk(ids: Vec<String>) -> CleanupResult`：删所选分类。文件级 `remove_file`/目录 `remove_dir_all` 容错——失败（被占用等）跳过并计数，累加已释放字节。回收站走 `SHEmptyRecycleBinW`（NoConfirmation|NoProgressUI|NoSound）。`spawn_blocking` 跑。

### 数据结构

```rust
struct CleanupCategory { id: String, label: String, bytes: u64, files: u64, needs_admin: bool, available: bool }
struct CleanupReport { categories: Vec<CleanupCategory>, total_bytes: u64 }
struct CleanupItemResult { id: String, freed_bytes: u64, skipped: u64 }
struct CleanupResult { freed_bytes: u64, items: Vec<CleanupItemResult> }
```

`available` = 根目录存在。`needs_admin` 项在非提权时仍可被扫描（若可读），但 UI 标注需提权清理。

### 可测纯函数

`dir_stats(path) -> (u64 bytes, u64 files)`（递归累加，目录不存在返回 0）——单元测试用 `std::env::temp_dir()` 下临时子目录造文件验证。`clean_dir_contents(path) -> (freed, skipped)` 同理用临时目录测删除与跳过计数。**测试只碰自建临时目录，绝不碰真实系统路径。**

---

## 模块 2：内存加速 `src-tauri/src/memboost.rs`

### 接口

- `memory_boost() -> BoostResult`：
  1. `GlobalMemoryStatusEx` 记基线可用内存；
  2. 取进程 pid 列表（复用 `process` 的枚举）；逐个 `OpenProcess(PROCESS_SET_QUOTA|PROCESS_QUERY_LIMITED_INFORMATION)` + `EmptyWorkingSet(handle)` + `CloseHandle`，失败跳过、成功计数；
  3. 再 `GlobalMemoryStatusEx`；`freed_mb = max(0, after_avail - before_avail)`。
- `suggest_background() -> Vec<BgProc>`：复用 `process::list_processes()`，过滤关键进程 denylist + 排除自身/pid 0/4，按内存降序，仅取 `mem_mb` 超阈值（默认 50MB）的 Top-N（默认 8）。
- 关闭候选进程**复用现有 `terminate_process` 命令**（前端勾选 + 确认弹窗）。

### 数据结构

```rust
struct BoostResult { freed_mb: f64, trimmed_count: u32, before_avail_mb: f64, after_avail_mb: f64 }
struct BgProc { pid: u32, name: String, mem_mb: f64 }
```

### 关键进程 denylist（不进"建议关闭"列表，大小写不敏感）

`System`, `System Idle Process`, `Registry`, `smss.exe`, `csrss.exe`, `wininit.exe`, `winlogon.exe`, `services.exe`, `lsass.exe`, `svchost.exe`, `fontdrvhost.exe`, `dwm.exe`, `explorer.exe`, `sihost.exe`, `ctfmon.exe`, `conhost.exe`, `RuntimeBroker.exe`, `win-top.exe`。

### 可测纯函数

`pick_background(procs: &[(pid,name,mem)], deny: &Set, threshold_mb, n) -> Vec<BgProc>`——单元测试验证过滤/排序/截断。

---

## 模块 3：启动项 `src-tauri/src/startup.rs`

### 来源

- 注册表 Run：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`、`HKLM\Software\Microsoft\Windows\CurrentVersion\Run`。
- 启动文件夹：`%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup`（用户）、`%ProgramData%\Microsoft\Windows\Start Menu\Programs\Startup`（公共）下的 `.lnk`。

### 启用/禁用机制（与任务管理器一致、可逆）

Windows 把启用态存在 `StartupApproved` 键，**不删原始项**：
- Run 项：`<hive>\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run`，值名 = Run 值名。
- 文件夹项：`...\StartupApproved\StartupFolder`，值名 = `.lnk` 文件名。
- 值为 12 字节二进制：首字节 `0x02`/`0x06` = 启用，`0x03` = 禁用；值缺失视为启用。
- 禁用写 `[0x03,0,0,0,0,0,0,0,0,0,0,0]`，启用写 `[0x02,0,0,0,0,0,0,0,0,0,0,0]`。HKLM 项需管理员。

### 接口

- `list_startup() -> Vec<StartupItem>`：枚举 Run 值 + 启动文件夹 `.lnk`，交叉 `StartupApproved` 判 `enabled`。
- `set_startup_enabled(id: String, enabled: bool) -> ActionResult`：按 id 解析 location+name，写对应 `StartupApproved` 值。

### 数据结构

```rust
struct StartupItem { id: String, name: String, command: String, location: String, enabled: bool }
// location ∈ {"HKCU-Run","HKLM-Run","User-Folder","Common-Folder"}
// id = "{location}|{name}"
// 返回值复用现有 process::ActionResult { success: bool, message: String }
```

### 可测纯函数

`parse_approved_state(bytes: &[u8]) -> bool`（按首字节判启用）与 `encode_approved(enabled) -> [u8;12]`——单元测试。

### 依赖

注册表用 windows-rs `Win32_System_Registry`（`RegOpenKeyExW`/`RegEnumValueW`/`RegQueryValueExW`/`RegSetValueExW`/`RegCloseKey`），不引新 crate。MVP **不做**签名/发布者校验、不扫 Task Scheduler / 服务（后续 spec）。

---

## 接线与依赖

- `main.rs` 声明 `mod cleanup; mod memboost; mod startup;`，注册命令：`scan_junk`、`clean_junk`、`memory_boost`、`suggest_background`、`list_startup`、`set_startup_enabled`（关闭后台复用已有 `terminate_process`）。
- `src-tauri/Cargo.toml` windows features 新增：`Win32_System_ProcessStatus`（EmptyWorkingSet）、`Win32_System_Registry`（启动项）。`Win32_UI_Shell`（SH*RecycleBin*）、`Win32_Storage_FileSystem`、`Win32_System_Threading` 已启用。

## 前端 `src/lib/views/Optimize.svelte`

- 子标签「一键优化」：
  - `扫描` → `scan_junk`，仪表区显示「可清理 X.X GB」+ 分类勾选列表（体积/文件数；`needs_admin` 且未提权的项灰显+锁标）。默认勾选所有 `available` 分类。
  - `一键优化` → 确认弹窗 → `clean_junk(选中ids)` + `memory_boost` → 结果摘要卡（释放磁盘 / 释放内存 / 裁剪进程数）→ 调 `suggest_background` 出「建议关闭的后台进程」勾选列表 → `结束所选`（确认弹窗 → 逐个 `terminate_process`）。
- 子标签「启动项」：`onMount` 调 `list_startup` → 表格（名称 / 命令 / 位置 / 启用开关）；开关调 `set_startup_enabled` 后刷新。
- 非提权横幅复用 `elevated` store + `relaunchAdmin`。
- 这些都是按需 `invoke`，不需常驻事件订阅。

## 测试策略

- cleanup：`dir_stats` / `clean_dir_contents` 纯函数单测（临时目录）。
- memboost：`pick_background` 纯函数单测。
- startup：`parse_approved_state` / `encode_approved` 纯函数单测。
- 与系统强相关的 SH*/注册表写/EmptyWorkingSet：`cargo check` + 提权实测（清理对照磁盘占用、加速看可用内存、启动项开关后任务管理器对照、禁用项重启验证不自启）。
- GUI：WebView2 无法 attach，最终渲染由用户截图确认。
- 构建：release 必须 `--features custom-protocol`（已知坑）；替换 exe 前关闭在跑实例。

## 不在本特性范围（后续独立 spec）

- 启动项签名/发布者校验、Task Scheduler / 服务自启、彻底删除自启项。
- 磁盘空间 treemap、概览 Bento、进程管理增强、Ctrl+K 命令面板。
