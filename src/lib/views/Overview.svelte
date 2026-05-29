<script>
  import { metrics, cpuHistory, MAX_HISTORY } from "../stores.js";

  $: cpu = $metrics?.cpu ?? 0;
  $: memLoad = $metrics?.mem_load ?? 0;
  $: memUsed = $metrics?.mem_used_gb ?? 0;
  $: memTotal = $metrics?.mem_total_gb ?? 0;

  const sev = (v) => (v >= 85 ? "danger" : v >= 70 ? "warn" : "ok");
  const sevLabel = (v) => (v >= 85 ? "高负载" : v >= 70 ? "偏高" : "正常");

  $: spark = (() => {
    const h = $cpuHistory;
    if (h.length < 2) return "";
    const w = 320,
      ht = 70;
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
</script>

<div class="grid">
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

  <article class="card">
    <div class="card-head">
      <span class="card-label">内存占用</span>
      <span class="badge {sev(memLoad)}">{memLoad}%</span>
    </div>
    <div class="value">{memUsed.toFixed(1)}<span class="unit">/ {memTotal.toFixed(1)} GB</span></div>
    <div class="bar">
      <div class="bar-fill {sev(memLoad)}" style="width:{memLoad}%"></div>
    </div>
    <div class="card-foot">GlobalMemoryStatusEx</div>
  </article>
</div>

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
  .unit {
    font-size: 15px;
    color: var(--text-muted);
    margin-left: var(--sp-2);
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
  .card-foot {
    font-size: 11px;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }
</style>
