//! 每进程网络流量（EventTier）：订阅 Microsoft-Windows-Kernel-Network，
//! 按 PID 累加收发字节，采样线程每秒算速率并 emit。需要管理员权限。

use std::collections::HashMap;

/// 纯函数：由上一轮/本轮累计字节快照(pid->(sent,recv))与时间差，算每进程速率。
/// down = recv 增量 / elapsed，up = sent 增量 / elapsed。
/// 新 PID（prev 缺失）以 cur 作基线 → 速率 0。
/// 仅产出累计>0 的进程行。返回 (total_down_bps, total_up_bps, rows)，
/// rows 元素 = (pid, down_bps, up_bps, sent_total, recv_total)。
fn diff_rates(
    prev: &HashMap<u32, (u64, u64)>,
    cur: &HashMap<u32, (u64, u64)>,
    elapsed: f64,
) -> (f64, f64, Vec<(u32, f64, f64, u64, u64)>) {
    let mut total_down = 0.0;
    let mut total_up = 0.0;
    let mut rows = Vec::new();
    if elapsed <= 0.0 {
        return (0.0, 0.0, rows);
    }
    for (&pid, &(sent, recv)) in cur {
        let (psent, precv) = prev.get(&pid).copied().unwrap_or((sent, recv));
        let up = sent.saturating_sub(psent) as f64 / elapsed;
        let down = recv.saturating_sub(precv) as f64 / elapsed;
        total_up += up;
        total_down += down;
        if sent + recv > 0 {
            rows.push((pid, down, up, sent, recv));
        }
    }
    (total_down, total_up, rows)
}

#[cfg(test)]
mod tests {
    use super::diff_rates;
    use std::collections::HashMap;

    #[test]
    fn computes_per_pid_rates_and_totals() {
        let mut prev = HashMap::new();
        prev.insert(100u32, (1_000u64, 2_000u64)); // (sent, recv)
        let mut cur = HashMap::new();
        cur.insert(100u32, (1_500u64, 6_000u64)); // +500 sent, +4000 recv，跨 2 秒

        let (down, up, rows) = diff_rates(&prev, &cur, 2.0);
        assert_eq!(rows.len(), 1);
        let (pid, d, u, sent, recv) = rows[0];
        assert_eq!(pid, 100);
        assert!((d - 2000.0).abs() < 1e-6, "down={d}");
        assert!((u - 250.0).abs() < 1e-6, "up={u}");
        assert_eq!(sent, 1500);
        assert_eq!(recv, 6000);
        assert!((down - 2000.0).abs() < 1e-6);
        assert!((up - 250.0).abs() < 1e-6);
    }

    #[test]
    fn new_pid_uses_baseline_zero_rate() {
        let prev = HashMap::new();
        let mut cur = HashMap::new();
        cur.insert(7u32, (5_000u64, 5_000u64));
        let (_d, _u, rows) = diff_rates(&prev, &cur, 1.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, 0.0);
        assert_eq!(rows[0].2, 0.0);
    }

    #[test]
    fn zero_elapsed_yields_nothing() {
        let prev = HashMap::new();
        let mut cur = HashMap::new();
        cur.insert(1u32, (10, 10));
        let (d, u, rows) = diff_rates(&prev, &cur, 0.0);
        assert_eq!(d, 0.0);
        assert_eq!(u, 0.0);
        assert!(rows.is_empty());
    }
}
