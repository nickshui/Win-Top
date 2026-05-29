<script>
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { pushToast } from "../stores.js";
  import Modal from "../components/Modal.svelte";

  let rows = [];
  let loading = true;
  let q = "";
  let sortKey = "cpu";
  let sortDir = -1; // -1 降序, 1 升序
  let timer;

  // 弹窗状态
  let termTarget = null; // {pid,name}
  let prioTarget = null;
  const priorities = ["低", "低于普通", "普通", "高于普通", "高", "实时"];

  async function refresh() {
    try {
      rows = await invoke("get_processes");
      loading = false;
    } catch (e) {
      pushToast("加载进程失败：" + e, "error");
    }
  }

  onMount(() => {
    refresh();
    timer = setInterval(refresh, 2000);
  });
  onDestroy(() => clearInterval(timer));

  function setSort(k) {
    if (sortKey === k) sortDir = -sortDir;
    else {
      sortKey = k;
      sortDir = k === "name" ? 1 : -1;
    }
  }

  $: filtered = rows
    .filter((r) => {
      if (!q) return true;
      const s = q.toLowerCase();
      return r.name.toLowerCase().includes(s) || String(r.pid).includes(s);
    })
    .slice()
    .sort((a, b) => {
      if (sortKey === "name") return sortDir * a.name.localeCompare(b.name);
      return sortDir * (a[sortKey] - b[sortKey]);
    });

  const arrow = (k) => (sortKey === k ? (sortDir === -1 ? " ↓" : " ↑") : "");

  async function doTerminate() {
    const t = termTarget;
    termTarget = null;
    const r = await invoke("terminate_process", { pid: t.pid });
    pushToast(r.message, r.success ? "ok" : "error");
    refresh();
  }

  async function doPriority(level) {
    const t = prioTarget;
    prioTarget = null;
    const r = await invoke("set_process_priority", { pid: t.pid, level });
    pushToast(r.message, r.success ? "ok" : "error");
  }

  const sev = (v) => (v >= 50 ? "danger" : v >= 20 ? "warn" : "ok");
</script>

<div class="toolbar">
  <input
    class="search"
    bind:value={q}
    placeholder="按名称 / PID 过滤"
    aria-label="过滤进程"
  />
  <span class="count">{filtered.length} / {rows.length} 进程 · 每 2s 刷新</span>
</div>

<div class="table-wrap">
  <table>
    <thead>
      <tr>
        <th class="col-name" on:click={() => setSort("name")}>进程{arrow("name")}</th>
        <th class="num" on:click={() => setSort("pid")}>PID{arrow("pid")}</th>
        <th class="num" on:click={() => setSort("cpu")}>CPU{arrow("cpu")}</th>
        <th class="num" on:click={() => setSort("mem_mb")}>内存{arrow("mem_mb")}</th>
        <th class="num" on:click={() => setSort("threads")}>线程{arrow("threads")}</th>
        <th class="col-act">操作</th>
      </tr>
    </thead>
    <tbody>
      {#if loading}
        <tr><td colspan="6" class="empty">加载中…</td></tr>
      {:else if filtered.length === 0}
        <tr><td colspan="6" class="empty">无匹配进程</td></tr>
      {:else}
        {#each filtered as p (p.pid)}
          <tr>
            <td class="col-name" title={p.name}>{p.name}</td>
            <td class="num mono">{p.pid}</td>
            <td class="num mono">
              <span class="cpu {sev(p.cpu)}">{p.cpu.toFixed(1)}%</span>
            </td>
            <td class="num mono">{p.mem_mb.toFixed(1)} MB</td>
            <td class="num mono">{p.threads}</td>
            <td class="col-act">
              <button class="ghost" on:click={() => (prioTarget = p)}>优先级</button>
              <button class="danger" on:click={() => (termTarget = p)}>结束</button>
            </td>
          </tr>
        {/each}
      {/if}
    </tbody>
  </table>
</div>

<!-- 结束进程确认 -->
<Modal open={!!termTarget} title="结束进程确认" on:close={() => (termTarget = null)}>
  {#if termTarget}
    <p>
      将结束 <strong>{termTarget.name}</strong>（PID {termTarget.pid}）。
      未保存的数据可能丢失，此操作不可撤销。
    </p>
    <div class="modal-actions">
      <button class="ghost" on:click={() => (termTarget = null)}>取消</button>
      <button class="danger" on:click={doTerminate}>确认结束</button>
    </div>
  {/if}
</Modal>

<!-- 优先级选择 -->
<Modal open={!!prioTarget} title="设置优先级" on:close={() => (prioTarget = null)}>
  {#if prioTarget}
    <p>为 <strong>{prioTarget.name}</strong>（PID {prioTarget.pid}）选择优先级：</p>
    <div class="prio-grid">
      {#each priorities as level}
        <button class="ghost" on:click={() => doPriority(level)}>{level}</button>
      {/each}
    </div>
  {/if}
</Modal>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin-bottom: var(--sp-4);
  }
  .search {
    flex: 0 0 280px;
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
  .count {
    font-size: 12px;
    color: var(--text-muted);
    font-family: var(--font-mono);
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
    cursor: pointer;
    user-select: none;
    white-space: nowrap;
  }
  thead th:hover {
    color: var(--text);
  }
  th.num,
  td.num {
    text-align: right;
  }
  .col-act {
    text-align: right;
    width: 160px;
  }
  tbody {
    display: block;
    max-height: calc(100vh - 230px);
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
  .cpu.ok {
    color: var(--text);
  }
  .cpu.warn {
    color: var(--warn);
  }
  .cpu.danger {
    color: var(--danger);
    font-weight: 600;
  }
  .empty {
    text-align: center;
    color: var(--text-muted);
    padding: 40px;
  }

  button {
    font-family: inherit;
    font-size: 12px;
    padding: 5px 10px;
    border-radius: 8px;
    cursor: pointer;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    transition: background 0.15s ease, border-color 0.15s ease;
  }
  button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .ghost:hover {
    background: var(--surface-2);
  }
  .danger {
    color: var(--danger);
    border-color: rgba(239, 68, 68, 0.4);
  }
  .danger:hover {
    background: rgba(239, 68, 68, 0.12);
  }
  .col-act button + button {
    margin-left: 6px;
  }
  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--sp-2);
  }
  .prio-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: var(--sp-2);
  }
  .prio-grid button {
    padding: 10px;
  }
</style>
