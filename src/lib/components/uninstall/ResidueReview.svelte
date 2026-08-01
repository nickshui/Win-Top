<script>
  import { createEventDispatcher } from "svelte";

  export let artifacts = [];
  export let selectedPaths = new Set();
  export let fmtBytes = (value) => `${value} B`;

  const dispatch = createEventDispatcher();

  const removable = (item) => item.kind === "dir" || item.kind === "reg-key";
  const recommendedItem = (item) =>
    removable(item) &&
    item.source?.startsWith("residue") &&
    item.confidence === "high" &&
    item.category !== "user-data";

  $: recommended = artifacts.filter(recommendedItem);
  $: review = artifacts.filter((item) => removable(item) && !recommendedItem(item));
  $: related = artifacts.filter((item) => !removable(item));

  const kindLabel = (kind) =>
    ({ dir: "目录", "reg-key": "注册表", service: "服务", startup: "启动项" }[kind] || kind);
  const categoryLabel = (category) =>
    ({
      "user-data": "用户数据",
      "app-runtime": "运行数据",
      cache: "缓存",
      config: "配置",
      binary: "程序文件",
      unknown: "其他",
    }[category] || category);
</script>

{#each [
  { title: "建议清理", note: "完整产品名匹配且不含用户数据，已默认选中", rows: recommended, tone: "safe" },
  { title: "需要复核", note: "共享目录、安装目录或归属依据不足，默认不选", rows: review, tone: "review" },
  { title: "仅供参考", note: "服务和启动项仍由厂商卸载器处理", rows: related, tone: "related" },
] as group}
  {#if group.rows.length}
    <section class="group {group.tone}">
      <header>
        <h4>{group.title} <span>{group.rows.length}</span></h4>
        <p>{group.note}</p>
      </header>
      <div class="artifact-list">
        {#each group.rows as artifact (artifact.path)}
          <label class="artifact" class:readonly={group.tone === "related"} class:userdata={artifact.category === "user-data"}>
            {#if group.tone !== "related"}
              <input
                type="checkbox"
                checked={selectedPaths.has(artifact.path)}
                on:change={() => dispatch("toggle", { path: artifact.path })}
              />
            {:else}
              <span class="reference-mark" aria-hidden="true">—</span>
            {/if}
            <span class="badges">
              <span>{kindLabel(artifact.kind)}</span>
              <span class:danger={artifact.category === "user-data"}>{categoryLabel(artifact.category)}</span>
              <span>{artifact.confidence === "high" ? "强匹配" : artifact.confidence === "medium" ? "需核对" : "弱匹配"}</span>
            </span>
            <span class="path" title={artifact.path}>{artifact.path}</span>
            <span class="size">{artifact.size ? fmtBytes(artifact.size) : ""}</span>
            <span class="reason">{artifact.match_reason}</span>
          </label>
        {/each}
      </div>
    </section>
  {/if}
{/each}

{#if artifacts.length === 0}
  <div class="empty">未发现仍存在的文件、目录或注册表项</div>
{/if}

<style>
  .group { margin-top: 20px; }
  .group header { display: flex; justify-content: space-between; align-items: baseline; gap: 16px; margin-bottom: 8px; }
  .group h4, .group p { margin: 0; }
  .group h4 { font-size: 13px; }
  .group h4 span { color: var(--text-muted); font-weight: 400; }
  .group p { color: var(--text-muted); font-size: 11px; text-align: right; }
  .artifact-list { display: flex; flex-direction: column; gap: 6px; }
  .artifact {
    min-height: 54px; display: grid; grid-template-columns: auto auto minmax(0, 1fr) auto;
    align-items: center; gap: 8px; padding: 8px 10px; border: 1px solid var(--border);
    border-radius: var(--radius-sm); background: var(--surface-2); cursor: pointer;
  }
  .artifact:hover { border-color: #45607e; }
  .artifact:focus-within { outline: 2px solid var(--accent); outline-offset: 2px; }
  .artifact.readonly { cursor: default; opacity: 0.78; }
  .artifact.userdata { border-color: rgba(239, 68, 68, 0.38); background: rgba(239, 68, 68, 0.06); }
  .artifact input { width: 16px; height: 16px; accent-color: var(--accent); }
  .reference-mark { width: 16px; color: var(--text-muted); text-align: center; }
  .badges { display: flex; gap: 4px; flex-wrap: wrap; }
  .badges span { padding: 2px 6px; border: 1px solid var(--border); border-radius: 5px; color: var(--text-muted); font-size: 10px; white-space: nowrap; }
  .badges .danger { color: #fca5a5; border-color: rgba(239, 68, 68, 0.42); }
  .path, .size { font: 12px var(--font-mono); }
  .path { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .size { color: var(--text-muted); white-space: nowrap; }
  .reason { grid-column: 3 / -1; color: var(--text-muted); font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .review .artifact { border-color: rgba(245, 158, 11, 0.25); }
  .related .artifact { background: rgba(51, 65, 85, 0.55); }
  .empty { padding: 36px 16px; color: var(--text-muted); text-align: center; font-size: 13px; }
  @media (max-width: 980px) {
    .group header { align-items: flex-start; flex-direction: column; gap: 3px; }
    .group p { text-align: left; }
    .artifact { grid-template-columns: auto minmax(0, 1fr) auto; }
    .badges { grid-column: 2 / -1; }
    .reason { grid-column: 2 / -1; }
  }
</style>
