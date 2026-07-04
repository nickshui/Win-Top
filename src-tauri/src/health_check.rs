//! 系统健康体检：聚合多个模块的扫描结果，生成评分 + 问题清单 + 建议。
//! 纯数据汇总，不做任何修改。

use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct HealthReport {
    pub score: u32,               // 0-100
    pub junk_mb: f64,             // 可清理垃圾总量
    pub startup_count: u32,       // 自启项总数
    pub startup_disabled: u32,    // 已禁用的自启项
    pub heavy_procs: Vec<HeavyProc>, // CPU/内存占用高的进程
    pub issues: Vec<HealthIssue>, // 发现的问题列表
    pub suggestions: Vec<String>, // 优化建议
}

#[derive(Serialize, Clone)]
pub struct HeavyProc {
    pub pid: u32,
    pub name: String,
    pub cpu: f64,
    pub mem_mb: f64,
    pub reason: String, // "高 CPU" | "高内存" | "高磁盘 I/O"
}

#[derive(Serialize, Clone)]
pub struct HealthIssue {
    pub category: String,   // "垃圾清理" | "启动项" | "进程" | "磁盘"
    pub severity: String,   // "info" | "warn" | "danger"
    pub title: String,
    pub detail: String,
    pub actionable: bool,
}

/// 聚合体检：不修改任何系统设置，仅返回报告。
pub fn run_health_check() -> Result<HealthReport, String> {
    let mut issues: Vec<HealthIssue> = Vec::new();
    let mut suggestions: Vec<String> = Vec::new();

    // 1. 垃圾扫描
    let junk = crate::cleanup::scan_junk();
    let junk_mb = junk.total_bytes as f64 / 1024.0 / 1024.0;
    if junk_mb > 100.0 {
        issues.push(HealthIssue {
            category: "垃圾清理".into(),
            severity: if junk_mb > 1024.0 { "danger" } else { "warn" }.into(),
            title: format!("可清理 {:.0} MB 系统垃圾", junk_mb),
            detail: junk.categories.iter()
                .filter(|c| c.bytes > 0)
                .map(|c| format!("{}: {:.1} MB", c.label, c.bytes as f64 / 1024.0 / 1024.0))
                .collect::<Vec<_>>()
                .join(", "),
            actionable: true,
        });
        suggestions.push("前往「垃圾清理」释放磁盘空间".into());
    }

    // 2. 启动项
    let startup = crate::startup::list_startup();
    let enabled_startup: Vec<_> = startup.iter().filter(|s| s.enabled).collect();
    if enabled_startup.len() > 8 {
        issues.push(HealthIssue {
            category: "启动项".into(),
            severity: if enabled_startup.len() > 15 { "danger" } else { "warn" }.into(),
            title: format!("{} 个自启程序可能拖慢开机", enabled_startup.len()),
            detail: enabled_startup.iter()
                .take(5)
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
                .join(", "),
            actionable: true,
        });
        suggestions.push("前往「性能加速」禁用不必要的启动项".into());
    }
    let startup_count = startup.len() as u32;
    let startup_disabled = startup.iter().filter(|s| !s.enabled).count() as u32;

    // 3. 高资源占用进程
    let procs = crate::process::list_processes().unwrap_or_default();
    let heavy: Vec<HeavyProc> = procs.iter()
        .filter(|p| p.cpu > 30.0 || p.mem_mb > 500.0)
        .take(8)
        .map(|p| {
            let reason = if p.cpu > 50.0 {
                "高 CPU".into()
            } else if p.mem_mb > 1000.0 {
                "高内存".into()
            } else if p.cpu > 30.0 && p.mem_mb > 500.0 {
                "高 CPU + 内存".into()
            } else if p.cpu > 30.0 {
                "高 CPU".into()
            } else {
                "高内存".into()
            };
            HeavyProc { pid: p.pid, name: p.name.clone(), cpu: p.cpu, mem_mb: p.mem_mb, reason }
        })
        .collect();

    if !heavy.is_empty() {
        let count = heavy.len();
        issues.push(HealthIssue {
            category: "进程".into(),
            severity: if heavy.iter().any(|p| p.cpu > 70.0) { "danger" } else { "warn" }.into(),
            title: format!("{} 个进程资源占用较高", count),
            detail: heavy.iter().take(3).map(|p| format!("{} (CPU {:.0}%)", p.name, p.cpu)).collect::<Vec<_>>().join(", "),
            actionable: true,
        });
    }

    // 4. 磁盘健康
    let disk = crate::disk::report();
    for v in &disk.volumes {
        if v.used_pct > 90.0 {
            issues.push(HealthIssue {
                category: "磁盘".into(),
                severity: "danger".into(),
                title: format!("{} 盘仅剩 {:.1} GB", v.drive, v.free as f64 / 1024.0 / 1024.0 / 1024.0),
                detail: format!("已使用 {:.0}%", v.used_pct),
                actionable: true,
            });
            suggestions.push(format!("建议清理 {} 盘无用文件", v.drive));
        }
    }
    for d in &disk.disks {
        if !d.healthy {
            issues.push(HealthIssue {
                category: "磁盘".into(),
                severity: "danger".into(),
                title: format!("磁盘 {} 健康状态异常", d.model),
                detail: format!("状态: {}", d.status),
                actionable: false,
            });
        }
    }

    // 计算评分
    let score = calc_score(&issues, junk_mb, enabled_startup.len(), &heavy);

    Ok(HealthReport {
        score,
        junk_mb,
        startup_count,
        startup_disabled,
        heavy_procs: heavy,
        issues,
        suggestions,
    })
}

fn calc_score(issues: &[HealthIssue], junk_mb: f64, startup: usize, heavy: &[HeavyProc]) -> u32 {
    let mut s: i32 = 100;

    // 垃圾 > 100MB 扣分
    if junk_mb > 5000.0 { s -= 20; }
    else if junk_mb > 1000.0 { s -= 12; }
    else if junk_mb > 100.0 { s -= 5; }

    // 启动项过多扣分
    if startup > 20 { s -= 15; }
    else if startup > 12 { s -= 8; }
    else if startup > 8 { s -= 4; }

    // 高资源进程扣分
    if heavy.iter().any(|p| p.cpu > 70.0) { s -= 15; }
    else if heavy.len() > 3 { s -= 8; }
    else if heavy.len() > 1 { s -= 4; }

    // 严重问题扣分
    let danger_count = issues.iter().filter(|i| i.severity == "danger").count();
    s -= (danger_count as i32) * 5;

    s.max(0).min(100) as u32
}
