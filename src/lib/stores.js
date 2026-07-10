import { writable, get } from "svelte/store";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/tauri";

// 实时指标（来自后端常驻采集器的 metrics 事件）
export const metrics = writable(null);
export const cpuHistory = writable([]);
export const connected = writable(false);

const MAX_HISTORY = 60;

// 在应用根级调用一次：订阅事件并写入 store，使数据在视图切换间保持。
// 返回取消订阅函数。
export function startMetrics() {
  let unlisten;
  listen("metrics", (e) => {
    const m = e.payload;
    connected.set(true);
    metrics.set(m);
    cpuHistory.update((h) => [...h, m.cpu].slice(-MAX_HISTORY));
  }).then((u) => {
    unlisten = u;
  });

  return () => {
    if (unlisten) unlisten();
  };
}

export { MAX_HISTORY };

// 历史趋势数据（来自后端的环形缓冲区）
export const historyData = writable([]);

export async function loadHistory(n = 120) {
  try {
    const data = await invoke("get_history", { n });
    historyData.set(data);
  } catch (e) {
    // silently fail
  }
}

// 轻量 Toast 通知
export const toasts = writable([]);
let toastId = 0;

export function pushToast(message, kind = "ok") {
  const id = ++toastId;
  toasts.update((list) => [...list, { id, message, kind }]);
  setTimeout(() => {
    toasts.update((list) => list.filter((t) => t.id !== id));
  }, 3800);
}

// 权限 + ETW 实时事件
export const elevated = writable(false);
export const etwAvailable = writable(false);
export const etwReason = writable("");
export const procEvents = writable([]);
const MAX_EVENTS = 200;

let eventSeq = 0;

function refreshEtwStatus() {
  invoke("get_etw_status")
    .then((s) => {
      etwAvailable.set(!!s.available);
      etwReason.set(s.reason || "");
    })
    .catch(() => {});
}

export function startEvents() {
  let un1, un2;
  // 批量节流：高频事件（每秒上百条）先入缓冲，每 250ms 合并刷新一次，
  // 避免逐条 re-render 造成的抖动/残影，同时大幅降低开销。
  let pending = [];
  let flushTimer = null;
  const flush = () => {
    flushTimer = null;
    if (!pending.length) return;
    const batch = pending;
    pending = [];
    procEvents.update((list) => {
      const newestFirst = [];
      for (let i = batch.length - 1; i >= 0; i--) newestFirst.push(batch[i]);
      return [...newestFirst, ...list].slice(0, MAX_EVENTS);
    });
  };

  invoke("is_elevated")
    .then((v) => elevated.set(!!v))
    .catch(() => {});
  listen("proc-event", (e) => {
    pending.push({ ...e.payload, _id: ++eventSeq });
    if (!flushTimer) flushTimer = setTimeout(flush, 250);
  }).then((u) => (un1 = u));
  listen("etw-status", (e) => {
    etwAvailable.set(!!e.payload.available);
    etwReason.set(e.payload.reason || "");
  }).then((u) => (un2 = u));
  // 主动查询当前状态（规避后端先于监听器 emit 的时序竞争），并稍后重查一次
  refreshEtwStatus();
  setTimeout(refreshEtwStatus, 1500);
  return () => {
    if (un1) un1();
    if (un2) un2();
    if (flushTimer) clearTimeout(flushTimer);
  };
}

export async function relaunchAdmin() {
  try {
    await invoke("relaunch_as_admin");
    pushToast("已请求以管理员身份启动，请在 UAC 弹窗确认。", "ok");
  } catch (e) {
    pushToast(String(e), "error");
  }
}

// 每进程网络流量（来自 ETW Kernel-Network）
export const netTraffic = writable(null);
export const netTrafficAvailable = writable(false);
export const netTrafficReason = writable("");

function refreshNetTrafficStatus() {
  invoke("get_nettraffic_status")
    .then((s) => {
      netTrafficAvailable.set(!!s.available);
      netTrafficReason.set(s.reason || "");
    })
    .catch(() => {});
}

export function startNetTraffic() {
  let un1, un2;
  listen("net-traffic", (e) => netTraffic.set(e.payload)).then((u) => (un1 = u));
  listen("net-traffic-status", (e) => {
    netTrafficAvailable.set(!!e.payload.available);
    netTrafficReason.set(e.payload.reason || "");
  }).then((u) => (un2 = u));
  // 主动查询当前状态（规避后端先于监听器 emit 的时序竞争），并稍后重查一次
  refreshNetTrafficStatus();
  setTimeout(refreshNetTrafficStatus, 1500);
  return () => {
    if (un1) un1();
    if (un2) un2();
  };
}

