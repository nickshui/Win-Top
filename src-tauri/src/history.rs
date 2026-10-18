//! 环形历史缓存（热数据）：在内存中保存最近 1 小时的指标快照，前端可按需查询。
//! 1 秒采样 → 3600 点/小时。
//!
//! 内存实测约 **787.4 KiB**（证据：`docs/research/evidence/2026-10-18-storage-and-adapter-measurements.md` §B2），
//! 不是旧注释写的 "~200 KB"。两个原因：
//!   1. `Vec::with_capacity(3700)` 在 push #3700 时触发扩容到 **7400**，而随后的
//!      `drain(0..excess)` **不缩容** —— 稳态缓冲是 7400 × 96 B = 693.8 KiB，不是 3600 × 96 B；
//!   2. 每条 `ts` 的堆分配合计约 93.6 KiB。
//!
//! 冷数据（跨重启、最长 90 天）由 `history_store` 落盘到
//! `%LOCALAPPDATA%\WinTop\history\`；本模块只负责热路径，**不做任何磁盘 I/O**。

use std::sync::LazyLock;
use std::sync::Mutex;

use crate::collector::MetricsSnapshot;

static RING: LazyLock<Mutex<Vec<MetricsSnapshot>>> =
    LazyLock::new(|| Mutex::new(Vec::with_capacity(3700)));
const MAX: usize = 3600;

/// 由 collector 每周期调用，追加一条快照（不阻塞、无磁盘 I/O）。
pub fn push(snapshot: MetricsSnapshot) {
    if let Ok(mut v) = RING.lock() {
        v.push(snapshot);
        if v.len() > MAX + 100 {
            let excess = v.len() - MAX;
            v.drain(0..excess);
        }
    }
}

/// 返回最近 N 条快照（最多 MAX），按时间正序。
pub fn recent(n: usize) -> Vec<MetricsSnapshot> {
    let n = n.min(MAX);
    RING.lock()
        .map(|v| v.iter().rev().take(n).rev().cloned().collect())
        .unwrap_or_default()
}

/// 返回 `ts_ms` **大于** `after_ms` 的快照，按时间正序。
///
/// 供 `history_store` 的写线程每 15 秒增量取数；锁内只做过滤 + clone（一次最多几十条），
/// 对 1 Hz 的 `push` 无可感知影响。
pub fn snapshot_since(after_ms: i64) -> Vec<MetricsSnapshot> {
    RING.lock()
        .map(|v| v.iter().filter(|s| s.ts_ms > after_ms).cloned().collect())
        .unwrap_or_default()
}
