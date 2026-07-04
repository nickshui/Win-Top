<script>
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { pushToast } from "../stores.js";

  let tab = "overview";

  // ===== Volume + Disk (existing) =====
  let volumes = [];
  let disks = [];
  let smartNote = "";
  let loading = true;
  let timer;

  function fmt(bytes) {
    if (!bytes) return "0 B";
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

  // ===== Space Analyzer =====
  let scanDrive = "";
  let scanLoading = false;
  let scanResult = null;
  let scanError = null;

  // 默认选中系统盘（或第一个固定盘）
  $: if (!scanDrive && volumes.length > 0) {
    const sys = volumes.find((v) => v.drive?.toUpperCase().startsWith("C")) ?? volumes[0];
    scanDrive = sys.drive;
  }

  async function scanVolume(drive) {
    scanDrive = drive;
    scanLoading = true;
    scanResult = null;
    scanError = null;
    try {
      scanResult = await invoke("scan_volume", { drive, topN: 50 });
      if (scanResult.errors > 0) {
        pushToast(`扫描完成，${scanResult.errors} 个文件/目录无法访问`, "warn");
      }
    } catch (e) {
      scanError = String(e);
      pushToast("扫描失败：" + e, "error");
    } finally {
      scanLoading = false;
    }
  }

  const srcLabel = (s) => (s === "mft" ? "MFT 极速扫描" : "逐目录扫描");

  const fmtTime = (secs) => {
    if (!secs) return "-";
    const d = new Date(secs * 1000);
    return d.toLocaleDateString("zh-CN") + " " + d.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
  };

  const fmtSize = (n) => {
    if (!n) return "0 B";
    if (n < 1024) return n + " B";
    if (n < 1024 * 1024) return (n / 1024).toFixed(1) + " KB";
    if (n < 1024 * 1024 * 1024) return (n / 1024 / 1024).toFixed(1) + " MB";
    return (n / 1024 / 1024 / 1024).toFixed(2) + " GB";
  };

  const fmtCount = (n) => {
    if (!n) return "0";
    if (n < 1000) return String(n);
    if (n < 1000000) return (n / 1000).toFixed(1) + "K";
    return (n / 1000000).toFixed(1) + "M";
  };

  // Max bar width calculation
  $: maxDirSize = scanResult?.dirs?.[0]?.total_size ?? 1;
</script>

<div class="tabs" role="tablist">
  <button class="tab" class:active={tab === "overview"} on:click={() => (tab = "overview")} role="tab" aria-selected={tab === "overview"}>磁盘概况</button>
  <button class="tab" class:active={tab === "analyzer"} on:click={() => (tab = "analyzer")} role="tab" aria-selected={tab === "analyzer"}>空间分析</button>
</div>

{#if tab === "overview"}
  <!-- ===== Existing disk overview ===== -->
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
{:else}
  <!-- ===== Space Analyzer tab ===== -->
  <section class="analyzer">
    <div class="drive-picker">
      {#each volumes as v}
        <button
          class="drive-chip"
          class:active={scanDrive === v.drive}
          on:click={() => scanVolume(v.drive)}
          disabled={scanLoading}
        >
          <span class="dc-drive mono">{v.drive}</span>
          <span class="dc-meta">{fmt(v.free)} 可用 / {fmt(v.total)}</span>
        </button>
      {/each}
    </div>

    {#if scanLoading}
      <p class="scan-status">正在扫描 {scanDrive} 整盘，首次读取 MFT 需管理员权限…</p>
    {/if}

    {#if scanError}
      <div class="note">{scanError}</div>
    {/if}

    {#if scanResult}
      <div class="scan-summary">
        <div class="sum-item"><span class="sum-val mono">{fmtCount(scanResult.scanned)}</span><span class="sum-label">已扫描文件</span></div>
        <div class="sum-item"><span class="sum-val mono">{(scanResult.elapsed_ms / 1000).toFixed(2)}s</span><span class="sum-label">耗时</span></div>
        <div class="sum-item"><span class="sum-val src-tag" class:mft={scanResult.source === "mft"}>{srcLabel(scanResult.source)}</span><span class="sum-label">扫描方式</span></div>
        {#if scanResult.errors > 0}
          <div class="sum-item"><span class="sum-val mono danger-text">{scanResult.errors}</span><span class="sum-label">无法访问</span></div>
        {/if}
      </div>

      {#if scanResult.dirs.length > 0}
        <section class="dir-section">
          <h4 class="section-subtitle">目录分布</h4>
          <div class="dir-list">
            {#each scanResult.dirs.slice(0, 20) as dir (dir.path)}
              <div class="dir-row">
                <span class="dir-path mono" title={dir.path}>{dir.path || "(根目录)"}</span>
                <span class="dir-size mono">{fmtSize(dir.total_size)}</span>
                <span class="dir-count mono">{fmtCount(dir.file_count)} 文件</span>
                <div class="dir-bar-track">
                  <div class="dir-bar-fill" style="width:{(dir.total_size / maxDirSize * 100).toFixed(1)}%"></div>
                </div>
              </div>
            {/each}
          </div>
        </section>
      {/if}

      {#if scanResult.large_files.length > 0}
        <section class="files-section">
          <h4 class="section-subtitle">最大文件 TOP {scanResult.large_files.length}</h4>
          <div class="table-wrap small-table">
            <table>
              <thead>
                <tr><th>文件路径</th><th class="num">大小</th><th class="num">修改时间</th></tr>
              </thead>
              <tbody>
                {#each scanResult.large_files as f (f.path)}
                  <tr>
                    <td class="file-path mono" title={f.path}>{f.path}</td>
                    <td class="num mono">{fmtSize(f.size)}</td>
                    <td class="num mono">{fmtTime(f.modified_secs)}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </section>
      {/if}
    {/if}
  </section>
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

  /* ===== Tabs ===== */
  .tabs {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    margin-bottom: var(--sp-4);
    border-bottom: 1px solid var(--border);
  }
  .tab {
    border: none;
    background: transparent;
    color: var(--text-muted);
    font-family: inherit;
    font-size: 14px;
    font-weight: 500;
    padding: 10px 16px;
    cursor: pointer;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
  }
  .tab:hover {
    color: var(--text);
  }
  .tab.active {
    color: var(--text);
    border-bottom-color: var(--accent);
  }

  /* ===== Space Analyzer styles ===== */
  .mono {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }
  .drive-picker {
    display: flex;
    gap: var(--sp-3);
    flex-wrap: wrap;
    margin-bottom: var(--sp-4);
  }
  .drive-chip {
    display: flex;
    flex-direction: column;
    gap: 2px;
    align-items: flex-start;
    min-width: 140px;
    padding: 10px 16px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--surface);
    color: var(--text);
    font-family: inherit;
    cursor: pointer;
    transition: border-color 0.15s ease, background 0.15s ease;
  }
  .drive-chip:hover:not(:disabled) {
    border-color: var(--accent);
  }
  .drive-chip.active {
    border-color: var(--accent);
    background: rgba(99, 102, 241, 0.12);
  }
  .drive-chip:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .dc-drive {
    font-size: 16px;
    font-weight: 700;
  }
  .dc-meta {
    font-size: 11px;
    color: var(--text-muted);
  }
  .scan-status {
    color: var(--text-muted);
    font-size: 14px;
    padding: var(--sp-6);
    text-align: center;
  }
  .note {
    padding: var(--sp-3);
    margin-bottom: var(--sp-4);
    border: 1px solid rgba(245, 158, 11, 0.35);
    background: rgba(245, 158, 11, 0.1);
    border-radius: var(--radius-sm);
    font-size: 13px;
    color: #fcd34d;
  }

  .scan-summary {
    display: flex;
    gap: var(--sp-8);
    margin-bottom: var(--sp-6);
  }
  .sum-item {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .sum-val {
    font-size: 28px;
    font-weight: 700;
  }
  .src-tag {
    font-size: 15px;
    font-weight: 600;
    color: var(--text-muted);
    align-self: flex-start;
    padding: 4px 10px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
  }
  .src-tag.mft {
    color: var(--ok);
    background: rgba(34, 197, 94, 0.12);
  }
  .sum-label {
    font-size: 12px;
    color: var(--text-muted);
  }
  .danger-text {
    color: var(--danger);
  }

  .section-subtitle {
    font-size: 14px;
    font-weight: 600;
    margin: 0 0 var(--sp-3);
  }
  .dir-section {
    margin-bottom: var(--sp-6);
  }
  .dir-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .dir-row {
    display: grid;
    grid-template-columns: 1fr 100px 80px 200px;
    align-items: center;
    gap: var(--sp-3);
    padding: 6px 0;
    font-size: 13px;
  }
  .dir-path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dir-size {
    text-align: right;
  }
  .dir-count {
    text-align: right;
    color: var(--text-muted);
  }
  .dir-bar-track {
    height: 8px;
    background: var(--surface-2);
    border-radius: 999px;
    overflow: hidden;
  }
  .dir-bar-fill {
    height: 100%;
    border-radius: 999px;
    background: var(--accent);
  }

  .small-table tbody {
    max-height: 400px;
  }
  .file-path {
    max-width: 0;
  }

  .table-wrap {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--surface);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }
  thead th {
    position: sticky;
    top: 0;
    background: var(--surface-2);
    text-align: left;
    padding: 8px 14px;
    font-weight: 500;
    color: var(--text-muted);
  }
  th.num {
    text-align: right;
  }
  tbody {
    display: block;
    max-height: 500px;
    overflow-y: auto;
  }
  thead, tbody tr {
    display: table;
    width: 100%;
    table-layout: fixed;
  }
  td {
    padding: 6px 14px;
    border-top: 1px solid var(--border);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  td.num {
    text-align: right;
  }
  tbody tr:hover {
    background: var(--surface-2);
  }
</style>
