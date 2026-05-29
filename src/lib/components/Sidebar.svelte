<script>
  import Icon from "./Icon.svelte";

  export let current = "overview";
  export let onSelect = () => {};

  export const items = [
    { id: "overview", label: "概览", icon: "gauge" },
    { id: "process", label: "进程管理", icon: "cpu" },
    { id: "events", label: "实时事件", icon: "activity" },
    { id: "network", label: "网络与端口", icon: "network" },
    { id: "disk", label: "磁盘管理", icon: "harddrive" },
    { id: "toolbox", label: "工具箱", icon: "terminal" },
    { id: "ai", label: "AI 助手", icon: "sparkles" },
    { id: "about", label: "关于", icon: "info" },
  ];
</script>

<aside class="sidebar">
  <div class="brand">
    <span class="brand-dot"></span>
    <div class="brand-title">Win-Top</div>
  </div>

  <nav>
    {#each items as item}
      <button
        class="nav-item"
        class:active={current === item.id}
        on:click={() => onSelect(item.id)}
        aria-current={current === item.id ? "page" : undefined}
      >
        <Icon name={item.icon} size={18} />
        <span>{item.label}</span>
      </button>
    {/each}
  </nav>

  <div class="foot">
    <span class="status-dot"></span>
    <span>原生采集 · ETW 架构</span>
  </div>
</aside>

<style>
  .sidebar {
    width: 212px;
    flex-shrink: 0;
    background: var(--surface);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    padding: var(--sp-4);
    gap: var(--sp-6);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2);
  }
  .brand-dot {
    width: 12px;
    height: 12px;
    border-radius: 999px;
    background: var(--accent);
    box-shadow: 0 0 12px rgba(99, 102, 241, 0.7);
  }
  .brand-title {
    font-weight: 600;
    font-size: 17px;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    border: none;
    background: transparent;
    color: var(--text-muted);
    font-size: 14px;
    font-family: inherit;
    cursor: pointer;
    text-align: left;
    transition: background 0.15s ease, color 0.15s ease;
  }
  .nav-item:hover {
    background: var(--surface-2);
    color: var(--text);
  }
  .nav-item.active {
    background: var(--surface-2);
    color: var(--text);
  }
  .nav-item.active::before {
    content: "";
    position: absolute;
    left: 0;
    width: 3px;
    height: 20px;
    border-radius: 0 3px 3px 0;
    background: var(--accent);
    margin-left: -12px;
  }
  .nav-item:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .foot {
    margin-top: auto;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: 12px;
    color: var(--text-muted);
    padding: var(--sp-2);
  }
  .status-dot {
    width: 8px;
    height: 8px;
    border-radius: 999px;
    background: var(--ok);
  }
</style>
