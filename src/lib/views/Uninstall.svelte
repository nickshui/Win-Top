<script>
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { pushToast, elevated, relaunchAdmin } from "../stores.js";
  import Modal from "../components/Modal.svelte";

  let apps = [];
  let loading = true;
  let search = "";
  let sortKey = "score"; // score | name | size | lastUsed
  let sortDir = "desc";

  let selected = null; // ScoredApp { entry, score }
  let footprint = null;
  let fpLoading = false;
  let selectedPaths = new Set();

  let uninstalling = false;
  let removing = false;
  let removeConfirm = false;
  let lastRemoval = null;

  async function load() {
    loading = true;
    try {
      apps = await invoke("list_apps_scored");
    } catch (e) {
      pushToast("读取应用清单失败：" + e, "error");
    } finally {
      loading = false;
    }
  }
  onMount(load);

  $: filtered = filterSort(apps, search, sortKey, sortDir);
  function filterSort(list, q, key, dir) {
    let out = list;
    if (q.trim()) {
      const s = q.toLowerCase();
      out = out.filter((a) =>
        (a.entry.name + " " + a.entry.publisher).toLowerCase().includes(s)
      );
    }
    const sign = dir === "asc" ? 1 : -1;
    const val = (a) =>
      key === "name"
        ? a.entry.name.toLowerCase()
        : key === "size"
        ? a.entry.estimated_size_kb
        : key === "lastUsed"
        ? a.score.last_used_secs ?? 0
        : a.score.removal_recommendation;
    return [...out].sort((x, y) => {
      const vx = val(x),
        vy = val(y);
      if (vx < vy) return -1 * sign;
      if (vx > vy) return 1 * sign;
      return 0;
    });
  }
  function setSort(key) {
    if (sortKey === key) sortDir = sortDir === "asc" ? "desc" : "asc";
    else {
      sortKey = key;
      sortDir = key === "name" ? "asc" : "desc";
    }
  }

  async function selectApp(a) {
    selected = a;
    footprint = null;
    lastRemoval = null;
    await resolveFp();
  }

  async function resolveFp() {
    if (!selected) return;
    fpLoading = true;
    try {
      footprint = await invoke("resolve_footprint", { appId: selected.entry.id });
      // 默认勾选：残留的目录/注册表键，中高置信、非用户数据
      const def = new Set();
      for (const art of footprint.artifacts) {
        if (
          (art.kind === "dir" || art.kind === "reg-key") &&
          art.source.startsWith("residue") &&
          art.confidence !== "low" &&
          art.category !== "user-data"
        ) {
          def.add(art.path);
        }
      }
      selectedPaths = def;
    } catch (e) {
      pushToast("解析产物失败：" + e, "error");
    } finally {
      fpLoading = false;
    }
  }

  function toggle(path) {
    const s = new Set(selectedPaths);
    if (s.has(path)) s.delete(path);
    else s.add(path);
    selectedPaths = s;
  }

  $: removable = footprint
    ? footprint.artifacts.filter((a) => a.kind === "dir" || a.kind === "reg-key")
    : [];
  $: related = footprint
    ? footprint.artifacts.filter((a) => a.kind === "service" || a.kind === "startup")
    : [];
  $: selectedSize = footprint
    ? footprint.artifacts
        .filter((a) => selectedPaths.has(a.path))
        .reduce((s, a) => s + a.size, 0)
    : 0;
  $: selectedHasUserData = footprint
    ? footprint.artifacts.some(
        (a) => selectedPaths.has(a.path) && a.category === "user-data"
      )
    : false;

  async function runUninstaller() {
    if (!selected) return;
    uninstalling = true;
    try {
      const msg = await invoke("run_uninstaller", { appId: selected.entry.id });
      pushToast(msg, "ok");
    } catch (e) {
      pushToast("启动卸载程序失败：" + e, "error");
    } finally {
      uninstalling = false;
    }
  }

  async function doRemove() {
    removeConfirm = false;
    if (!selected) return;
    removing = true;
    try {
      const res = await invoke("remove_residue", {
        appId: selected.entry.id,
        targets: [...selectedPaths],
      });
      lastRemoval = res;
      pushToast(
        `已清理 ${res.removed} 项，释放 ${fmtBytes(res.freed_bytes)}` +
          (res.skipped ? `，跳过 ${res.skipped} 项` : ""),
        "ok"
      );
      if (res.warnings?.length) res.warnings.forEach((w) => pushToast(w, "warn"));
      await resolveFp();
    } catch (e) {
      pushToast("清理失败：" + e, "error");
    } finally {
      removing = false;
    }
  }

  async function doUndo() {
    if (!lastRemoval?.undo_token) return;
    try {
      const msg = await invoke("undo_removal", { undoToken: lastRemoval.undo_token });
      pushToast(msg, "ok");
      lastRemoval = null;
      await resolveFp();
    } catch (e) {
      pushToast("撤销失败：" + e, "error");
    }
  }

  const fmtBytes = (n) => {
    if (!n) return "0 B";
    if (n < 1024) return `${n} B`;
    if (n < 1048576) return `${(n / 1024).toFixed(1)} KB`;
    if (n < 1073741824) return `${(n / 1048576).toFixed(1)} MB`;
    return `${(n / 1073741824).toFixed(2)} GB`;
  };
  const fmtKb = (kb) => fmtBytes((kb || 0) * 1024);
  const scoreClass = (s) => (s >= 70 ? "danger" : s >= 40 ? "warn" : "ok");
  const catLabel = (c) =>
    ({
      "user-data": "用户数据",
      "app-runtime": "运行数据",
      cache: "缓存",
      config: "配置",
      binary: "程序文件",
      unknown: "其他",
    }[c] || c);
  const confLabel = (c) => ({ high: "高", medium: "中", low: "低" }[c] || c);
  const kindLabel = (k) =>
    ({ dir: "目录", "reg-key": "注册表", service: "服务", startup: "启动项" }[k] || k);
