<script>
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { pushToast } from "../stores.js";
  import Modal from "../components/Modal.svelte";

  let rows = [];
  let killTarget = null; // 选中要结束的端口行
  let loading = true;
  let q = "";
  let proto = "all"; // all | TCP | UDP
  let family = "all"; // all | IPv4 | IPv6
  let listeningOnly = false;
  let timer;

  const COMMON_PORTS = new Set([
    20, 21, 22, 23, 25, 53, 80, 110, 143, 443, 465, 587, 993, 995, 1433, 1521,
    2375, 2376, 3000, 3306, 3389, 4200, 5000, 5173, 5432, 5672, 5984, 6379,
    6443, 7474, 8000, 8080, 8443, 8888, 9000, 9200, 9300, 11211, 15672, 27017,
    50000,
  ]);

  async function refresh() {
    try {
      rows = await invoke("get_connections");
      loading = false;
    } catch (e) {
      pushToast("加载端口失败：" + e, "error");
    }
  }

  onMount(() => {
    refresh();
    timer = setInterval(refresh, 3000);
  });
  onDestroy(() => clearInterval(timer));

  $: filtered = rows.filter((r) => {
    if (proto !== "all" && r.protocol !== proto) return false;
    if (family !== "all" && r.family !== family) return false;
    if (listeningOnly && r.state !== "LISTEN") return false;
    if (q) {
      const s = q.toLowerCase();
      const hay = `${r.port} ${r.process.toLowerCase()} ${r.pid} ${r.remote.toLowerCase()}`;
      if (!hay.includes(s)) return false;
    }
    return true;
  });

  $: listenCount = rows.filter((r) => r.state === "LISTEN").length;

  // 结束确认：分析同端口的其它绑定，消除「v4/v6 不同进程」的误杀风险
  $: killSiblings = killTarget
    ? rows.filter(
        (r) =>
          r.port === killTarget.port &&
          !(
            r.protocol === killTarget.protocol &&
            r.family === killTarget.family &&
            r.pid === killTarget.pid
          )
      )
    : [];
  $: killSameProc = killSiblings.filter((r) => r.pid === killTarget?.pid);
  $: killOtherProc = killSiblings.filter((r) => r.pid !== killTarget?.pid);

  async function doKill() {
    const t = killTarget;
    killTarget = null;
    const r = await invoke("terminate_process", { pid: t.pid });
    pushToast(r.message, r.success ? "ok" : "error");
    refresh();
  }
</script>

