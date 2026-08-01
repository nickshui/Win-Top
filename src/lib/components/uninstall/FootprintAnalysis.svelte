<script>
  import { createEventDispatcher } from "svelte";
  import { canOpenArtifact, summarizeFootprint } from "../../uninstallWorkflow.js";

  export let footprint;
  export let fmtBytes = (value) => `${value} B`;

  const dispatch = createEventDispatcher();

  const removableDir = (item) => item.kind === "dir";
  const categoryLabel = (category) =>
    ({
      "user-data": "用户数据",
      "app-runtime": "运行数据",
      cache: "缓存",
      config: "配置",
      binary: "程序文件",
      unknown: "其他",
    }[category] || category);
  const kindLabel = (kind) =>
    ({ dir: "目录", "reg-key": "注册表", service: "服务", startup: "启动项" }[kind] || kind);
  const confidenceLabel = (confidence) =>
    ({ high: "高置信", medium: "中置信", low: "低置信" }[confidence] || confidence);

  $: artifacts = footprint?.artifacts || [];
  $: summary = summarizeFootprint(footprint);
  $: groups = [
    {
      title: "程序文件",
      note: "安装目录及由系统登记信息定位到的程序文件",
      rows: artifacts.filter(
        (item) => removableDir(item) && (item.source === "install-dir" || item.category === "binary")
      ),
    },
    {
      title: "应用数据",
      note: "运行缓存、配置及应用生成的数据目录",
      rows: artifacts.filter(
        (item) =>
          removableDir(item) &&
          item.source !== "install-dir" &&
          item.category !== "binary" &&
          item.category !== "user-data"
      ),
    },
    {
      title: "用户数据",
      note: "可能包含存档、项目、作品或个人设置",
      rows: artifacts.filter((item) => removableDir(item) && item.category === "user-data"),
      tone: "user-data",
    },
    {
      title: "注册表",
      note: "匹配到的应用配置项，不计入磁盘体积",
      rows: artifacts.filter((item) => item.kind === "reg-key"),
    },
    {
      title: "系统关联",
      note: "与安装目录关联的服务和启动项",
      rows: artifacts.filter((item) => item.kind === "service" || item.kind === "startup"),
    },
  ];
</script>

<div class="summary-grid" aria-label="应用占用概况">
  <article><span>扫描到的占用</span><strong>{fmtBytes(summary.totalSize)}</strong><small>可访问目录合计</small></article>
  <article><span>程序文件</span><strong>{fmtBytes(summary.programSize)}</strong><small>安装目录</small></article>
  <article><span>应用数据</span><strong>{fmtBytes(summary.appDataSize)}</strong><small>缓存、配置和运行数据</small></article>
  <article class:user-data={summary.userDataSize > 0}><span>用户数据</span><strong>{fmtBytes(summary.userDataSize)}</strong><small>存档、项目和个人数据</small></article>
</div>

<div class="system-counts" aria-label="系统关联概况">
  <span>注册表 <strong>{summary.registryCount}</strong></span>
  <span>服务 <strong>{summary.serviceCount}</strong></span>
  <span>启动项 <strong>{summary.startupCount}</strong></span>
</div>

