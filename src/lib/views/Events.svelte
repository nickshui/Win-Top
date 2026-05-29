<script>
  import { procEvents, etwAvailable, etwReason, elevated, relaunchAdmin } from "../stores.js";

  let filter = "all"; // all | start | stop
  let q = "";
  let paused = false;
  let frozen = [];

  $: live = $procEvents.filter((e) => {
    if (filter !== "all" && e.action !== filter) return false;
    if (q && !e.image.toLowerCase().includes(q.toLowerCase())) return false;
    return true;
  });
  $: shown = paused ? frozen : live;

  $: startCount = $procEvents.filter((e) => e.action === "start").length;
  $: stopCount = $procEvents.filter((e) => e.action === "stop").length;

  function togglePause() {
    if (!paused) frozen = live;
    paused = !paused;
  }
</script>

{#if !$etwAvailable}
  <div class="banner">
    <div>
      <strong>ETW 实时事件未启用</strong>
      <p>{$etwReason || "实时进程事件流需要管理员权限。"}</p>
    </div>
    {#if !$elevated}
      <button class="elevate" on:click={relaunchAdmin}>以管理员重启</button>
    {/if}
  </div>
{/if}

<div class="toolbar">
  <input
    class="search"
    bind:value={q}
    placeholder="按进程名过滤（如 notepad）"
    aria-label="过滤事件"
  />
  <div class="chips" role="group" aria-label="事件过滤">
    {#each [["all", "全部"], ["start", "启动"], ["stop", "结束"]] as [v, label]}
      <button class="chip" class:active={filter === v} on:click={() => (filter = v)}>{label}</button>
    {/each}
  </div>
  <button class="chip pause" class:on={paused} on:click={togglePause}>
    {paused ? "▶ 继续" : "⏸ 暂停"}
  </button>
  <span class="count">
    {#if $etwAvailable}<span class="dot live"></span>实时{:else}<span class="dot"></span>未连接{/if}
    · 启动 {startCount} · 结束 {stopCount} · 缓冲 {$procEvents.length}/200
  </span>
</div>

<div class="feed">
  {#if shown.length === 0}
    <div class="empty">
      {$etwAvailable
        ? q
          ? `无匹配「${q}」的事件——打开/关闭该程序试试`
          : "等待进程事件…（打开/关闭任意程序试试）"
        : "无事件"}
    </div>
  {:else}
    {#each shown as e (e._id)}
      <div class="row">
        <span class="ts">{e.ts}</span>
        <span class="badge {e.action}">{e.action === "start" ? "启动" : "结束"}</span>
        <span class="image" title={e.image}>{e.image}</span>
        <span class="pid">PID {e.pid}</span>
      </div>
    {/each}
  {/if}
</div>

<style>
  .banner {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--sp-4);
    padding: var(--sp-3) var(--sp-4);
    margin-bottom: var(--sp-4);
    border-radius: var(--radius-sm);
    background: rgba(245, 158, 11, 0.1);
    border: 1px solid rgba(245, 158, 11, 0.35);
    color: #fcd34d;
  }
  .banner p {
    margin: 4px 0 0;
    font-size: 12px;
    color: var(--text-muted);
  }
  .elevate {
    flex-shrink: 0;
    font-size: 12px;
    padding: 8px 14px;
    border-radius: var(--radius-sm);
    border: 1px solid rgba(245, 158, 11, 0.45);
    background: rgba(245, 158, 11, 0.15);
    color: var(--warn);
    font-family: inherit;
    cursor: pointer;
  }
  .elevate:hover {
    background: rgba(245, 158, 11, 0.25);
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin-bottom: var(--sp-4);
  }
  .search {
    flex: 0 0 240px;
    padding: 8px 12px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
  }
  .search:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .pause {
    font-variant-numeric: tabular-nums;
  }
  .pause.on {
    background: rgba(245, 158, 11, 0.15);
    color: var(--warn);
    border-color: rgba(245, 158, 11, 0.45);
  }
  .chips {
    display: flex;
    gap: 6px;
  }
  .chip {
    padding: 6px 14px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text-muted);
    cursor: pointer;
    font-size: 12px;
    font-family: inherit;
  }
  .chip:hover {
    color: var(--text);
    border-color: var(--accent);
  }
  .chip.active {
    background: var(--surface-2);
    color: var(--text);
    border-color: var(--accent);
  }
  .count {
    margin-left: auto;
    font-size: 12px;
    color: var(--text-muted);
    font-family: var(--font-mono);
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 999px;
    background: var(--text-muted);
  }
  .dot.live {
    background: var(--ok);
    box-shadow: 0 0 8px rgba(34, 197, 94, 0.6);
  }

  .feed {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    overflow: hidden;
    max-height: calc(100vh - 220px);
    overflow-y: auto;
  }
  .row {
    display: grid;
    grid-template-columns: 90px 60px 1fr 120px;
    align-items: center;
    gap: var(--sp-3);
    padding: 8px 14px;
    border-top: 1px solid var(--border);
    font-size: 13px;
  }
  .row:first-child {
    border-top: none;
  }
  .ts {
    font-family: var(--font-mono);
    color: var(--text-muted);
    font-size: 12px;
  }
  .badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    text-align: center;
  }
  .badge.start {
    color: var(--ok);
    background: rgba(34, 197, 94, 0.12);
  }
  .badge.stop {
    color: var(--text-muted);
    background: var(--surface-2);
  }
  .image {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pid {
    font-family: var(--font-mono);
    color: var(--text-muted);
    text-align: right;
    font-size: 12px;
  }
  .empty {
    padding: 40px;
    text-align: center;
    color: var(--text-muted);
  }
</style>
