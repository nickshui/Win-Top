<script>
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { pushToast, elevated, relaunchAdmin } from "../stores.js";
  import Modal from "../components/Modal.svelte";

  let tab = "optimize"; // optimize | startup

  // 清理
  let scanning = false;
  let report = null;
  let selected = new Set();
  let optimizing = false;
  let result = null;
  let confirmOpen = false;

  // 建议关闭后台
  let bgList = [];
  let bgSelected = new Set();
  let closeOpen = false;

  // 启动项
  let startupItems = [];
  let startupLoading = true;

  const fmtBytes = (n) => {
    if (!n) return "0 B";
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
    return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
  };

  function canUse(cat) {
    return cat.available && !(cat.needs_admin && !$elevated);
  }

  async function scan() {
    scanning = true;
    result = null;
    bgList = [];
    try {
      report = await invoke("scan_junk");
      selected = new Set(report.categories.filter(canUse).map((c) => c.id));
    } catch (e) {
      pushToast("扫描失败：" + e, "error");
    } finally {
      scanning = false;
    }
  }

  function toggleCat(id) {
    const s = new Set(selected);
    if (s.has(id)) s.delete(id);
    else s.add(id);
    selected = s;
  }

  $: selectedBytes = report
    ? report.categories.filter((c) => selected.has(c.id)).reduce((a, c) => a + c.bytes, 0)
    : 0;

  async function runOptimize() {
    confirmOpen = false;
    optimizing = true;
    try {
      const ids = [...selected];
      const clean = await invoke("clean_junk", { ids });
      const boost = await invoke("memory_boost");
      result = {
        freed_bytes: clean.freed_bytes,
        freed_mb: boost.freed_mb,
        trimmed_count: boost.trimmed_count,
      };
      bgList = await invoke("suggest_background");
      bgSelected = new Set();
      report = await invoke("scan_junk");
      selected = new Set(report.categories.filter(canUse).map((c) => c.id));
    } catch (e) {
      pushToast("优化失败：" + e, "error");
    } finally {
      optimizing = false;
    }
  }

  function toggleBg(pid) {
    const s = new Set(bgSelected);
    if (s.has(pid)) s.delete(pid);
    else s.add(pid);
    bgSelected = s;
  }

  async function closeSelectedBg() {
    closeOpen = false;
    const pids = [...bgSelected];
    for (const pid of pids) {
      try {
        const r = await invoke("terminate_process", { pid });
        pushToast(r.message, r.success ? "ok" : "error");
      } catch (e) {
        pushToast("结束失败：" + e, "error");
      }
    }
    bgList = bgList.filter((b) => !bgSelected.has(b.pid));
    bgSelected = new Set();
  }

  async function loadStartup() {
    startupLoading = true;
    try {
      startupItems = await invoke("list_startup");
    } catch (e) {
      pushToast("读取启动项失败：" + e, "error");
    } finally {
      startupLoading = false;
    }
  }

  let toggleTarget = null; // 待确认切换的启动项
  let togglingId = null; // 切换请求进行中的 id

  function requestToggle(item) {
    if (togglingId) return;
    toggleTarget = item;
  }

  async function confirmToggle() {
    const item = toggleTarget;
    toggleTarget = null;
    if (!item) return;
    const next = !item.enabled;
    togglingId = item.id;
    try {
      const r = await invoke("set_startup_enabled", { id: item.id, enabled: next });
      if (r.success) {
        item.enabled = next;
        startupItems = startupItems;
      }
      pushToast(r.message, r.success ? "ok" : "error");
    } catch (e) {
      pushToast("操作失败：" + e, "error");
    } finally {
      togglingId = null;
    }
  }

  const locLabel = (l) =>
    ({
      "HKCU-Run": "用户注册表",
      "HKLM-Run": "系统注册表",
      "User-Folder": "用户启动文件夹",
      "Common-Folder": "公共启动文件夹",
    })[l] || l;

  onMount(loadStartup);
