<script>
  import { metrics, connected, elevated, relaunchAdmin } from "../stores.js";

  export let title = "";
</script>

<header class="topbar">
  <div class="left">
    <h1>{title}</h1>
  </div>
  <div class="right">
    <input class="search" placeholder="搜索进程 / 端口 / 命令  (Ctrl+K)" aria-label="全局搜索" />
    {#if $elevated}
      <span class="admin">管理员</span>
    {:else}
      <button class="elevate" on:click={relaunchAdmin} title="ETW 实时事件 / 磁盘温度需管理员权限">
        以管理员重启
      </button>
    {/if}
    {#if $connected}
      <span class="status ok">● 实时</span>
      <span class="ts">{$metrics?.ts ?? "--:--:--"}</span>
    {:else}
      <span class="status wait">○ 连接中…</span>
    {/if}
  </div>
</header>

<style>
  .topbar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: var(--sp-4) var(--sp-6);
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  h1 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
  }
  .right {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
  }
  .search {
    width: 280px;
    padding: 8px 12px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
  }
  .search:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .status {
    font-size: 13px;
  }
  .status.ok {
    color: var(--ok);
  }
  .status.wait {
    color: var(--text-muted);
  }
  .ts {
    font-family: var(--font-mono);
    font-size: 13px;
    color: var(--text-muted);
  }
  .admin {
    font-size: 11px;
    padding: 3px 10px;
    border-radius: 999px;
    color: var(--ok);
    background: rgba(34, 197, 94, 0.12);
    border: 1px solid rgba(34, 197, 94, 0.4);
  }
  .elevate {
    font-size: 12px;
    padding: 6px 12px;
    border-radius: var(--radius-sm);
    border: 1px solid rgba(245, 158, 11, 0.45);
    background: rgba(245, 158, 11, 0.1);
    color: var(--warn);
    font-family: inherit;
    cursor: pointer;
    transition: background 0.15s ease;
  }
  .elevate:hover {
    background: rgba(245, 158, 11, 0.18);
  }
  .elevate:focus-visible {
    outline: 2px solid var(--warn);
    outline-offset: 1px;
  }
</style>
