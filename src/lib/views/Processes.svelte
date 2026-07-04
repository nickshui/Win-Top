<script>
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { pushToast, diskIo } from "../stores.js";
  import Modal from "../components/Modal.svelte";

  let rows = [];
  let loading = true;
  let q = "";
  let sortKey = "cpu";
  let sortDir = -1;
  let timer;
  let viewMode = "list";

  // 详情面板
  let detailPid = null;
  let detailData = null;
  let detailLoading = false;

  // 弹窗状态
  let termTarget = null;
  let prioTarget = null;
  const priorities = ["低", "低于普通", "普通", "高于普通", "高", "实时"];

  // 树形展开状态
  let expanded = new Set();
  let allExpanded = false;

  async function refresh() {
    try {
      rows = await invoke("get_processes");
      loading = false;
    } catch (e) {
      pushToast("加载进程失败：" + e, "error");
    }
  }

  async function showDetail(pid) {
    if (detailPid === pid) { detailPid = null; detailData = null; return; }
    detailPid = pid;
    detailLoading = true;
    detailData = null;
    try {
      detailData = await invoke("get_process_detail", { pid });
    } catch (e) {
      pushToast("获取进程详情失败：" + e, "error");
      detailPid = null;
    } finally { detailLoading = false; }
  }

  onMount(() => { refresh(); timer = setInterval(refresh, 2000); });
  onDestroy(() => clearInterval(timer));

  function setSort(k) {
    if (sortKey === k) sortDir = -sortDir;
    else { sortKey = k; sortDir = k === "name" ? 1 : -1; }
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

  // ——— 树形构建 ———
  $: treeRoots = buildTree(merged);

  function buildTree(procs) {
    const map = new Map();
    const roots = [];
    for (const p of procs) { p.children = []; map.set(p.pid, p); }
    for (const p of procs) {
      const parent = map.get(p.parent_pid);
      if (parent && parent !== p) { parent.children.push(p); }
      else { roots.push(p); }
    }
    for (const [_, p] of map) { p.children.sort((a, b) => b.cpu - a.cpu); }
    roots.sort((a, b) => b.cpu - a.cpu);
    return roots;
  }

  // 可折叠展平
  $: flatTree = flattenTree(treeRoots, 0, expanded);

  function flattenTree(nodes, depth, expandedSet) {
    let result = [];
    for (const node of nodes) {
      const hasKids = node.children && node.children.length > 0;
      const isOpen = expandedSet.has(node.pid);
      result.push({ ...node, _depth: depth, _hasChildren: hasKids, _open: isOpen });
      if (hasKids && isOpen) {
        result = result.concat(flattenTree(node.children, depth + 1, expandedSet));
      }
    }
    return result;
  }

  function toggleExpand(pid) {
    const next = new Set(expanded);
    if (next.has(pid)) next.delete(pid); else next.add(pid);
    expanded = next;
    allExpanded = false;
  }

  function expandAll() {
    const next = new Set();
    for (const p of merged) {
      if (p.children && p.children.length > 0) next.add(p.pid);
    }
    expanded = next;
    allExpanded = true;
  }

  function collapseAll() {
    expanded = new Set();
    allExpanded = false;
  }

  const arrow = (k) => (sortKey === k ? (sortDir === -1 ? " ↓" : " ↑") : "");

  async function doTerminate() {
    const t = termTarget; termTarget = null;
    try {
      const r = await invoke("terminate_process", { pid: t.pid });
      pushToast(r.message, r.success ? "ok" : "error");
      refresh();
      if (detailPid === t.pid) { detailPid = null; detailData = null; }
    } catch (e) { pushToast("结束进程失败：" + e, "error"); }
  }

  async function doPriority(level) {
    const t = prioTarget; prioTarget = null;
    try {
      const r = await invoke("set_process_priority", { pid: t.pid, level });
      pushToast(r.message, r.success ? "ok" : "error");
    } catch (e) { pushToast("设置优先级失败：" + e, "error"); }
  }

  const sev = (v) => (v >= 50 ? "danger" : v >= 20 ? "warn" : "ok");

  // Disk I/O map
  $: diskIoMap = new Map(($diskIo?.rows ?? []).map(r => [r.pid, r]));
  const fmtIO = (v) => {
    if (!v || v < 1024) return (v || 0).toFixed(0) + " B";
    if (v < 1024 * 1024) return (v / 1024).toFixed(1) + " K";
    return (v / 1024 / 1024).toFixed(1) + " M";
  };

  // 合并进程列表与磁盘 I/O，使其可排序
  $: merged = filtered.map(p => ({
    ...p,
    read_bps: diskIoMap.get(p.pid)?.read_bps ?? 0,
    write_bps: diskIoMap.get(p.pid)?.write_bps ?? 0,
  })).sort((a, b) => {
    if (sortKey === "name") return sortDir * a.name.localeCompare(b.name);
    if (sortKey === "read_bps" || sortKey === "write_bps") return sortDir * (a[sortKey] - b[sortKey]);
    return sortDir * ((a[sortKey] ?? 0) - (b[sortKey] ?? 0));
  });
</script>

<div class="toolbar">
  <div class="view-toggle">
    <button class="toggle-btn" class:active={viewMode === "list"} on:click={() => (viewMode = "list")}>列表</button>
    <button class="toggle-btn" class:active={viewMode === "tree"} on:click={() => (viewMode = "tree")}>树形</button>
  </div>
  <input class="search" bind:value={q} placeholder="按名称 / PID 过滤" aria-label="过滤进程" />
  {#if viewMode === "tree"}
    {#if allExpanded}
      <button class="ghost-sm" on:click={collapseAll}>折叠全部</button>
    {:else}
      <button class="ghost-sm" on:click={expandAll}>展开全部</button>
    {/if}
  {/if}
  <span class="count">{merged.length} / {rows.length} 进程 · 每 2s 刷新</span>
</div>

<div class="content-area">
  <div class="table-wrap" class:with-detail={!!detailPid}>
    <table>
      <thead>
        <tr>
          <th class="col-name" on:click={() => setSort("name")}>进程{arrow("name")}</th>
          <th class="col-pid num" on:click={() => setSort("pid")}>PID{arrow("pid")}</th>
          <th class="col-cpu num" on:click={() => setSort("cpu")}>CPU{arrow("cpu")}</th>
          <th class="col-mem num" on:click={() => setSort("mem_mb")}>内存{arrow("mem_mb")}</th>
          <th class="col-thr num" on:click={() => setSort("threads")}>线程{arrow("threads")}</th>
          <th class="col-ior num" on:click={() => setSort("read_bps")}><span class="io-hdr-r">磁盘R</span>{arrow("read_bps")}</th>
          <th class="col-iow num" on:click={() => setSort("write_bps")}><span class="io-hdr-w">磁盘W</span>{arrow("write_bps")}</th>
          <th class="col-act">操作</th>
        </tr>
      </thead>
      <tbody>
        {#if loading}
          <tr><td colspan="8" class="empty">加载中…</td></tr>
        {:else if filtered.length === 0}
          <tr><td colspan="8" class="empty">无匹配进程</td></tr>
        {:else if viewMode === "tree"}
          {#each flatTree as p (p.pid)}
            <tr class="tree-row" class:active-row={detailPid === p.pid} on:click={() => showDetail(p.pid)}>
              <td class="col-name" title={p.name}>
                <span style="padding-left:{p._depth * 18}px">
                  {#if p._hasChildren}
                    <button class="tree-arrow" class:open={p._open} on:click|stopPropagation={() => toggleExpand(p.pid)} aria-label={p._open ? '折叠' : '展开'}>{p._open ? '▼' : '▶'}</button>
                  {:else if p._depth > 0}
                    <span class="tree-arrow">├</span>
                  {:else}
                    <span class="tree-arrow"></span>
                  {/if}
                  {p.name}
                </span>
              </td>
              <td class="col-pid num mono">{p.pid}</td>
              <td class="col-cpu num mono"><span class="cpu {sev(p.cpu)}">{p.cpu.toFixed(1)}%</span></td>
              <td class="col-mem num mono">{p.mem_mb.toFixed(1)} MB</td>
              <td class="col-thr num mono">{p.threads}</td>
              <td class="col-ior num mono io-r">{fmtIO(p.read_bps)}</td>
              <td class="col-iow num mono io-w">{fmtIO(p.write_bps)}</td>
              <td class="col-act" on:click|stopPropagation>
                <button class="ghost" on:click={() => (prioTarget = p)}>优先级</button>
                <button class="danger" on:click={() => (termTarget = p)}>结束</button>
              </td>
            </tr>
          {/each}
        {:else}
          {#each merged as p (p.pid)}
            <tr class:active-row={detailPid === p.pid} on:click={() => showDetail(p.pid)}>
              <td class="col-name" title={p.name}>{p.name}</td>
              <td class="col-pid num mono">{p.pid}</td>
              <td class="col-cpu num mono"><span class="cpu {sev(p.cpu)}">{p.cpu.toFixed(1)}%</span></td>
              <td class="col-mem num mono">{p.mem_mb.toFixed(1)} MB</td>
              <td class="col-thr num mono">{p.threads}</td>
              <td class="col-ior num mono io-r">{fmtIO(p.read_bps)}</td>
              <td class="col-iow num mono io-w">{fmtIO(p.write_bps)}</td>
              <td class="col-act" on:click|stopPropagation>
                <button class="ghost" on:click={() => (prioTarget = p)}>优先级</button>
                <button class="danger" on:click={() => (termTarget = p)}>结束</button>
              </td>
            </tr>
          {/each}
        {/if}
      </tbody>
    </table>
  </div>

  {#if detailPid}
    <aside class="detail-panel">
      <button class="close-btn" on:click={() => { detailPid = null; detailData = null; }}>✕</button>
      {#if detailLoading}
        <p class="muted">加载详情…</p>
      {:else if detailData}
        <div class="detail-header">
          <h3 class="detail-name">{detailData.name}</h3>
          <span class="mono">PID {detailData.pid}</span>
        </div>
        <dl class="detail-grid">
          <dt>命令行</dt><dd class="mono cmd">{detailData.command_line || "(无)"}</dd>
          <dt>完整路径</dt><dd class="mono path-text">{detailData.full_path || "(无)"}</dd>
          <dt>父进程 PID</dt><dd class="mono">{detailData.parent_pid || 0}</dd>
          <dt>CPU 使用率</dt><dd class="mono">{detailData.cpu?.toFixed(1) ?? "0.0"}%</dd>
          <dt>内存占用</dt><dd class="mono">{detailData.mem_mb?.toFixed(1) ?? "0.0"} MB</dd>
          <dt>线程数</dt><dd class="mono">{detailData.threads ?? 0}</dd>
          <dt>句柄数</dt><dd class="mono">{detailData.handles ?? 0}</dd>
          <dt>优先级</dt><dd class="mono">{detailData.priority ?? "-"}</dd>
        </dl>
        <div class="detail-actions">
          <button class="ghost" on:click={() => (prioTarget = detailData)}>优先级</button>
          <button class="danger" on:click={() => (termTarget = detailData)}>结束进程</button>
        </div>
      {:else}
        <p class="muted">获取详情失败</p>
      {/if}
    </aside>
  {/if}
</div>

<Modal open={!!termTarget} title="结束进程确认" on:close={() => (termTarget = null)}>
  {#if termTarget}
    <p>将结束 <strong>{termTarget.name}</strong>（PID {termTarget.pid}）。未保存的数据可能丢失，此操作不可撤销。</p>
    <div class="modal-actions">
      <button class="ghost" on:click={() => (termTarget = null)}>取消</button>
      <button class="danger" on:click={doTerminate}>确认结束</button>
    </div>
  {/if}
</Modal>

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
  .toolbar { display: flex; align-items: center; gap: var(--sp-3); margin-bottom: var(--sp-4); flex-wrap: wrap; }
  .view-toggle { display: flex; border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: hidden; }
  .toggle-btn { border: none; background: transparent; color: var(--text-muted); font-family: inherit; font-size: 12px; padding: 6px 12px; cursor: pointer; }
  .toggle-btn.active { background: var(--surface-2); color: var(--text); }
  .search { flex: 0 0 200px; padding: 8px 12px; border-radius: var(--radius-sm); border: 1px solid var(--border); background: var(--surface); color: var(--text); font-family: inherit; font-size: 13px; }
  .search:focus-visible { outline: 2px solid var(--accent); outline-offset: 1px; }
  .ghost-sm { border: 1px solid var(--border); background: transparent; color: var(--text-muted); font-family: inherit; font-size: 12px; padding: 4px 10px; border-radius: 8px; cursor: pointer; }
  .ghost-sm:hover { background: var(--surface-2); color: var(--text); }
  .count { font-size: 12px; color: var(--text-muted); font-family: var(--font-mono); margin-left: auto; }

  .content-area { display: flex; gap: 0; }
  .table-wrap { flex: 1; min-width: 0; border: 1px solid var(--border); border-radius: var(--radius); overflow: hidden; background: var(--surface); }
  .table-wrap.with-detail { border-radius: var(--radius) 0 0 var(--radius); border-right: none; }

  table { width: 100%; border-collapse: collapse; font-size: 13px; table-layout: fixed; }
  thead { }
  thead th { position: sticky; top: 0; background: var(--surface-2); text-align: left; padding: 10px 8px; font-weight: 500; color: var(--text-muted); cursor: pointer; user-select: none; white-space: nowrap; border-bottom: 1px solid var(--border); z-index: 1; }
  thead th:hover { color: var(--text); }
  thead th:first-child { padding-left: 14px; }
  thead th:last-child { padding-right: 14px; }
  tbody { display: block; max-height: calc(100vh - 240px); overflow-y: auto; }
  tr { display: table; width: 100%; table-layout: fixed; }
  td { padding: 6px 8px; border-top: 1px solid var(--border); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  td:first-child { padding-left: 14px; }
  td:last-child { padding-right: 14px; }
  tbody tr:hover { background: var(--surface-2); }
  tbody tr.active-row { background: rgba(99, 102, 241, 0.12); }

  /* 固定列宽 —— 精确到 px，保证表头表体对齐 */
  .col-name { width: 44%; }
  .col-pid { width: 58px; }
  .col-cpu { width: 62px; }
  .col-mem { width: 80px; }
  .col-thr { width: 54px; }
  .col-ior { width: 68px; }
  .col-iow { width: 68px; }
  .col-act { width: 146px; }
  th.num, td.num { text-align: right; }
  .col-act { text-align: right; }

  .tree-row { cursor: pointer; }
  .tree-arrow { display: inline-block; width: 18px; height: 18px; text-align: center; color: var(--text-muted); font-size: 10px; cursor: pointer; user-select: none; flex-shrink: 0; padding: 0; border: none; background: transparent; line-height: 18px; border-radius: 4px; vertical-align: middle; }
  .tree-arrow:hover { color: var(--text); background: var(--surface-2); }
  .tree-arrow.open { color: var(--accent); }
  .mono { font-family: var(--font-mono); font-variant-numeric: tabular-nums; }
  .cpu.ok { color: var(--text); }
  .cpu.warn { color: var(--warn); }
  .cpu.danger { color: var(--danger); font-weight: 600; }
  .empty { text-align: center; color: var(--text-muted); padding: 40px; }

  .io-hdr-r { color: var(--ok); font-size: 10px; }
  .io-hdr-w { color: var(--warn); font-size: 10px; }
  .io-r { color: var(--ok); font-size: 11px; }
  .io-w { color: var(--warn); font-size: 11px; }

  .detail-panel { width: 360px; flex-shrink: 0; background: var(--surface); border: 1px solid var(--border); border-left: none; border-radius: 0 var(--radius) var(--radius) 0; padding: var(--sp-4); overflow-y: auto; max-height: calc(100vh - 230px); position: relative; }
  .close-btn { position: absolute; top: var(--sp-3); right: var(--sp-3); border: none; background: transparent; color: var(--text-muted); font-size: 16px; cursor: pointer; padding: 4px 8px; border-radius: 4px; }
  .close-btn:hover { background: var(--surface-2); color: var(--text); }
  .detail-header { margin-bottom: var(--sp-4); }
  .detail-name { margin: 0; font-size: 16px; font-weight: 600; }
  .detail-grid { display: grid; grid-template-columns: auto 1fr; gap: 8px 12px; font-size: 13px; margin: 0; }
  .detail-grid dt { color: var(--text-muted); }
  .detail-grid dd { margin: 0; word-break: break-all; }
  .cmd, .path-text { font-size: 12px; line-height: 1.4; }
  .detail-actions { display: flex; gap: var(--sp-2); margin-top: var(--sp-4); }
  .muted { color: var(--text-muted); }

  button { font-family: inherit; font-size: 12px; padding: 5px 10px; border-radius: 8px; cursor: pointer; border: 1px solid var(--border); background: transparent; color: var(--text); transition: background 0.15s ease, border-color 0.15s ease; }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 1px; }
  .ghost:hover { background: var(--surface-2); }
  .danger { color: var(--danger); border-color: rgba(239, 68, 68, 0.4); }
  .danger:hover { background: rgba(239, 68, 68, 0.12); }
  .col-act button + button { margin-left: 4px; }
  .modal-actions { display: flex; justify-content: flex-end; gap: var(--sp-2); }
  .prio-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: var(--sp-2); }
  .prio-grid button { padding: 10px; }
</style>