// 每进程磁盘 I/O 追踪（来自 disk_io 线程的 disk-io 事件）
export const diskIo = writable(null);

export function startDiskIo() {
  let un;
  listen("disk-io", (e) => diskIo.set(e.payload)).then((u) => (un = u));
  return () => { if (un) un(); };
}

// 磁盘容量报告（供加速中心宫格展示系统盘用量）
export const diskReport = writable(null);

export async function loadDiskReport() {
  try {
    const r = await invoke("get_disk_report");
    diskReport.set(r);
    return r;
  } catch (e) {
    return null;
  }
}

// 定时清理通知
export const cleanupNotification = writable(null);

export function startCleanupNotifications() {
  let un;
  listen("scheduled-cleanup", (e) => {
    cleanupNotification.set(e.payload);
    if (e.payload?.total_mb > 0) {
      pushToast(`定时清理：发现 ${e.payload.total_mb.toFixed(0)} MB 可清理空间`, "ok");
    }
  }).then((u) => (un = u));
  return () => { if (un) un(); };
}

// ===== AI 助手：采集真实系统快照作为对话上下文 =====
// 汇总当前 metrics、进程 TOP、磁盘容量，生成结构化文本供 LLM 参考。
// 采集系统快照：仅读取后台常驻采集的 store（纯内存、零阻塞），
// 绝不调用 invoke("get_processes") 等同步 Tauri 命令——那会在 Rust 主线程
// 全量枚举进程，冻结整个 WebView UI。
export function collectSystemSnapshot() {
  const m = get(metrics);
  const io = get(diskIo);
  const disk = get(diskReport);
  const net = get(netTraffic);

  const lines = [];
  lines.push("【系统实时指标】");
  if (m) {
    lines.push(`CPU 使用率: ${m.cpu?.toFixed(1)}%`);
    lines.push(`内存: ${m.mem_used_gb?.toFixed(1)}/${m.mem_total_gb?.toFixed(1)} GB (${m.mem_load}%)`);
    lines.push(`页面文件: ${m.mem_page_used_gb?.toFixed(1)}/${m.mem_page_total_gb?.toFixed(1)} GB`);
    lines.push(`磁盘 I/O: 读 ${fmtBytesPerSec(m.disk_read_bps)} / 写 ${fmtBytesPerSec(m.disk_write_bps)}`);
    lines.push(`网络吞吐: ${(m.net_total_bps * 8 / 1e6).toFixed(2)} Mbps`);
  } else {
    lines.push("(实时指标尚未就绪，请稍候再问)");
  }

  // 磁盘 I/O TOP（来自后台 disk-io 事件，已含进程名/PID）
  if (io?.rows?.length) {
    lines.push("", "【磁盘 I/O 活跃进程 TOP 5】");
    io.rows.slice(0, 5).forEach((r) =>
      lines.push(`- ${r.name} (PID ${r.pid}): 读 ${fmtBytesPerSec(r.read_bps)}, 写 ${fmtBytesPerSec(r.write_bps)}`)
    );
  }

  // 网络流量 TOP（来自后台 net-traffic 事件，如可用）
  if (net?.rows?.length) {
    lines.push("", "【网络流量进程 TOP 5】");
    net.rows.slice(0, 5).forEach((r) =>
      lines.push(`- ${r.name} (PID ${r.pid}): ↓${fmtBytesPerSec(r.down_bps)} ↑${fmtBytesPerSec(r.up_bps)}`)
    );
  }

  if (disk?.volumes?.length) {
    lines.push("", "【磁盘分区】");
    disk.volumes.forEach((v) => {
      const freeGb = (v.free / 1024 ** 3).toFixed(1);
      const totalGb = (v.total / 1024 ** 3).toFixed(1);
      lines.push(`- ${v.drive} ${v.label || ""}: 已用 ${v.used_pct?.toFixed(0)}%, 可用 ${freeGb}/${totalGb} GB`);
    });
  } else {
    lines.push("", "【磁盘分区】(未加载，用户可到「磁盘管理」查看)");
  }

  lines.push("", "注：进程级 CPU/内存 TOP 明细可让用户到「进程管理」页按列排序查看。");
  return lines.join("\n");
}

function fmtBytesPerSec(v) {
  v = v || 0;
  if (v < 1024) return v.toFixed(0) + " B/s";
  if (v < 1024 * 1024) return (v / 1024).toFixed(1) + " KB/s";
  if (v < 1024 ** 3) return (v / 1024 ** 2).toFixed(1) + " MB/s";
  return (v / 1024 ** 3).toFixed(1) + " GB/s";
}
