<script>
  import { onMount } from "svelte";
  import { metrics, cpuHistory, MAX_HISTORY, historyData, loadHistory, diskIo } from "../stores.js";

  $: cpu = $metrics?.cpu ?? 0;
  $: memLoad = $metrics?.mem_load ?? 0;
  $: memUsed = $metrics?.mem_used_gb ?? 0;
  $: memTotal = $metrics?.mem_total_gb ?? 0;
  $: memPageTotal = $metrics?.mem_page_total_gb ?? 0;
  $: memPageUsed = $metrics?.mem_page_used_gb ?? 0;
  $: diskRead = $metrics?.disk_read_bps ?? 0;
  $: diskWrite = $metrics?.disk_write_bps ?? 0;
  $: netTotal = $metrics?.net_total_bps ?? 0;
  // Aggregate per-process disk I/O for a more accurate total
  $: ioRows = $diskIo?.rows ?? [];
  $: totalDiskRead = ioRows.reduce((s, r) => s + (r.read_bps ?? 0), 0);
  $: totalDiskWrite = ioRows.reduce((s, r) => s + (r.write_bps ?? 0), 0);
  $: showDiskRead = totalDiskRead > 0 ? totalDiskRead : diskRead;
  $: showDiskWrite = totalDiskWrite > 0 ? totalDiskWrite : diskWrite;

  const sev = (v) => (v >= 85 ? "danger" : v >= 70 ? "warn" : "ok");
  const sevLabel = (v) => (v >= 85 ? "高负载" : v >= 70 ? "偏高" : "正常");

  // CPU sparkline (existing)
  $: spark = (() => {
    const h = $cpuHistory;
    if (h.length < 2) return "";
    const w = 320, ht = 70;
    const step = w / (MAX_HISTORY - 1);
    return h
      .map((v, i) => {
        const x = i * step;
        const y = ht - (Math.min(100, v) / 100) * ht;
        return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(" ");
  })();
  $: sparkArea = spark ? `${spark} L320,70 L0,70 Z` : "";

  // ===== History chart (interactive) =====
  const HW = 800, HH = 160; // viewBox 尺寸（等比，不再拉伸变形）

  // 可切换的指标系列
  const series = [
    { key: "cpu", label: "CPU", color: "#6366f1", unit: "%", max: () => 100,
      get: (d) => d.cpu ?? 0, fmt: (v) => v.toFixed(1) + "%" },
    { key: "mem", label: "内存", color: "#22c55e", unit: "%", max: () => 100,
      get: (d) => d.mem_load ?? 0, fmt: (v) => v.toFixed(0) + "%" },
    { key: "net", label: "网络", color: "#f59e0b", unit: "", max: (h) => Math.max(1, ...h.map((d) => (d.net_total_bps ?? 0) * 8)),
      get: (d) => (d.net_total_bps ?? 0) * 8, fmt: (v) => fmtBitsRaw(v) },
  ];
  let activeSeries = { cpu: true, mem: true, net: false };

  function toggleSeries(k) {
    activeSeries = { ...activeSeries, [k]: !activeSeries[k] };
  }

  // 每个系列的路径（按各自 max 归一化到 0..HH）
  $: seriesPaths = (() => {
    const h = $historyData;
    if (h.length < 2) return [];
    const step = HW / (h.length - 1);
    return series
      .filter((s) => activeSeries[s.key])
      .map((s) => {
        const mx = s.max(h);
        const line = h
          .map((d, i) => {
            const x = i * step;
            const y = HH - Math.min(1, s.get(d) / mx) * HH;
            return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
          })
          .join(" ");
        return { ...s, line, area: `${line} L${HW},${HH} L0,${HH} Z` };
      });
  })();

  // ——— hover 游标 ———
  let hoverIdx = null;
  let hoverX = 0;
  let chartEl;

  function onChartMove(e) {
    const h = $historyData;
    if (h.length < 2) return;
    const rect = chartEl.getBoundingClientRect();
    const rel = (e.clientX - rect.left) / rect.width; // 0..1
    const idx = Math.round(rel * (h.length - 1));
    hoverIdx = Math.max(0, Math.min(h.length - 1, idx));
    hoverX = (hoverIdx / (h.length - 1)) * 100; // 百分比，用于定位竖线/tooltip
  }
  function onChartLeave() {
    hoverIdx = null;
  }

  $: hoverSample = hoverIdx != null ? $historyData[hoverIdx] : null;

  const fmtHoverTime = (ts) => {
    if (!ts) return "";
    // ts 可能是 ISO 字符串
    const d = new Date(ts);
    if (isNaN(d.getTime())) return String(ts);
    return d.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
  };

  // 原始 bit/s 数值格式化（输入已是 bit/s）
  const fmtBitsRaw = (bps) => {
    if (bps < 1000) return bps.toFixed(0) + " bps";
    if (bps < 1e6) return (bps / 1e3).toFixed(1) + " Kbps";
    if (bps < 1e9) return (bps / 1e6).toFixed(1) + " Mbps";
    return (bps / 1e9).toFixed(1) + " Gbps";
  };

  // Byte/s formatting
  const fmtBytes = (v) => {
    if (v < 1024) return v.toFixed(1) + " B";
    if (v < 1024 * 1024) return (v / 1024).toFixed(1) + " KB";
    if (v < 1024 * 1024 * 1024) return (v / (1024 * 1024)).toFixed(1) + " MB";
    return (v / (1024 * 1024 * 1024)).toFixed(1) + " GB";
  };

  // Bits/s formatting (input in bytes/s, multiply by 8)
  const fmtBits = (v) => {
    const bps = v * 8;
    if (bps < 1000) return bps.toFixed(1) + " bps";
    if (bps < 1e6) return (bps / 1e3).toFixed(1) + " Kbps";
    if (bps < 1e9) return (bps / 1e6).toFixed(1) + " Mbps";
    return (bps / 1e9).toFixed(1) + " Gbps";
  };

  // Page file utilization
  $: pageFilePct = memPageTotal > 0 ? (memPageUsed / memPageTotal * 100) : 0;

  // Time ranges
  const ranges = [
    { label: "1分", n: 60 },
    { label: "5分", n: 300 },
    { label: "15分", n: 900 },
    { label: "1小时", n: 3600 },
  ];
  let activeRange = 60;

  function selectRange(n) {
    activeRange = n;
    hoverIdx = null;
    loadHistory(n);
  }

  onMount(() => {
    loadHistory(activeRange);
  });
</script>

<div class="grid">
  <!-- CPU 使用率 -->
  <article class="card">
    <div class="card-head">
      <span class="card-label">CPU 使用率</span>
      <span class="badge {sev(cpu)}">{sevLabel(cpu)}</span>
    </div>
    <div class="value">{cpu.toFixed(1)}<span class="unit">%</span></div>
    <svg class="spark" viewBox="0 0 320 70" preserveAspectRatio="none">
      {#if sparkArea}
        <path d={sparkArea} class="spark-area" />
        <path d={spark} class="spark-line" />
      {/if}
    </svg>
    <div class="card-foot">最近 {$cpuHistory.length}/{MAX_HISTORY} 个采样 · PDH _Total</div>
  </article>

  <!-- 内存占用 -->
  <article class="card">
    <div class="card-head">
      <span class="card-label">内存占用</span>
      <span class="badge {sev(memLoad)}">{memLoad}%</span>
    </div>
    <div class="value">{memUsed.toFixed(1)}<span class="unit">/ {memTotal.toFixed(1)} GB</span></div>
    <div class="bar">
      <div class="bar-fill {sev(memLoad)}" style="width:{memLoad}%"></div>
    </div>
    <div class="mem-page-row">
      <div>
        <span class="io-label">页面文件</span>
        <span class="io-value mono">{memPageUsed.toFixed(1)}<span class="unit-sm">/ {memPageTotal.toFixed(1)} GB</span></span>
      </div>
    </div>
    <div class="bar bar-sm">
      <div class="bar-fill {sev(pageFilePct)}" style="width:{pageFilePct}%"></div>
    </div>
    <div class="card-foot">GlobalMemoryStatusEx</div>
  </article>

  <!-- 磁盘 I/O -->
  <article class="card">
    <div class="card-head">
      <span class="card-label">磁盘 I/O</span>
    </div>
    <div class="io-row">
      <div>
        <span class="io-label">读取</span>
        <span class="io-value mono">{fmtBytes(showDiskRead)}/s</span>
      </div>
      <div>
        <span class="io-label">写入</span>
        <span class="io-value mono">{fmtBytes(showDiskWrite)}/s</span>
      </div>
    </div>
    <div class="card-foot">PDH PhysicalDisk(_Total)</div>
  </article>

  <!-- 网络吞吐 -->
  <article class="card">
    <div class="card-head">
      <span class="card-label">网络吞吐</span>
    </div>
    <div class="value-sm mono">{fmtBits(netTotal)}/s</div>
    <div class="bar">
      <div class="bar-fill ok" style="width:{Math.min(100, netTotal / 1e8 * 100)}%"></div>
    </div>
    <div class="card-foot">GetIfTable</div>
  </article>

  <!-- 每进程磁盘 I/O TOP 5 -->
  <article class="card">
    <div class="card-head">
      <span class="card-label">进程磁盘 I/O TOP 5</span>
    </div>
    {#if $diskIo?.rows?.length > 0}
      <div class="io-proc-list">
        {#each $diskIo.rows.slice(0, 5) as row}
          <div class="io-proc-row">
            <span class="io-proc-name" title="{row.name} ({row.pid})">{row.name}</span>
            <span class="io-proc-r">
              <span class="io-label-r">R</span> {fmtBytes(row.read_bps)}/s
            </span>
            <span class="io-proc-w">
              <span class="io-label-w">W</span> {fmtBytes(row.write_bps)}/s
            </span>
          </div>
        {/each}
      </div>
    {:else}
      <div class="io-empty">等待数据…</div>
    {/if}
    <div class="card-foot">GetProcessIoCounters · 每 2 秒</div>
  </article>

  <!-- 系统负载概览 -->
  <article class="card">
    <div class="card-head">
      <span class="card-label">系统负载</span>
    </div>
    <div class="io-row">
      <div>
        <span class="io-label">CPU</span>
        <span class="io-value mono">{cpu.toFixed(1)}%</span>
      </div>
      <div>
        <span class="io-label">内存</span>
        <span class="io-value mono">{memUsed.toFixed(1)} GB</span>
      </div>
    </div>
    <div class="io-row">
      <div>
        <span class="io-label">页面文件</span>
        <span class="io-value mono">{memPageUsed.toFixed(1)} GB</span>
      </div>
      <div>
        <span class="io-label">负载状态</span>
        <span class="badge {sev(Math.max(cpu, memLoad))}">{sevLabel(Math.max(cpu, memLoad))}</span>
      </div>
    </div>
    <div class="card-foot">综合指标</div>
  </article>
</div>

<!-- 历史趋势 -->
{#if $historyData.length > 0}
<section class="history-section">
  <div class="history-head">
    <h3 class="section-title">历史趋势</h3>
    <div class="history-controls">
      <div class="series-group">
        {#each series as s}
          <button
            class="series-btn"
            class:active={activeSeries[s.key]}
            style="--sc:{s.color}"
            on:click={() => toggleSeries(s.key)}
          >
            <span class="series-dot"></span>{s.label}
          </button>
        {/each}
      </div>
      <div class="range-group">
        {#each ranges as r}
          <button
            class="range-btn"
            class:active={activeRange === r.n}
            on:click={() => selectRange(r.n)}
          >{r.label}</button>
        {/each}
      </div>
    </div>
  </div>
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <div
    class="history-chart"
    bind:this={chartEl}
    on:mousemove={onChartMove}
    on:mouseleave={onChartLeave}
  >
    <svg viewBox="0 0 {HW} {HH}" preserveAspectRatio="none" class="history-svg">
      <!-- 水平网格线 25/50/75% -->
      {#each [0.25, 0.5, 0.75] as g}
        <line class="grid-line" x1="0" y1={HH * g} x2={HW} y2={HH * g} />
      {/each}
      {#each seriesPaths as sp}
        <path d={sp.area} fill={sp.color} fill-opacity="0.1" stroke="none" />
        <path d={sp.line} fill="none" stroke={sp.color} stroke-width="2" vector-effect="non-scaling-stroke" />
      {/each}
    </svg>

    {#if hoverIdx != null}
      <div class="cursor-line" style="left:{hoverX}%"></div>
    {/if}

    <div class="chart-labels">
      <span>0%</span>
      <span>50%</span>
      <span>100%</span>
    </div>

    {#if hoverSample}
      <div
        class="hover-tip"
        class:flip={hoverX > 60}
        style="left:{hoverX}%"
      >
        <div class="tip-time mono">{fmtHoverTime(hoverSample.ts)}</div>
        {#each series as s}
          {#if activeSeries[s.key]}
            <div class="tip-row">
              <span class="tip-dot" style="background:{s.color}"></span>
              <span class="tip-label">{s.label}</span>
              <span class="tip-val mono">{s.fmt(s.get(hoverSample))}</span>
            </div>
          {/if}
        {/each}
      </div>
    {/if}
  </div>
</section>
{/if}

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
    gap: var(--sp-4);
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: var(--sp-6);
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
  .card-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .card-label {
    font-size: 13px;
    color: var(--text-muted);
  }
  .value {
    font-family: var(--font-mono);
    font-size: 40px;
    font-weight: 600;
    line-height: 1;
    font-variant-numeric: tabular-nums;
  }
  .value-sm {
    font-family: var(--font-mono);
    font-size: 28px;
    font-weight: 600;
    line-height: 1;
    font-variant-numeric: tabular-nums;
  }
  .unit {
    font-size: 15px;
    color: var(--text-muted);
    margin-left: var(--sp-2);
    font-weight: 400;
  }
  .unit-sm {
    font-size: 12px;
    color: var(--text-muted);
    margin-left: var(--sp-1);
    font-weight: 400;
  }
  .badge {
    font-size: 11px;
    padding: 3px 10px;
    border-radius: 999px;
    border: 1px solid transparent;
    font-variant-numeric: tabular-nums;
  }
  .badge.ok {
    color: var(--ok);
    border-color: rgba(34, 197, 94, 0.4);
    background: rgba(34, 197, 94, 0.12);
  }
  .badge.warn {
    color: var(--warn);
    border-color: rgba(245, 158, 11, 0.4);
    background: rgba(245, 158, 11, 0.12);
  }
  .badge.danger {
    color: var(--danger);
    border-color: rgba(239, 68, 68, 0.4);
    background: rgba(239, 68, 68, 0.12);
  }
  .spark {
    width: 100%;
    height: 70px;
    display: block;
  }
  .spark-line {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2;
    vector-effect: non-scaling-stroke;
  }
  .spark-area {
    fill: rgba(99, 102, 241, 0.15);
    stroke: none;
  }
  .bar {
    height: 10px;
    background: var(--surface-2);
    border-radius: 999px;
    overflow: hidden;
  }
  .bar-sm {
    height: 6px;
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
  .io-row {
    display: flex;
    gap: var(--sp-6);
    align-items: baseline;
  }
  .io-label {
    font-size: 11px;
    color: var(--text-muted);
    display: block;
    margin-bottom: 2px;
  }
  .io-value {
    font-variant-numeric: tabular-nums;
  }
  .mono {
    font-family: var(--font-mono);
  }
  .card-foot {
    font-size: 11px;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }
  .io-proc-list {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .io-proc-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: 12px;
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }
  .io-proc-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
  }
  .io-proc-r {
    color: var(--ok);
    min-width: 90px;
    text-align: right;
  }
  .io-proc-w {
    color: var(--warn);
    min-width: 90px;
    text-align: right;
  }
  .io-label-r {
    font-size: 10px;
    color: var(--ok);
    opacity: 0.7;
  }
  .io-label-w {
    font-size: 10px;
    color: var(--warn);
    opacity: 0.7;
  }
  .io-empty {
    font-size: 13px;
    color: var(--text-muted);
    padding: var(--sp-3) 0;
  }
  .mem-page-row {
    display: flex;
    gap: var(--sp-4);
  }

  /* History section */
  .history-section {
    margin-top: var(--sp-6);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: var(--sp-6);
  }
  .history-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: var(--sp-4);
  }
  .section-title {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
  }
  .history-controls {
    display: flex;
    align-items: center;
    gap: var(--sp-4);
  }
  .series-group {
    display: flex;
    gap: var(--sp-1);
  }
  .series-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text-muted);
    font-family: inherit;
    font-size: 12px;
    padding: 4px 10px;
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: all 0.15s;
  }
  .series-btn .series-dot {
    width: 8px;
    height: 8px;
    border-radius: 999px;
    background: var(--sc);
    opacity: 0.35;
    transition: opacity 0.15s;
  }
  .series-btn.active {
    color: var(--text);
    border-color: var(--sc);
    background: color-mix(in srgb, var(--sc) 12%, transparent);
  }
  .series-btn.active .series-dot {
    opacity: 1;
  }
  .range-group {
    display: flex;
    gap: var(--sp-2);
  }
  .range-btn {
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--text-muted);
    font-size: 12px;
    padding: 4px 12px;
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: all 0.15s;
  }
  .range-btn:hover {
    background: var(--border);
    color: var(--text);
  }
  .range-btn.active {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }
  .history-chart {
    position: relative;
  }
  .history-svg {
    width: 100%;
    height: 160px;
    display: block;
    cursor: crosshair;
  }
  .grid-line {
    stroke: var(--border);
    stroke-width: 1;
    stroke-dasharray: 3 4;
    vector-effect: non-scaling-stroke;
    opacity: 0.5;
  }
  .cursor-line {
    position: absolute;
    top: 0;
    bottom: 18px;
    width: 1px;
    background: var(--text-muted);
    opacity: 0.5;
    pointer-events: none;
    transform: translateX(-0.5px);
  }
  .hover-tip {
    position: absolute;
    top: 8px;
    transform: translateX(10px);
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 8px 10px;
    font-size: 12px;
    pointer-events: none;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.35);
    min-width: 120px;
    z-index: 2;
  }
  .hover-tip.flip {
    transform: translateX(calc(-100% - 10px));
  }
  .tip-time {
    font-size: 11px;
    color: var(--text-muted);
    margin-bottom: 5px;
  }
  .tip-row {
    display: flex;
    align-items: center;
    gap: 6px;
    line-height: 1.7;
  }
  .tip-dot {
    width: 8px;
    height: 8px;
    border-radius: 999px;
    flex-shrink: 0;
  }
  .tip-label {
    color: var(--text-muted);
  }
  .tip-val {
    margin-left: auto;
    font-variant-numeric: tabular-nums;
  }
  .chart-labels {
    display: flex;
    justify-content: space-between;
    font-size: 10px;
    color: var(--text-muted);
    font-family: var(--font-mono);
    margin-top: var(--sp-1);
  }
</style>
