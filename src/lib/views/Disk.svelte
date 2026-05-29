<script>
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { pushToast } from "../stores.js";

  let volumes = [];
  let disks = [];
  let smartNote = "";
  let loading = true;
  let timer;

  function fmt(bytes) {
    const u = ["B", "KB", "MB", "GB", "TB"];
    let i = 0;
    let v = bytes;
    while (v >= 1024 && i < u.length - 1) {
      v /= 1024;
      i++;
    }
    return `${v.toFixed(i >= 3 ? 1 : 0)} ${u[i]}`;
  }

  async function refresh() {
    try {
      const r = await invoke("get_disk_report");
      volumes = r.volumes;
      disks = r.disks;
      smartNote = r.smart_note;
      loading = false;
    } catch (e) {
      pushToast("加载磁盘信息失败：" + e, "error");
    }
  }

  onMount(() => {
    refresh();
    timer = setInterval(refresh, 15000);
  });
  onDestroy(() => clearInterval(timer));

  const sev = (v) => (v >= 90 ? "danger" : v >= 75 ? "warn" : "ok");
</script>

<div class="head">
  <h2 class="section-title">分区使用率</h2>
  <button class="ghost" on:click={refresh}>刷新</button>
</div>

{#if loading}
  <p class="muted">加载中…</p>
{:else}
  <div class="vol-grid">
    {#each volumes as v}
      <article class="card">
        <div class="card-head">
          <span class="vol-name">
            <strong>{v.drive}</strong>
            <span class="vol-label">{v.label}</span>
          </span>
          <span class="pct {sev(v.used_pct)}">{v.used_pct.toFixed(0)}%</span>
        </div>
        <div class="bar">
          <div class="bar-fill {sev(v.used_pct)}" style="width:{Math.min(100, v.used_pct)}%"></div>
        </div>
        <div class="vol-meta">
          <span>{v.fs} · {v.drive_type}</span>
          <span>可用 {fmt(v.free)} / 共 {fmt(v.total)}</span>
        </div>
      </article>
    {/each}
  </div>

  <div class="head" style="margin-top: var(--sp-8)">
    <h2 class="section-title">物理磁盘健康</h2>
  </div>

  {#if smartNote}
    <div class="note-box warn">{smartNote}</div>
  {:else if disks.length === 0}
    <p class="muted">未检测到物理磁盘。</p>
  {:else}
    <div class="disk-list">
      {#each disks as d}
        <article class="card disk-row">
          <div class="disk-main">
            <span class="health {d.healthy ? 'ok' : 'bad'}" title={d.status}></span>
            <div>
              <div class="disk-model">{d.model || "未知型号"}</div>
              <div class="disk-sub">
                {d.media || "—"} · {d.interface || "—"}{d.serial ? ` · SN ${d.serial}` : ""}
              </div>
            </div>
          </div>
          <div class="disk-right">
            {#if d.temperature != null}
              <span class="temp">{d.temperature}°C</span>
            {/if}
            <span class="disk-size">{d.size > 0 ? fmt(d.size) : "—"}</span>
            <span class="status-badge {d.healthy ? 'ok' : 'bad'}">{d.status}</span>
          </div>
        </article>
      {/each}
    </div>
    <p class="muted small">
      健康状态来自 Win32_DiskDrive（SMART 预测：OK / Degraded / Pred Fail）。
      温度来自 MSFT_StorageReliabilityCounter，需管理员权限且多为 NVMe 支持——未显示温度即表示未提权或该盘不支持。
    </p>
  {/if}
{/if}

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: var(--sp-4);
  }
  .section-title {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
  }
  .muted {
    color: var(--text-muted);
    font-size: 13px;
  }
  .muted.small {
    font-size: 11px;
    margin-top: var(--sp-3);
  }

  .vol-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: var(--sp-4);
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: var(--sp-4);
  }
  .card-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: var(--sp-3);
  }
  .vol-name strong {
    font-size: 16px;
    font-family: var(--font-mono);
  }
  .vol-label {
    color: var(--text-muted);
    font-size: 13px;
    margin-left: 8px;
  }
  .pct {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    font-weight: 600;
  }
  .pct.ok {
    color: var(--text);
  }
  .pct.warn {
    color: var(--warn);
  }
  .pct.danger {
    color: var(--danger);
  }
  .bar {
    height: 10px;
    background: var(--surface-2);
    border-radius: 999px;
    overflow: hidden;
  }
  .bar-fill {
    height: 100%;
    border-radius: 999px;
    transition: width 0.4s ease;
  }
  .bar-fill.ok {
    background: linear-gradient(90deg, var(--accent), var(--ok));
  }
  .bar-fill.warn {
    background: linear-gradient(90deg, var(--warn), #f97316);
  }
  .bar-fill.danger {
    background: linear-gradient(90deg, var(--danger), #dc2626);
  }
  .vol-meta {
    display: flex;
    justify-content: space-between;
    margin-top: var(--sp-3);
    font-size: 12px;
    color: var(--text-muted);
  }

  .disk-list {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .disk-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: var(--sp-3) var(--sp-4);
  }
  .disk-main {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
  }
  .health {
    width: 10px;
    height: 10px;
    border-radius: 999px;
    flex-shrink: 0;
  }
  .health.ok {
    background: var(--ok);
    box-shadow: 0 0 8px rgba(34, 197, 94, 0.6);
  }
  .health.bad {
    background: var(--danger);
    box-shadow: 0 0 8px rgba(239, 68, 68, 0.6);
  }
  .disk-model {
    font-size: 14px;
    font-weight: 500;
  }
  .disk-sub {
    font-size: 12px;
    color: var(--text-muted);
    font-family: var(--font-mono);
    margin-top: 2px;
  }
  .disk-right {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
  }
  .disk-size {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    color: var(--text);
  }
  .temp {
    font-family: var(--font-mono);
    font-size: 12px;
    padding: 3px 8px;
    border-radius: 999px;
    color: #93c5fd;
    background: rgba(59, 130, 246, 0.14);
  }
  .status-badge {
    font-size: 11px;
    padding: 3px 10px;
    border-radius: 999px;
  }
  .status-badge.ok {
    color: var(--ok);
    background: rgba(34, 197, 94, 0.12);
  }
  .status-badge.bad {
    color: var(--danger);
    background: rgba(239, 68, 68, 0.12);
  }
  .note-box {
    font-size: 12px;
    line-height: 1.6;
    padding: 10px 12px;
    border-radius: 8px;
  }
  .note-box.warn {
    color: #fcd34d;
    background: rgba(245, 158, 11, 0.1);
    border: 1px solid rgba(245, 158, 11, 0.35);
  }
  .ghost {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
    padding: 6px 14px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .ghost:hover {
    background: var(--surface-2);
  }
</style>