</script>

{#if !$elevated}
  <div class="admin-banner">
    <span>部分操作需要管理员权限（系统垃圾清理 / 系统注册表启动项 / 裁剪系统进程）。</span>
    <button class="primary" on:click={relaunchAdmin}>以管理员重启</button>
  </div>
{/if}

<div class="tabs" role="tablist">
  <button class="tab" class:active={tab === "optimize"} on:click={() => (tab = "optimize")}>一键优化</button>
  <button class="tab" class:active={tab === "startup"} on:click={() => (tab = "startup")}>
    启动项 <span class="tab-badge">{startupItems.length}</span>
  </button>
</div>

{#if tab === "optimize"}
  <section class="opt">
    {#if !report}
      <div class="scan-intro">
        <p class="muted">扫描临时文件、缓存、回收站等可清理项，并可一键释放内存。</p>
        <button class="primary big" on:click={scan} disabled={scanning}>
          {scanning ? "扫描中…" : "扫描垃圾"}
        </button>
      </div>
    {:else}
      <div class="summary">
        <div class="gauge">
          <div class="gauge-num mono">{fmtBytes(selectedBytes)}</div>
          <div class="gauge-label muted">已选可清理</div>
        </div>
        <div class="summary-actions">
          <button class="ghost" on:click={scan} disabled={scanning}>{scanning ? "扫描中…" : "重新扫描"}</button>
          <button class="primary" on:click={() => (confirmOpen = true)} disabled={optimizing || selected.size === 0}>
            {optimizing ? "优化中…" : "一键优化"}
          </button>
        </div>
      </div>

      <div class="cats">
        {#each report.categories as c (c.id)}
          <label class="cat" class:off={!canUse(c)}>
            <input type="checkbox" checked={selected.has(c.id)} disabled={!canUse(c)} on:change={() => toggleCat(c.id)} />
            <span class="cat-label">
              {c.label}
              {#if c.needs_admin}<span class="lock" title="需管理员">🔒</span>{/if}
              {#if !c.available}<span class="muted">（不可用）</span>{/if}
            </span>
            <span class="cat-size mono">{fmtBytes(c.bytes)}</span>
          </label>
        {/each}
      </div>

      {#if result}
        <div class="result">
          <div class="res-item"><span class="muted">释放磁盘</span><b class="mono">{fmtBytes(result.freed_bytes)}</b></div>
          <div class="res-item"><span class="muted">释放内存</span><b class="mono">{result.freed_mb.toFixed(0)} MB</b></div>
          <div class="res-item"><span class="muted">裁剪进程</span><b class="mono">{result.trimmed_count}</b></div>
        </div>

        {#if bgList.length > 0}
          <div class="bg">
            <div class="bg-head">
              <h3>建议关闭的后台进程</h3>
              <button class="danger" on:click={() => (closeOpen = true)} disabled={bgSelected.size === 0}>
                结束所选 ({bgSelected.size})
              </button>
            </div>
            <div class="bg-list">
              {#each bgList as b (b.pid)}
                <label class="bg-item">
                  <input type="checkbox" checked={bgSelected.has(b.pid)} on:change={() => toggleBg(b.pid)} />
                  <span class="bg-name">{b.name}</span>
                  <span class="muted mono">PID {b.pid}</span>
                  <span class="bg-mem mono">{b.mem_mb.toFixed(0)} MB</span>
                </label>
              {/each}
            </div>
          </div>
        {/if}
      {/if}
    {/if}
  </section>
{:else}
  <div class="table-wrap">
    <table>
      <thead>
        <tr><th class="col-name">名称</th><th>位置</th><th class="col-cmd">命令</th><th class="col-sw">状态</th></tr>
      </thead>
      <tbody>
        {#if startupLoading}
          <tr><td colspan="4" class="empty">加载中…</td></tr>
        {:else if startupItems.length === 0}
          <tr><td colspan="4" class="empty">无启动项</td></tr>
        {:else}
          {#each startupItems as item (item.id)}
            <tr>
              <td class="col-name" title={item.name}>{item.name}</td>
              <td>{locLabel(item.location)}</td>
              <td class="col-cmd mono" title={item.command}>{item.command}</td>
              <td class="col-sw">
                <button
                  class="sw"
                  class:on={item.enabled}
                  class:pending={togglingId === item.id}
                  on:click={() => requestToggle(item)}
                  disabled={togglingId === item.id}
                  role="switch"
                  aria-checked={item.enabled}
                  aria-label={(item.enabled ? "禁用 " : "启用 ") + item.name}
                >
                  <span class="sw-knob"></span>
                </button>
              </td>
            </tr>
          {/each}
        {/if}
      </tbody>
    </table>
  </div>
{/if}

<Modal open={confirmOpen} title="确认一键优化" on:close={() => (confirmOpen = false)}>
  <p>将清理选中的 {selected.size} 个分类（约 {fmtBytes(selectedBytes)}），并裁剪进程工作集释放内存。</p>
  <p class="muted">清理为删除操作、不可撤销；被占用的文件会自动跳过。</p>
  <div class="modal-actions">
    <button class="ghost" on:click={() => (confirmOpen = false)}>取消</button>
    <button class="primary" on:click={runOptimize}>确认优化</button>
  </div>
</Modal>

<Modal open={closeOpen} title="结束所选进程" on:close={() => (closeOpen = false)}>
  <p>将结束选中的 {bgSelected.size} 个进程，未保存数据可能丢失。</p>
  <div class="modal-actions">
    <button class="ghost" on:click={() => (closeOpen = false)}>取消</button>
    <button class="danger" on:click={closeSelectedBg}>确认结束</button>
  </div>
</Modal>

<Modal
  open={!!toggleTarget}
  title={toggleTarget && toggleTarget.enabled ? "禁用启动项" : "启用启动项"}
  on:close={() => (toggleTarget = null)}
>
  {#if toggleTarget}
    <p>
      确定要{toggleTarget.enabled ? "禁用" : "启用"}
      <strong>{toggleTarget.name}</strong> 的开机自启吗？
    </p>
    <p class="muted">
      {toggleTarget.enabled
        ? "禁用后该程序将不再随 Windows 启动（可随时重新启用）。"
        : "启用后该程序将随 Windows 启动。"}
    </p>
    <div class="modal-actions">
      <button class="ghost" on:click={() => (toggleTarget = null)}>取消</button>
      <button class="primary" on:click={confirmToggle}>
        确认{toggleTarget.enabled ? "禁用" : "启用"}
      </button>
    </div>
  {/if}
</Modal>

<style>
  .admin-banner {
    display: flex;
    align-items: center;
    gap: var(--sp-4);
    flex-wrap: wrap;
    padding: 10px 14px;
    margin-bottom: var(--sp-4);
    border: 1px solid rgba(245, 158, 11, 0.35);
    background: rgba(245, 158, 11, 0.1);
    border-radius: var(--radius-sm);
    font-size: 13px;
    color: #fcd34d;
  }
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
  .tab:hover { color: var(--text); }
  .tab.active { color: var(--text); border-bottom-color: var(--accent); }
  .tab-badge {
    font-size: 11px;
    color: var(--text-muted);
    background: var(--surface-2);
    padding: 1px 7px;
    border-radius: 999px;
    font-variant-numeric: tabular-nums;
  }

  .scan-intro {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sp-4);
    padding: var(--sp-6);
    text-align: center;
  }
  .summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-4);
    padding: var(--sp-4) var(--sp-6);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    margin-bottom: var(--sp-4);
  }
  .gauge-num { font-size: 32px; font-weight: 700; font-variant-numeric: tabular-nums; }
  .gauge-label { font-size: 12px; }
  .summary-actions { display: flex; gap: var(--sp-2); }

  .cats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: var(--sp-2);
  }
  .cat {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 10px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--surface);
    cursor: pointer;
    font-size: 13px;
  }
  .cat.off { opacity: 0.5; cursor: default; }
  .cat input { accent-color: var(--accent); }
  .cat-label { flex: 1; }
  .cat-size { color: var(--text-muted); }
  .lock { margin-left: 4px; }

  .result {
    display: flex;
    gap: var(--sp-6);
    margin: var(--sp-4) 0;
    padding: var(--sp-4);
    border: 1px solid rgba(34, 197, 94, 0.35);
    background: rgba(34, 197, 94, 0.08);
    border-radius: var(--radius);
  }
  .res-item { display: flex; flex-direction: column; gap: 2px; }
  .res-item b { font-size: 20px; }

  .bg { margin-top: var(--sp-4); }
  .bg-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: var(--sp-2); }
  .bg-head h3 { margin: 0; font-size: 14px; }
  .bg-list { display: flex; flex-direction: column; gap: 6px; }
  .bg-item {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: 8px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--surface);
    cursor: pointer;
    font-size: 13px;
  }
  .bg-item input { accent-color: var(--accent); }
  .bg-name { flex: 1; font-weight: 500; }
  .bg-mem { color: var(--text-muted); }

  .table-wrap {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--surface);
  }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  thead th {
    position: sticky;
    top: 0;
    background: var(--surface-2);
    text-align: left;
    padding: 10px 14px;
    font-weight: 500;
    color: var(--text-muted);
    white-space: nowrap;
  }
  tbody { display: block; max-height: calc(100vh - 230px); overflow-y: auto; }
  thead, tbody tr { display: table; width: 100%; table-layout: fixed; }
  td { padding: 8px 14px; border-top: 1px solid var(--border); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  tbody tr:hover { background: var(--surface-2); }
  .col-sw { width: 110px; text-align: right; }
  .empty { text-align: center; color: var(--text-muted); padding: 40px; }

  .mono { font-family: var(--font-mono); font-variant-numeric: tabular-nums; }
  .muted { color: var(--text-muted); }

  .primary {
    border: none;
    background: linear-gradient(135deg, var(--accent), #7c3aed);
    color: #fff;
    font-family: inherit;
    font-size: 13px;
    padding: 8px 16px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .primary.big { font-size: 15px; padding: 12px 28px; }
  .primary:disabled { opacity: 0.6; cursor: default; }
  .ghost {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
    padding: 8px 16px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .ghost:hover { background: var(--surface-2); }
  .danger {
    border: 1px solid rgba(239, 68, 68, 0.4);
    background: transparent;
    color: var(--danger);
    font-family: inherit;
    font-size: 12px;
    padding: 6px 12px;
    border-radius: 8px;
    cursor: pointer;
  }
  .danger:hover { background: rgba(239, 68, 68, 0.12); }
  .danger:disabled { opacity: 0.5; cursor: default; }
  .sw {
    position: relative;
    width: 44px;
    height: 24px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface-2);
    cursor: pointer;
    vertical-align: middle;
    transition: background 0.25s ease, border-color 0.25s ease;
  }
  .sw-knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--text-muted);
    transition: transform 0.25s cubic-bezier(0.4, 0, 0.2, 1), background 0.25s ease;
  }
  .sw.on {
    background: rgba(34, 197, 94, 0.25);
    border-color: rgba(34, 197, 94, 0.5);
  }
  .sw.on .sw-knob {
    transform: translateX(20px);
    background: var(--ok);
  }
  .sw:hover { border-color: var(--accent); }
  .sw:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .sw.pending { opacity: 0.55; cursor: default; }
  .sw.pending .sw-knob { animation: sw-pulse 0.8s ease-in-out infinite; }
  @keyframes sw-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.35; }
  }
  .modal-actions { display: flex; justify-content: flex-end; gap: var(--sp-2); margin-top: var(--sp-3); }
</style>
