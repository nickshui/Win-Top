import { writable } from "svelte/store";
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
  invoke("is_elevated")
    .then((v) => elevated.set(!!v))
    .catch(() => {});
  listen("proc-event", (e) => {
    procEvents.update((l) => [{ ...e.payload, _id: ++eventSeq }, ...l].slice(0, MAX_EVENTS));
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