{#if footprint?.warnings?.length}
  <div class="warnings" role="status">
    {#each footprint.warnings as warning}<p>{warning}</p>{/each}
  </div>
{/if}

{#each groups as group}
  {#if group.rows.length}
    <section class="analysis-group" class:user-data={group.tone === "user-data"}>
      <header>
        <h4>{group.title} <span>{group.rows.length}</span></h4>
        <p>{group.note}</p>
      </header>
      <div class="artifact-list">
        {#each group.rows as artifact (artifact.path)}
          <div class="artifact" class:user-data={artifact.category === "user-data"}>
            <span class="badges">
              <span>{kindLabel(artifact.kind)}</span>
              <span>{categoryLabel(artifact.category)}</span>
              <span class="confidence {artifact.confidence}">{confidenceLabel(artifact.confidence)}</span>
            </span>
            <span class="path" title={artifact.path}>{artifact.path}</span>
            <span class="size">{artifact.size ? fmtBytes(artifact.size) : ""}</span>
            {#if canOpenArtifact(artifact)}
              <button
                type="button"
                class="open-action"
                title="在资源管理器中打开此目录"
                on:click={() => dispatch("open", { path: artifact.path })}
              >打开目录</button>
            {/if}
            <span class="reason" title={artifact.match_reason}>{artifact.match_reason}</span>
          </div>
        {/each}
      </div>
    </section>
  {/if}
{/each}

{#if artifacts.length === 0}
  <div class="empty">未在当前可访问位置发现关联文件、目录或注册表项</div>
{/if}

<p class="scan-note">体积来自可访问目录的实际统计；注册表、服务和启动项不计入磁盘占用。</p>

<style>
  .summary-grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 8px; margin-top: 16px; }
  .summary-grid article { min-width: 0; padding: 12px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: rgba(15, 23, 42, 0.28); }
  .summary-grid article.user-data { border-color: rgba(239, 68, 68, 0.34); background: rgba(239, 68, 68, 0.05); }
  .summary-grid span, .summary-grid small { display: block; color: var(--text-muted); font-size: 10px; }
  .summary-grid strong { display: block; margin: 6px 0 3px; overflow: hidden; text-overflow: ellipsis; color: var(--text); font: 600 16px var(--font-mono); }
  .system-counts { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 8px; }
  .system-counts span { padding: 4px 8px; border: 1px solid var(--border); border-radius: 999px; color: var(--text-muted); font-size: 10px; }
  .system-counts strong { color: var(--text); font-family: var(--font-mono); }
  .warnings { margin-top: 12px; padding: 8px 10px; border: 1px solid rgba(245, 158, 11, 0.3); border-radius: var(--radius-sm); background: rgba(245, 158, 11, 0.06); }
  .warnings p { margin: 2px 0; color: #fcd34d; font-size: 11px; line-height: 1.5; }
  .analysis-group { margin-top: 18px; }
  .analysis-group header { display: flex; justify-content: space-between; align-items: baseline; gap: 14px; margin-bottom: 7px; padding-bottom: 6px; border-bottom: 1px solid var(--border); }
  .analysis-group h4, .analysis-group p { margin: 0; }
  .analysis-group h4 { font-size: 13px; }
  .analysis-group h4 span { color: var(--text-muted); font-weight: 400; }
  .analysis-group p { color: var(--text-muted); font-size: 10px; text-align: right; }
  .artifact-list { display: flex; flex-direction: column; gap: 4px; }
  .artifact { display: grid; grid-template-columns: auto minmax(0, 1fr) auto auto; align-items: center; gap: 8px; min-height: 50px; padding: 7px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--surface-2); }
  .artifact.user-data { border-color: rgba(239, 68, 68, 0.34); background: rgba(239, 68, 68, 0.05); }
  .badges { display: flex; flex-wrap: wrap; gap: 4px; }
  .badges span { padding: 1px 5px; border: 1px solid var(--border); border-radius: 4px; color: var(--text-muted); font-size: 9px; white-space: nowrap; }
  .badges .confidence.high { color: #86efac; }
  .badges .confidence.medium { color: #fcd34d; }
  .path, .size { font: 11px var(--font-mono); }
  .path { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .size { color: var(--text-muted); white-space: nowrap; }
  .open-action { min-height: 28px; padding: 4px 8px; border: 1px solid var(--border); border-radius: 6px; background: transparent; color: var(--text); font-family: inherit; font-size: 10px; white-space: nowrap; cursor: pointer; }
  .open-action:hover { border-color: #526c8c; background: rgba(99, 102, 241, 0.09); }
  .open-action:focus-visible { outline: 2px solid #a5b4fc; outline-offset: 2px; }
  .reason { grid-column: 2 / -1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-muted); font-size: 10px; }
  .empty { padding: 32px 12px; color: var(--text-muted); text-align: center; font-size: 12px; }
  .scan-note { margin: 14px 0 0; color: var(--text-muted); font-size: 10px; }
  @media (max-width: 1050px) {
    .summary-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .analysis-group header { align-items: flex-start; flex-direction: column; gap: 2px; }
    .analysis-group p { text-align: left; }
  }
</style>
