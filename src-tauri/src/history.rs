//! 环形历史缓存：在内存中保存最近 1 小时的指标快照，前端可按需查询。
//! 1 秒采样 → 3600 点/小时；仅保留最近 3600 条，内存约 ~200 KB。

use std::sync::Mutex;
use std::sync::LazyLock;
use crate::collector::MetricsSnapshot;

static RING: LazyLock<Mutex<Vec<MetricsSnapshot>>> =
    LazyLock::new(|| Mutex::new(Vec::with_capacity(3700)));
const MAX: usize = 3600;

/// 由 collector 每周期调用，追加一条快照。
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