<div class="toolbar">
  <input
    class="search"
    bind:value={q}
    placeholder="搜索端口 / 进程 / PID / 远端"
    aria-label="搜索端口"
  />
  <div class="chips" role="group" aria-label="协议过滤">
    {#each ["all", "TCP", "UDP"] as p}
      <button class="chip" class:active={proto === p} on:click={() => (proto = p)}>
        {p === "all" ? "全部" : p}
      </button>
    {/each}
  </div>
  <div class="chips" role="group" aria-label="地址族过滤">
    {#each ["all", "IPv4", "IPv6"] as f}
      <button class="chip" class:active={family === f} on:click={() => (family = f)}>
        {f === "all" ? "全部" : f}
      </button>
    {/each}
  </div>
  <label class="toggle">
    <input type="checkbox" bind:checked={listeningOnly} />
    <span>仅监听</span>
  </label>
  <span class="count">{filtered.length} / {rows.length} · 监听 {listenCount} · 每 3s 刷新</span>
</div>

<div class="table-wrap">
  <table>
    <thead>
      <tr>
        <th class="num">端口</th>
        <th>协议</th>
        <th>状态</th>
        <th class="col-proc">进程</th>
        <th class="num">PID</th>
        <th class="col-remote">远端地址</th>
        <th class="col-act">操作</th>
      </tr>
    </thead>
    <tbody>
      {#if loading}
        <tr><td colspan="7" class="empty">加载中…</td></tr>
      {:else if filtered.length === 0}
        <tr><td colspan="7" class="empty">无匹配连接</td></tr>
      {:else}
        {#each filtered as r}
          <tr>
            <td class="num mono">
              <span class="port">{r.port}</span>
              {#if COMMON_PORTS.has(r.port)}<span class="tag">常用</span>{/if}
            </td>
            <td>
              {r.protocol}
              <span class="fam {r.family === 'IPv6' ? 'v6' : 'v4'}">{r.family === "IPv6" ? "v6" : "v4"}</span>
            </td>
            <td>
              <span class="state {r.state === 'LISTEN' ? 'listen' : r.state === 'ESTABLISHED' ? 'estab' : 'other'}">
                {r.state}
              </span>
            </td>
            <td class="col-proc" title={r.process}>{r.process}</td>
            <td class="num mono">{r.pid}</td>
            <td class="col-remote mono">{r.remote}</td>
            <td class="col-act">
              <button class="danger" on:click={() => (killTarget = r)}>结束</button>
            </td>
          </tr>
        {/each}
      {/if}
    </tbody>
  </table>
</div>

<!-- 结束端口进程确认（含同端口其它绑定分析）-->
<Modal open={!!killTarget} title="结束端口进程" on:close={() => (killTarget = null)}>
  {#if killTarget}
    <p>
      将结束 <strong>{killTarget.process}</strong>（PID {killTarget.pid}），
      它占用 {killTarget.family} 的 {killTarget.protocol} 端口
      <strong>{killTarget.port}</strong>{killTarget.state !== "-" ? `（${killTarget.state}）` : ""}。
    </p>

    {#if killSameProc.length > 0}
      <div class="note-box info">
        同一进程还监听了同端口的其它绑定：
        {#each killSameProc as s}<span class="bind">{s.protocol} {s.family}</span>{/each}
        —— 结束进程会一并释放它们。
      </div>
    {/if}

    {#if killOtherProc.length > 0}
      <div class="note-box warn">
        ⚠ 端口 {killTarget.port} 还被<strong>其它进程</strong>监听：
        {#each killOtherProc as s}<span class="bind">{s.process}(PID {s.pid}) {s.protocol} {s.family}</span>{/each}
        —— 结束本进程<strong>不会</strong>影响它们，端口可能仍被占用。
      </div>
    {/if}

    <p class="muted">未保存数据可能丢失，此操作不可撤销。</p>
    <div class="modal-actions">
      <button class="ghost" on:click={() => (killTarget = null)}>取消</button>
      <button class="danger" on:click={doKill}>确认结束</button>
    </div>
  {/if}
</Modal>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin-bottom: var(--sp-4);
    flex-wrap: wrap;
  }
  .search {
    flex: 0 0 260px;
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
    transition: all 0.15s ease;
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
  .toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--text-muted);
    cursor: pointer;
  }
  .toggle input {
    accent-color: var(--accent);
  }
  .count {
    font-size: 12px;
    color: var(--text-muted);
    font-family: var(--font-mono);
    margin-left: auto;
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
    padding: 10px 14px;
    font-weight: 500;
    color: var(--text-muted);
    white-space: nowrap;
  }
  th.num,
  td.num {
    text-align: right;
  }
  tbody {
    display: block;
    max-height: calc(100vh - 240px);
    overflow-y: auto;
  }
  thead,
  tbody tr {
    display: table;
    width: 100%;
    table-layout: fixed;
  }
  td {
    padding: 8px 14px;
    border-top: 1px solid var(--border);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  tbody tr:hover {
    background: var(--surface-2);
  }
  .mono {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }
  .port {
    font-weight: 600;
  }
  .tag {
    margin-left: 6px;
    padding: 1px 6px;
    border-radius: 4px;
    background: rgba(99, 102, 241, 0.2);
    color: #c7d2fe;
    font-size: 10px;
    font-family: var(--font-sans);
  }
  .state {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    font-family: var(--font-sans);
  }
  .state.listen {
    color: var(--ok);
    background: rgba(34, 197, 94, 0.12);
  }
  .state.estab {
    color: #93c5fd;
    background: rgba(59, 130, 246, 0.14);
  }
  .state.other {
    color: var(--text-muted);
    background: var(--surface-2);
  }
  .empty {
    text-align: center;
    color: var(--text-muted);
    padding: 40px;
  }

  .fam {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 4px;
    margin-left: 4px;
    font-family: var(--font-sans);
    vertical-align: middle;
  }
  .fam.v4 {
    color: #93c5fd;
    background: rgba(59, 130, 246, 0.14);
  }
  .fam.v6 {
    color: #c4b5fd;
    background: rgba(139, 92, 246, 0.16);
  }

  .col-act {
    text-align: right;
    width: 96px;
  }
  td.col-act button {
    font-family: inherit;
    font-size: 12px;
    padding: 4px 10px;
    border-radius: 8px;
    cursor: pointer;
  }
  .danger {
    color: var(--danger);
    border: 1px solid rgba(239, 68, 68, 0.4);
    background: transparent;
    transition: background 0.15s ease;
  }
  .danger:hover {
    background: rgba(239, 68, 68, 0.12);
  }
  .danger:focus-visible {
    outline: 2px solid var(--danger);
    outline-offset: 1px;
  }

  .note-box {
    font-size: 12px;
    line-height: 1.6;
    padding: 8px 10px;
    border-radius: 8px;
    border: 1px solid var(--border);
  }
  .note-box.info {
    color: #bfdbfe;
    background: rgba(59, 130, 246, 0.1);
    border-color: rgba(59, 130, 246, 0.3);
  }
  .note-box.warn {
    color: #fcd34d;
    background: rgba(245, 158, 11, 0.1);
    border-color: rgba(245, 158, 11, 0.35);
  }
  .bind {
    display: inline-block;
    margin: 2px 4px 0 0;
    padding: 1px 6px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.06);
    font-family: var(--font-mono);
    font-size: 11px;
  }
  .muted {
    color: var(--text-muted);
    font-size: 12px;
  }
  .ghost {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    font-family: inherit;
    font-size: 12px;
    padding: 6px 12px;
    border-radius: 8px;
    cursor: pointer;
  }
  .ghost:hover {
    background: var(--surface-2);
  }
  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--sp-2);
  }
</style>
