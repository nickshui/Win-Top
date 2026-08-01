<script>
  import { createEventDispatcher } from "svelte";
  import FootprintAnalysis from "./FootprintAnalysis.svelte";
  import ResidueReview from "./ResidueReview.svelte";

  export let workflow;
  export let selectedPaths = new Set();
  export let selectedSize = 0;
  export let statusNote = "";
  export let fmtBytes = (value) => `${value} B`;
  export let fmtKb = (value) => `${value} KB`;

  const dispatch = createEventDispatcher();
  const forwardOpen = (event) => dispatch("openArtifact", event.detail);
  const scoreClass = (value) => (value >= 70 ? "danger" : value >= 40 ? "warn" : "ok");

  $: app = workflow?.app;
  $: entry = app?.entry;
  $: score = app?.score || {};
  $: footprint = workflow?.footprint;
</script>

{#if !app}
  <div class="empty-state">
    <span class="empty-mark" aria-hidden="true"></span>
    <h3>选择一个应用查看详细情况</h3>
    <p>选择后会自动扫描程序文件、应用数据和系统关联项。</p>
  </div>
{:else}
  <header class="detail-head">
    <div class="detail-title">
      <h3>{entry.name}</h3>
      <p>
        {entry.publisher || "未知发行商"} · v{entry.version || "—"} · {fmtKb(entry.estimated_size_kb)}
        {#if score.run_count > 0} · 运行过 {score.run_count} 次{/if}
      </p>
    </div>
    <div class="detail-score">
      <span class="score-pill {scoreClass(score.removal_recommendation)}">{score.removal_recommendation}</span>
      <span>推荐卸载度</span>
    </div>
  </header>

  {#if score.reasons?.length}
    <div class="reasons" aria-label="推荐依据">
      {#each score.reasons as reason}<span>{reason}</span>{/each}
    </div>
  {/if}

  {#if workflow.phase === "analyzing"}
    <div class="scan-status" role="status">
      <span class="spinner" aria-hidden="true"></span>
      <div><strong>{footprint ? "正在更新应用分析" : "正在分析应用"}</strong><p>正在统计可访问的文件、应用数据和系统关联项。</p></div>
    </div>
    {#if footprint}<FootprintAnalysis {footprint} {fmtBytes} on:open={forwardOpen} />{/if}
  {:else if workflow.phase === "overview"}
    <div class="actions">
      <button class="ghost" on:click={() => dispatch("requestUninstall")}>运行卸载程序</button>
      <button class="ghost" on:click={() => dispatch("rescan")}>重新扫描</button>
    </div>
    <FootprintAnalysis {footprint} {fmtBytes} on:open={forwardOpen} />
  {:else if workflow.phase === "preparing"}
    <div class="scan-status" role="status"><span class="spinner" aria-hidden="true"></span><div><strong>正在准备卸载</strong><p>正在确认本次分析结果并保存卸载前基线。</p></div></div>
  {:else if workflow.phase === "launching"}
    <div class="scan-status" role="status"><span class="spinner" aria-hidden="true"></span><div><strong>正在启动厂商卸载器</strong><p>Windows 或厂商程序可能会显示确认窗口。</p></div></div>
  {:else if workflow.phase === "waiting"}
    <div class="status-card">
      <strong>等待厂商卸载完成</strong>
      <p>请在厂商卸载窗口中完成操作。卸载器已启动不代表应用已经卸载。</p>
      {#if statusNote}<p class="status-note">{statusNote}</p>{/if}
      <div class="status-actions">
        <button class="primary" on:click={() => dispatch("check")}>检查卸载结果</button>
        <button class="ghost" on:click={() => dispatch("cancelWait")}>我已取消卸载</button>
      </div>
      <p class="cancel-hint">如果已关闭或取消厂商卸载，请返回应用分析。</p>
    </div>
  {:else if workflow.phase === "scanning"}
    <div class="scan-status" role="status"><span class="spinner" aria-hidden="true"></span><div><strong>正在检查卸载结果</strong><p>应用登记项已移除，正在重新检查卸载前记录的产物。</p></div></div>
  {:else if workflow.phase === "review"}
    <div class="removed-banner"><strong>厂商卸载已完成</strong><span>下面仅显示卸载后仍然存在的项目。</span></div>
    <ResidueReview artifacts={footprint?.artifacts || []} {selectedPaths} {fmtBytes} on:toggle={(event) => dispatch("toggle", event.detail)} />
    <div class="cleanup-actions">
      <span>已选 <strong>{selectedPaths.size}</strong> 项 · {fmtBytes(selectedSize)}</span>
      <button class="danger-action" disabled={!selectedPaths.size} on:click={() => dispatch("clean")}>清理选中残留</button>
    </div>
  {:else if workflow.phase === "cleaning"}
    <div class="scan-status" role="status"><span class="spinner danger-spinner" aria-hidden="true"></span><div><strong>正在处理选中残留</strong><p>目录正在移入隔离区，注册表项会先导出备份。</p></div></div>
  {:else if workflow.phase === "complete"}
    <div class="result-card">
      <span class="result-mark" aria-hidden="true"></span>
      <div><strong>清理完成</strong><p>处理 {workflow.result?.removed || 0} 项，释放 {fmtBytes(workflow.result?.freed_bytes || 0)}{workflow.result?.skipped ? `，跳过 ${workflow.result.skipped} 项` : ""}。</p></div>
    </div>
    {#if workflow.result?.warnings?.length}<ul class="warnings">{#each workflow.result.warnings as warning}<li>{warning}</li>{/each}</ul>{/if}
    <div class="result-actions">
      {#if workflow.result?.undo_token}<button class="ghost" on:click={() => dispatch("undo")}>撤销本次清理</button>{/if}
      <button class="primary" on:click={() => dispatch("finish")}>完成</button>
    </div>
  {:else if workflow.phase === "error"}
    <div class="error-card"><span aria-hidden="true">!</span><div><strong>操作未完成</strong><p role="alert">{workflow.error}</p></div></div>
    <button class="ghost retry" on:click={() => dispatch("retry")}>重试</button>
  {/if}
{/if}

<style>
  .empty-state { min-height: 100%; display: grid; place-content: center; justify-items: center; padding: 40px; color: var(--text-muted); text-align: center; }
  .empty-mark { width: 48px; height: 48px; margin-bottom: 12px; border: 1px solid var(--border); border-radius: 14px; background: linear-gradient(135deg, rgba(99,102,241,.12), rgba(51,65,85,.18)); }
  .empty-state h3 { margin: 0 0 6px; color: var(--text); font-size: 15px; }
  .empty-state p { margin: 0; font-size: 12px; }
  .detail-head { display: flex; justify-content: space-between; align-items: flex-start; gap: 16px; }
  .detail-title h3 { margin: 0; font-size: 18px; }
  .detail-title p { margin: 4px 0 0; color: var(--text-muted); font-size: 11px; }
  .detail-score { display: flex; flex-direction: column; align-items: center; gap: 2px; flex: 0 0 auto; color: var(--text-muted); font-size: 10px; }
  .score-pill { min-width: 46px; padding: 6px 10px; border-radius: var(--radius-sm); text-align: center; font: 700 20px var(--font-mono); }
  .score-pill.ok { color: var(--ok); background: rgba(34,197,94,.12); }
  .score-pill.warn { color: var(--warn); background: rgba(245,158,11,.14); }
  .score-pill.danger { color: var(--danger); background: rgba(239,68,68,.14); }
  .reasons { display: flex; flex-wrap: wrap; gap: 6px; margin: 14px 0 8px; }
  .reasons span { padding: 3px 9px; border: 1px solid var(--border); border-radius: 999px; background: var(--surface-2); font-size: 10px; }
  .actions { display: flex; gap: 8px; margin-top: 14px; }
  button { min-height: 38px; padding: 7px 14px; border-radius: var(--radius-sm); font-family: inherit; font-size: 12px; cursor: pointer; }
  button:focus-visible { outline: 2px solid #a5b4fc; outline-offset: 2px; }
  button:disabled { opacity: .55; cursor: default; }
  .ghost { border: 1px solid var(--border); background: transparent; color: var(--text); }
  .ghost:hover { background: var(--surface-2); }
  .primary { border: none; background: linear-gradient(135deg, var(--accent), #7c3aed); color: white; }
  .scan-status, .status-card, .result-card, .error-card { margin-top: 16px; padding: 16px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: rgba(15,23,42,.28); }
  .scan-status, .result-card, .error-card { display: flex; align-items: flex-start; gap: 11px; }
  .scan-status strong, .status-card strong, .result-card strong, .error-card strong { font-size: 14px; }
  .scan-status p, .status-card p, .result-card p, .error-card p { margin: 5px 0 0; color: var(--text-muted); font-size: 11px; line-height: 1.55; }
  .status-card button { margin-top: 14px; }
  .status-actions { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 14px; }
  .status-actions button { margin-top: 0; }
  .cancel-hint { margin-top: 9px !important; font-size: 10px !important; }
  .status-note { color: #fcd34d !important; }
  .spinner { width: 16px; height: 16px; flex: 0 0 16px; margin-top: 2px; border: 2px solid rgba(255,255,255,.24); border-top-color: #a5b4fc; border-radius: 50%; animation: spin .75s linear infinite; }
  .danger-spinner { border-top-color: #fca5a5; }
  .removed-banner { display: flex; align-items: baseline; gap: 10px; margin-top: 16px; padding: 10px 12px; border: 1px solid rgba(34,197,94,.3); border-radius: var(--radius-sm); background: rgba(34,197,94,.08); }
  .removed-banner strong { color: #86efac; font-size: 12px; }
  .removed-banner span { color: var(--text-muted); font-size: 11px; }
  .cleanup-actions { position: sticky; bottom: -20px; display: flex; justify-content: space-between; align-items: center; gap: 14px; margin: 18px -20px -20px; padding: 11px 20px; border-top: 1px solid var(--border); background: rgba(30,41,59,.96); font-size: 11px; backdrop-filter: blur(8px); }
  .danger-action { border: 1px solid rgba(239,68,68,.5); background: rgba(239,68,68,.14); color: #fecaca; }
  .result-mark { width: 30px; height: 30px; flex: 0 0 30px; border-radius: 50%; background: rgba(34,197,94,.15); }
  .result-mark::after { content: ""; display: block; width: 10px; height: 5px; margin: 9px 0 0 9px; border-left: 2px solid #86efac; border-bottom: 2px solid #86efac; transform: rotate(-45deg); }
  .error-card > span { display: grid; place-items: center; width: 30px; height: 30px; flex: 0 0 30px; border-radius: 50%; background: rgba(239,68,68,.14); color: #fca5a5; font-weight: 700; }
  .warnings { color: #fcd34d; font-size: 11px; line-height: 1.6; }
  .result-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 12px; }
  .retry { margin-top: 10px; }
  @keyframes spin { to { transform: rotate(360deg); } }
</style>