</script>

{#if !$elevated}
  <div class="admin-banner">
    <span>卸载 HKLM/系统盘应用及清理受保护位置的残留需要<strong>管理员权限</strong>。</span>
    <button class="elevate-btn" on:click={relaunchAdmin}>以管理员重启</button>
  </div>
{/if}

<div class="ua">
  <!-- 左：应用清单 -->
  <div class="ua-list">
    <div class="ua-toolbar">
      <input class="search" placeholder="搜索应用 / 发行商" bind:value={search} />
      <button class="ghost sm" on:click={load} disabled={loading}>刷新</button>
    </div>
    <div class="sort-row">
      {#each [["score", "推荐度"], ["size", "体积"], ["lastUsed", "最近用"], ["name", "名称"]] as [k, label]}
        <button class="sort" class:active={sortKey === k} on:click={() => setSort(k)}>
          {label}{#if sortKey === k}<span class="arrow">{sortDir === "asc" ? "↑" : "↓"}</span>{/if}
        </button>
      {/each}
    </div>
    <div class="rows">
      {#if loading}
        <div class="empty">加载中…</div>
      {:else if filtered.length === 0}
        <div class="empty">无匹配应用</div>
      {:else}
        {#each filtered as a (a.entry.id)}
          <button
            class="row"
            class:sel={selected?.entry.id === a.entry.id}
            on:click={() => selectApp(a)}
          >
            <span class="score-pill {scoreClass(a.score.removal_recommendation)}">
              {a.score.removal_recommendation}
            </span>
            <div class="row-body">
              <div class="row-top">
                <span class="row-name" title={a.entry.name}>{a.entry.name}</span>
                {#if a.score.caution}<span class="chip caution">谨慎</span>{/if}
                {#if a.score.autostart}<span class="chip">自启</span>{/if}
              </div>
              <div class="row-sub">
                <span class="mono">{fmtKb(a.entry.estimated_size_kb)}</span>
                <span>·</span>
                <span>{a.score.last_used_label}</span>
                {#if a.entry.publisher}<span>·</span><span class="ellip">{a.entry.publisher}</span>{/if}
              </div>
            </div>
          </button>
        {/each}
      {/if}
    </div>
    <div class="list-foot muted">{filtered.length} 个应用</div>
  </div>

  <!-- 右：详情 -->
  <div class="ua-detail">
    {#if !selected}
      <div class="empty big">← 选择左侧应用，查看它的产物分布、使用度与卸载建议</div>
    {:else}
      <div class="det-head">
        <div class="det-title">
          <h3>{selected.entry.name}</h3>
          <p class="muted">
            {selected.entry.publisher || "未知发行商"} · v{selected.entry.version || "—"} ·
            {fmtKb(selected.entry.estimated_size_kb)}
            {#if selected.score.run_count > 0} · 运行过 {selected.score.run_count} 次{/if}
          </p>
        </div>
        <div class="det-score">
          <span class="score-pill big {scoreClass(selected.score.removal_recommendation)}">
            {selected.score.removal_recommendation}
          </span>
          <span class="score-cap muted">推荐卸载度</span>
        </div>
      </div>

      {#if selected.score.reasons.length}
        <div class="reasons">
          {#each selected.score.reasons as r}<span class="reason">{r}</span>{/each}
        </div>
      {/if}

      <div class="det-actions">
        <button class="ghost" on:click={runUninstaller} disabled={uninstalling}>
          {uninstalling ? "启动中…" : "运行卸载程序"}
        </button>
        <button class="ghost" on:click={resolveFp} disabled={fpLoading}>
          {fpLoading ? "扫描中…" : "重新扫描产物"}
        </button>
        <button
          class="primary"
          on:click={() => (removeConfirm = true)}
          disabled={removing || selectedPaths.size === 0}
        >
          {removing ? "清理中…" : `清理选中残留 (${selectedPaths.size})`}
        </button>
      </div>

      <p class="flow-hint muted">
        建议流程：先「运行卸载程序」走厂商卸载 → 完成后「重新扫描产物」→ 勾选残留「清理」。
      </p>

      {#if lastRemoval}
        <div class="undo-bar">
          <span>本次已清理 {lastRemoval.removed} 项 · 释放 {fmtBytes(lastRemoval.freed_bytes)}（已进隔离区）</span>
          <button class="ghost sm" on:click={doUndo}>撤销本次清理</button>
        </div>
        <p class="undo-note muted">撤销只还原本工具移入隔离区的项；厂商卸载器删掉的文件不在其中。</p>
      {/if}

      {#if fpLoading && !footprint}
        <div class="empty">正在解析产物…</div>
      {:else if footprint}
        {#if footprint.app_removed}
          <div class="removed-banner">
            ✓ 该应用已从系统卸载。下面是磁盘/注册表上<strong>仍残留</strong>的产物，可勾选清理；厂商卸载器已删掉的项不再显示。
          </div>
        {/if}
        {#if footprint.warnings.length}
          <div class="warns">
            {#each footprint.warnings as w}<div class="warn-row">⚠ {w}</div>{/each}
          </div>
        {/if}

        <div class="tree-head">
          <span>可清理产物</span>
          <span class="muted sm">删到隔离区，可一键撤销</span>
        </div>
        {#if removable.length === 0}
          <div class="empty">未发现可清理的目录/注册表残留</div>
        {:else}
          <div class="tree">
            {#each removable as art (art.path)}
              <label class="art" class:userdata={art.category === "user-data"}>
                <input
                  type="checkbox"
                  checked={selectedPaths.has(art.path)}
                  on:change={() => toggle(art.path)}
                />
                <span class="art-badges">
                  <span class="badge kind">{kindLabel(art.kind)}</span>
                  <span class="badge cat-{art.category}">{catLabel(art.category)}</span>
                  <span class="badge conf-{art.confidence}">{confLabel(art.confidence)}置信</span>
                </span>
                <span class="art-path mono" title={art.path}>{art.path}</span>
                <span class="art-size mono">{art.size ? fmtBytes(art.size) : ""}</span>
                {#if art.match_reason}<span class="art-reason" title={art.match_reason}>{art.match_reason}</span>{/if}
              </label>
            {/each}
          </div>
        {/if}

        {#if related.length}
          <div class="tree-head">
            <span>关联项</span>
            <span class="muted sm">由卸载程序处理，本工具 v1 不自动删除</span>
          </div>
          <div class="tree">
            {#each related as art (art.path)}
              <div class="art readonly">
                <span class="art-badges"><span class="badge kind">{kindLabel(art.kind)}</span></span>
                <span class="art-path mono">{art.path}</span>
                <span class="art-reason" title={art.match_reason}>{art.match_reason}</span>
              </div>
            {/each}
          </div>
        {/if}
      {/if}
    {/if}
  </div>
</div>

<Modal open={removeConfirm} title="确认清理残留" on:close={() => (removeConfirm = false)}>
  <p>
    将把选中的 <b>{selectedPaths.size}</b> 项（约 <b>{fmtBytes(selectedSize)}</b>）移入隔离区。
    文件/目录可一键撤销；注册表键会先导出 <span class="mono">.reg</span> 备份再删除。
  </p>
  {#if selectedHasUserData}
    <p class="warn-text">
      ⚠ 选中项包含<strong>用户数据</strong>，删除后你的作品/存档可能丢失，请再次确认。
    </p>
  {/if}
  <p class="muted">仅清理本工具解析到、且不在系统保护目录内的产物；其余会被安全跳过。</p>
  <div class="modal-actions">
    <button class="ghost" on:click={() => (removeConfirm = false)}>取消</button>
    <button class="primary" on:click={doRemove}>确认清理</button>
  </div>
</Modal>

<style>
  .admin-banner {
    display: flex; align-items: center; gap: var(--sp-4); flex-wrap: wrap;
    padding: 10px 14px; margin-bottom: var(--sp-4);
    border: 1px solid rgba(245, 158, 11, 0.35); background: rgba(245, 158, 11, 0.1);
    border-radius: var(--radius-sm); font-size: 13px; color: #fcd34d;
  }
  .elevate-btn {
    font-size: 12px; padding: 5px 12px; border-radius: var(--radius-sm);
    border: 1px solid rgba(245, 158, 11, 0.45); background: rgba(245, 158, 11, 0.1);
    color: var(--warn); font-family: inherit; cursor: pointer;
  }

  .ua { display: flex; gap: var(--sp-4); height: calc(100vh - 150px); }
  .ua-list {
    width: 360px; flex-shrink: 0; display: flex; flex-direction: column;
    border: 1px solid var(--border); border-radius: var(--radius); background: var(--surface);
    overflow: hidden;
  }
  .ua-toolbar { display: flex; gap: var(--sp-2); padding: var(--sp-3); border-bottom: 1px solid var(--border); }
  .search {
    flex: 1; background: var(--surface-2); border: 1px solid var(--border); border-radius: var(--radius-sm);
    padding: 7px 10px; color: var(--text); font-family: inherit; font-size: 13px;
  }
  .search:focus { outline: none; border-color: var(--accent); }
  .sort-row { display: flex; gap: 2px; padding: var(--sp-2) var(--sp-3); border-bottom: 1px solid var(--border); }
  .sort {
    flex: 1; border: none; background: transparent; color: var(--text-muted);
    font-family: inherit; font-size: 12px; padding: 4px 6px; border-radius: var(--radius-sm); cursor: pointer;
  }
  .sort:hover { background: var(--surface-2); color: var(--text); }
  .sort.active { color: var(--text); background: var(--surface-2); }
  .arrow { margin-left: 2px; }

  .rows { flex: 1; overflow-y: auto; }
  .row {
    display: flex; align-items: center; gap: var(--sp-3); width: 100%;
    padding: 9px 12px; border: none; border-bottom: 1px solid var(--border);
    background: transparent; color: var(--text); font-family: inherit; cursor: pointer; text-align: left;
  }
  .row:hover { background: var(--surface-2); }
  .row.sel { background: var(--surface-2); box-shadow: inset 3px 0 0 var(--accent); }
  .row-body { min-width: 0; flex: 1; }
  .row-top { display: flex; align-items: center; gap: 6px; }
  .row-name { font-size: 13px; font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .row-sub { display: flex; gap: 5px; font-size: 11px; color: var(--text-muted); margin-top: 2px; align-items: center; }
  .row-sub .ellip { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .list-foot { padding: 6px 12px; border-top: 1px solid var(--border); font-size: 11px; }

  .score-pill {
    flex-shrink: 0; min-width: 30px; text-align: center; font-family: var(--font-mono);
    font-weight: 700; font-size: 13px; padding: 3px 6px; border-radius: var(--radius-sm);
  }
  .score-pill.ok { color: var(--ok); background: rgba(34, 197, 94, 0.12); }
  .score-pill.warn { color: var(--warn); background: rgba(245, 158, 11, 0.14); }
  .score-pill.danger { color: var(--danger); background: rgba(239, 68, 68, 0.14); }
  .score-pill.big { font-size: 20px; min-width: 46px; padding: 6px 10px; }

  .chip { font-size: 10px; padding: 1px 6px; border-radius: 999px; background: var(--surface-2); color: var(--text-muted); border: 1px solid var(--border); }
  .chip.caution { color: var(--warn); border-color: rgba(245, 158, 11, 0.4); }

  .ua-detail {
    flex: 1; min-width: 0; overflow-y: auto;
    border: 1px solid var(--border); border-radius: var(--radius); background: var(--surface); padding: var(--sp-5);
  }
  .empty { text-align: center; color: var(--text-muted); padding: 30px; font-size: 13px; }
  .empty.big { padding: 80px 20px; }
  .muted { color: var(--text-muted); }
  .sm { font-size: 12px; }
  .mono { font-family: var(--font-mono); font-variant-numeric: tabular-nums; }

  .det-head { display: flex; justify-content: space-between; align-items: flex-start; gap: var(--sp-4); }
  .det-title h3 { margin: 0; font-size: 18px; }
  .det-title p { margin: 4px 0 0; font-size: 12px; }
  .det-score { display: flex; flex-direction: column; align-items: center; gap: 2px; flex-shrink: 0; }
  .score-cap { font-size: 11px; }

  .reasons { display: flex; flex-wrap: wrap; gap: 6px; margin: var(--sp-4) 0; }
  .reason { font-size: 12px; padding: 3px 10px; border-radius: 999px; background: var(--surface-2); border: 1px solid var(--border); color: var(--text); }

  .det-actions { display: flex; gap: var(--sp-2); flex-wrap: wrap; margin-top: var(--sp-2); }
  .flow-hint { font-size: 12px; margin: var(--sp-3) 0 0; }

  .undo-bar {
    display: flex; align-items: center; justify-content: space-between; gap: var(--sp-3);
    margin-top: var(--sp-4); padding: 9px 12px; border-radius: var(--radius-sm);
    background: rgba(34, 197, 94, 0.1); border: 1px solid rgba(34, 197, 94, 0.3); font-size: 13px;
  }

  .removed-banner {
    margin-top: var(--sp-4); padding: 10px 14px; border-radius: var(--radius-sm);
    background: rgba(34, 197, 94, 0.12); border: 1px solid rgba(34, 197, 94, 0.35);
    color: #86efac; font-size: 13px; line-height: 1.5;
  }
  .removed-banner strong { color: var(--text); }
  .undo-note { font-size: 12px; margin: 6px 2px 0; }

  .warns { margin-top: var(--sp-4); }
  .warn-row { font-size: 12px; color: var(--warn); padding: 3px 0; }

  .tree-head {
    display: flex; align-items: baseline; gap: var(--sp-2); margin: var(--sp-5) 0 var(--sp-2);
    font-size: 13px; font-weight: 600; border-bottom: 1px solid var(--border); padding-bottom: 6px;
  }
  .tree { display: flex; flex-direction: column; gap: 4px; }
  .art {
    display: grid; grid-template-columns: auto auto 1fr auto; align-items: center; gap: var(--sp-2);
    padding: 7px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm);
    background: var(--surface-2); font-size: 12px; cursor: pointer;
  }
  .art.readonly { grid-template-columns: auto 1fr auto; cursor: default; opacity: 0.85; }
  .art.userdata { border-color: rgba(239, 68, 68, 0.4); background: rgba(239, 68, 68, 0.06); }
  .art input { accent-color: var(--accent); }
  .art-badges { display: flex; gap: 4px; flex-shrink: 0; }
  .badge { font-size: 10px; padding: 1px 6px; border-radius: 4px; background: var(--surface); border: 1px solid var(--border); color: var(--text-muted); white-space: nowrap; }
  .badge.kind { color: var(--text); }
  .cat-user-data { color: var(--danger); border-color: rgba(239, 68, 68, 0.4); }
  .cat-cache { color: var(--ok); }
  .conf-high { color: var(--ok); }
  .conf-medium { color: var(--warn); }
  .conf-low { color: var(--text-muted); }
  .art-path { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
  .art-size { color: var(--text-muted); white-space: nowrap; }
  .art-reason {
    grid-column: 3 / -1; font-size: 11px; color: var(--text-muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }

  .warn-text { color: var(--warn); font-size: 13px; }
  .warn-text strong { color: var(--danger); }
  .modal-actions { display: flex; justify-content: flex-end; gap: var(--sp-2); margin-top: var(--sp-3); }
  .primary { border: none; background: linear-gradient(135deg, var(--accent), #7c3aed); color: #fff; font-family: inherit; font-size: 13px; padding: 8px 16px; border-radius: var(--radius-sm); cursor: pointer; }
  .primary:disabled { opacity: 0.55; cursor: default; }
  .ghost { border: 1px solid var(--border); background: transparent; color: var(--text); font-family: inherit; font-size: 13px; padding: 7px 14px; border-radius: var(--radius-sm); cursor: pointer; }
  .ghost:hover { background: var(--surface-2); }
  .ghost.sm { padding: 5px 10px; font-size: 12px; }
  .ghost:disabled { opacity: 0.55; cursor: default; }
</style>
