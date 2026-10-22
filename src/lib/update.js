// 应用更新：读取本地版本 → 询问后端 GitHub 最新版本 → 一键下载并自动安装。
//
// 边界：只访问 GitHub 公开 API / 官方 Release 资产；不静默提权、不上传任何本机数据。
// 安装方式（便携版原地替换 / NSIS / MSI）由后端按当前安装形态决定，
// 与 Tauri 内置 updater 的差异见 src-tauri/src/update.rs 顶部注释。
import { get, writable } from "svelte/store";
import { invoke } from "@tauri-apps/api/tauri";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { open } from "@tauri-apps/api/shell";
import { pushToast } from "./stores.js";

/// 当前本地版本（来自 tauri.conf.json 的 package.version，当前指向 package.json）。
export const currentVersion = writable("");
/// 最近一次检查结果（结构见 src-tauri/src/update.rs UpdateInfo）。
export const updateInfo = writable(null);
/// 是否正在请求版本信息。
export const updateChecking = writable(false);
/// 最近一次检查失败原因（空串 = 无错误）。
export const updateError = writable("");

/// 下载/安装进度（后端 update-progress 事件）。
export const updateProgress = writable({ phase: "", downloaded: 0, total: 0, pct: 0 });
/// 自动更新阶段："" | "preparing" | "downloading" | "verifying" | "installing"。
export const installPhase = writable("");
/// 自动更新失败原因。
export const installError = writable("");

/// 结果缓存有效期：避免用户反复开关弹窗时反复请求 GitHub（未认证限流 60 次/小时）。
const CHECK_TTL_MS = 5 * 60 * 1000;
let lastCheckAt = 0;

/// 读取本地版本（带缓存）。失败返回空串，由调用方决定如何提示。
export async function ensureVersion() {
  if (get(currentVersion)) return get(currentVersion);
  try {
    const v = await getVersion();
    currentVersion.set(String(v || ""));
  } catch {
    currentVersion.set("");
  }
  return get(currentVersion);
}

/**
 * 检查更新。
 *
 * @param {object}  [opts]
 * @param {boolean} [opts.force]  忽略 TTL 强制请求（用户点「重新检查」时用）
 * @param {boolean} [opts.silent] 静默模式：失败/无更新都不弹 toast（启动后台检查用）
 * @returns {Promise<object|null>} 成功后返回 UpdateInfo；失败返回 null
 */
export async function checkForUpdate({ force = false, silent = false } = {}) {
  if (get(updateChecking)) return get(updateInfo);
  if (!force && get(updateInfo) && Date.now() - lastCheckAt < CHECK_TTL_MS) {
    return get(updateInfo);
  }

  updateChecking.set(true);
  updateError.set("");
  try {
    const version = await ensureVersion();
    if (!version) throw new Error("无法读取当前版本号");

    const info = await invoke("check_update", { currentVersion: version });
    updateInfo.set(info);
    lastCheckAt = Date.now();

    if (!silent) {
      if (info?.has_update) pushToast(`发现新版本 v${info.latest_version}`, "ok");
      else pushToast("当前已是最新版本", "ok");
    }
    return info;
  } catch (e) {
    const message = typeof e === "string" ? e : e?.message ?? String(e ?? "未知错误");
    updateError.set(message);
    if (!silent) pushToast(`检查更新失败：${message}`, "error");
    return null;
  } finally {
    updateChecking.set(false);
  }
}

/// 订阅后端下载进度（只订阅一次）。
let progressListening = false;
export async function startUpdateProgress() {
  if (progressListening) return;
  progressListening = true;
  try {
    await listen("update-progress", (e) => {
      const p = e.payload ?? {};
      updateProgress.set({
        phase: p.phase ?? "",
        downloaded: p.downloaded ?? 0,
        total: p.total ?? 0,
        pct: p.pct ?? 0,
      });
      if (p.phase) installPhase.set(p.phase);
    });
  } catch {
    progressListening = false;
  }
}

/// 当前安装形态应使用的资产（与后端 Edition::asset_kind 对应）。
export function editionAsset(info) {
  if (!info) return null;
  const want =
    info.edition === "nsis" ? "nsis" : info.edition === "msi" ? "msi" : "portable";
  return (info.assets ?? []).find((a) => a.kind === want) ?? null;
}

/// 自动更新方式的说明文案。
export function installmentHint(info) {
  if (!info) return "";
  if (!info.can_self_install) {
    return "当前程序所在目录不可写，无法自动替换：请用下方链接手动下载新版。";
  }
  switch (info.edition) {
    case "portable":
      return "检测到便携版：将下载新版程序、原地替换当前 exe，完成后自动重启（不会安装到系统目录）。";
    case "nsis":
      return "检测到 NSIS 安装版：将下载安装包并静默安装，完成后自动重启。";
    case "msi":
      return "检测到 MSI 安装版：将下载安装包并静默安装；per-machine 安装需要你在 UAC 弹窗中确认管理员权限。";
    default:
      return "";
  }
}

/// 下载并自动安装更新。成功时进程会重启，Promise 通常不会 resolve。
export async function installUpdate(asset) {
  installError.set("");
  if (!asset?.url) {
    installError.set("当前版本没有可用的安装包，请前往 Releases 页面手动下载。");
    return false;
  }
  installPhase.set("preparing");
  updateProgress.set({ phase: "preparing", downloaded: 0, total: asset.size ?? 0, pct: 0 });
  await startUpdateProgress();
  try {
    await invoke("install_update", {
      url: asset.url,
      kind: asset.kind,
      digest: asset.digest ?? null,
      size: asset.size ?? 0,
    });
    // 理论上安装成功后进程会退出并重启；走到这里说明后端仍在运行。
    installPhase.set("installing");
    return true;
  } catch (e) {
    const message = typeof e === "string" ? e : e?.message ?? String(e ?? "未知错误");
    installError.set(message);
    installPhase.set("");
    pushToast(`自动更新失败：${message}`, "error");
    return false;
  }
}

/// 用系统默认浏览器打开链接（失败时退回 window.open）。
export async function openExternal(url) {
  if (!url) return false;
  try {
    await open(url);
    return true;
  } catch {
    try {
      window.open(url, "_blank");
      return true;
    } catch {
      return false;
    }
  }
}

/// 下载列表里优先推荐的资产：NSIS 安装包 > MSI > 便携版 > 其它。
export function preferredAsset(info) {
  const assets = info?.assets ?? [];
  if (!assets.length) return null;
  const rank = { nsis: 0, msi: 1, portable: 2, other: 3 };
  return [...assets].sort((a, b) => (rank[a.kind] ?? 9) - (rank[b.kind] ?? 9))[0];
}
