<script>
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { open as shellOpen } from "@tauri-apps/api/shell";
  import { pushToast, elevated, relaunchAdmin } from "../stores.js";
  import Modal from "../components/Modal.svelte";
  import UninstallWorkflow from "../components/uninstall/UninstallWorkflow.svelte";
  import {
    canOpenArtifact,
    createWorkflow,
    transition,
    defaultSelectedPaths,
    isCurrentRequest,
  } from "../uninstallWorkflow.js";

  let apps = [];
  let loading = true;
  let listError = "";
  let search = "";
  let sortKey = "score";
  let sortDir = "desc";
  let filters = new Set();
  let icons = new Map(); // app id -> data URL

  let workflow = createWorkflow();
  let selectedPaths = new Set();
  let capability = null;
  let uninstallConfirm = false;
  let cleanupConfirm = false;
  let userDataAcknowledged = false;
  let statusNote = "";
  let requestSeq = 0;
  let pollTimer = null;
  let checking = false;
  let undoing = false;
  let footprintCache = new Map();

  $: filtered = filterSort(apps, search, sortKey, sortDir, filters);
  $: flowLocked =
    ["preparing", "launching", "waiting", "scanning", "review", "cleaning", "complete"].includes(workflow.phase) ||
    (workflow.phase === "error" && workflow.recoverPhase === "waiting");
  $: artifacts = workflow.footprint?.artifacts || [];
  $: selectedArtifacts = artifacts.filter((item) => selectedPaths.has(item.path));
  $: selectedSize = selectedArtifacts.reduce((sum, item) => sum + (item.size || 0), 0);
  $: selectedUserData = selectedArtifacts.filter((item) => item.category === "user-data");

  async function load() {
    loading = true;
    listError = "";
    try {
      const next = await invoke("list_apps_scored");
      apps = next;
      if (workflow.app && !flowLocked) {
        const refreshed = next.find((item) => item.entry.id === workflow.app.entry.id);
        if (refreshed) workflow = { ...workflow, app: refreshed };
      }
      loadIcons(next);
    } catch (error) {
      listError = "读取应用清单失败：" + error;
      pushToast(listError, "error");
    } finally {
      loading = false;
    }
  }

  // 图标按需批量提取；失败的应用回退为首字母头像（不影响列表可用性）。
  async function loadIcons(list) {
    try {
      const items = list.map(({ entry }) => ({
        id: entry.id,
        display_icon: entry.display_icon || "",
        install_location: entry.install_location || "",
      }));
      if (items.length === 0) return;
      const map = await invoke("get_app_icons", { items });
      icons = new Map(Object.entries(map || {}));
    } catch (e) {
      // 图标是增强信息，失败静默
    }
  }

  onMount(load);
  onDestroy(stopPolling);

  function filterSort(list, query, key, direction, activeFilters) {
    const needle = query.trim().toLowerCase();
    let out = needle
      ? list.filter((item) =>
          `${item.entry.name} ${item.entry.publisher} ${item.entry.version} ${item.entry.install_location}`
            .toLowerCase()
            .includes(needle)
        )
      : list;
    if (activeFilters.size > 0) {
      out = out.filter((item) => {
        if (activeFilters.has("autostart") && !item.score.autostart) return false;
        if (activeFilters.has("caution") && !item.score.caution) return false;
        if (activeFilters.has("unused") && item.score.last_used_secs != null) return false;
        if (activeFilters.has("large") && item.entry.estimated_size_kb < 1024 * 1024) return false;
        return true;
      });
    }
    const sign = direction === "asc" ? 1 : -1;
    const value = (item) =>
      key === "name"
        ? item.entry.name.toLowerCase()
        : key === "size"
        ? item.entry.estimated_size_kb
        : key === "lastUsed"
        ? item.score.last_used_secs ?? 0
        : key === "installed"
        ? parseInt(item.entry.install_date, 10) || 0
        : item.score.removal_recommendation;
    return [...out].sort((left, right) => {
      const a = value(left);
      const b = value(right);
      return a < b ? -1 * sign : a > b ? 1 * sign : 0;
    });
  }

  function toggleFilter(key) {
    if (flowLocked) return;
    const next = new Set(filters);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    filters = next;
  }

  // 列表键盘导航：↑/↓/Home/End 切换选中应用
  function onRowKeydown(event) {
    if (flowLocked) return;
    const keys = ["ArrowDown", "ArrowUp", "Home", "End"];
    if (!keys.includes(event.key) || filtered.length === 0) return;
    event.preventDefault();
    const current = filtered.findIndex((item) => item.entry.id === workflow.app?.entry.id);
    let nextIndex;
    if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = filtered.length - 1;
    else nextIndex = current < 0 ? 0 : Math.min(filtered.length - 1, Math.max(0, current + (event.key === "ArrowDown" ? 1 : -1)));
    const target = filtered[nextIndex];
    selectApp(target);
    requestAnimationFrame(() => {
      const rows = document.querySelectorAll("[data-app-id]");
      for (const row of rows) {
        if (row.dataset.appId === target.entry.id) {
          row.scrollIntoView({ block: "nearest" });
          break;
        }
      }
    });
  }

  function setSort(key) {
    if (flowLocked) return;
    if (sortKey === key) sortDir = sortDir === "asc" ? "desc" : "asc";
    else {
      sortKey = key;
      sortDir = key === "name" ? "asc" : "desc";
    }
  }

  function selectApp(app) {
    if (flowLocked) return;
    const seq = ++requestSeq;
    stopPolling();
    capability = null;
    uninstallConfirm = false;
    cleanupConfirm = false;
    userDataAcknowledged = false;
    statusNote = "";
    selectedPaths = new Set();
    workflow = createWorkflow(app, footprintCache.get(app.entry.id) || null);
    analyzeApp(app, seq);
  }

  async function analyzeApp(app, seq) {
    const appId = app.entry.id;
    try {
      const footprint = await invoke("resolve_footprint", { appId });
      if (!isCurrentRequest(workflow.app?.entry.id, requestSeq, appId, seq)) return;
      footprintCache = new Map(footprintCache).set(appId, footprint);
      workflow = transition(workflow, "ANALYSIS_DONE", { footprint });
    } catch (error) {
      if (isCurrentRequest(workflow.app?.entry.id, requestSeq, appId, seq) && workflow.phase === "analyzing") {
        workflow = transition(workflow, "FAIL", { error: "应用分析失败：" + error, recoverPhase: "analyzing" });
      }
    }
  }

  function rescanAnalysis() {
    if (workflow.phase !== "overview") return;
    const app = workflow.app;
    const seq = ++requestSeq;
    workflow = transition(workflow, "RESCAN_START");
    analyzeApp(app, seq);
  }

  async function requestUninstall() {
    if (workflow.phase !== "overview") return;
    try {
      capability = await invoke("uninstall_capability", { appId: workflow.app.entry.id });
      if (capability.mode === "unavailable") throw new Error(capability.message);
      uninstallConfirm = true;
    } catch (error) {
      workflow = transition(workflow, "FAIL", { error: String(error), recoverPhase: "overview" });
    }
  }

  async function confirmUninstall() {
    uninstallConfirm = false;
    const appId = workflow.app.entry.id;
    const baseline = workflow.footprint;
    const seq = ++requestSeq;
    workflow = transition(workflow, "PREPARE_START");
    statusNote = "";
    try {
      let footprint = baseline;
      if (!footprint) footprint = await invoke("resolve_footprint", { appId });
      if (!isCurrentRequest(workflow.app.entry.id, requestSeq, appId, seq)) return;
      workflow = transition(workflow, "BASELINE_READY", { footprint });
      await invoke("start_uninstaller", { appId, allowQuiet: capability?.mode === "quiet" });
      if (!isCurrentRequest(workflow.app.entry.id, requestSeq, appId, seq)) return;
      workflow = transition(workflow, "LAUNCHED");
      startPolling();
    } catch (error) {
      if (workflow.phase === "preparing" || workflow.phase === "launching") {
        workflow = transition(workflow, "FAIL", { error: String(error), recoverPhase: "overview" });
      }
    }
  }

  function startPolling() {
    stopPolling();
    const deadline = Date.now() + 10 * 60 * 1000;
    pollTimer = setInterval(async () => {
      if (Date.now() >= deadline) {
        stopPolling();
        statusNote = "自动检查已暂停，请在厂商卸载完成后手动检查结果。";
        return;
      }
      await checkRemoval(false);
    }, 2000);
  }

  function stopPolling() {
    if (pollTimer) clearInterval(pollTimer);
    pollTimer = null;
  }

  async function checkRemoval(manual = true) {
    if (workflow.phase !== "waiting" || checking) return;
    checking = true;
    const appId = workflow.app.entry.id;
    const seq = requestSeq;
    try {
      const installed = await invoke("is_app_installed", { appId });
      if (!isCurrentRequest(workflow.app.entry.id, requestSeq, appId, seq)) return;
      if (installed) {
        if (manual) statusNote = "仍检测到该应用，请完成厂商卸载后再次检查。";
        return;
      }
      statusNote = "正在确认卸载结果…";
      await new Promise((resolve) => setTimeout(resolve, 1200));
      const confirmedInstalled = await invoke("is_app_installed", { appId });
      if (!isCurrentRequest(workflow.app.entry.id, requestSeq, appId, seq)) return;
      if (confirmedInstalled) {
        statusNote = "应用登记信息仍在变化，请稍后再次检查。";
        return;
      }
      stopPolling();
      statusNote = "";
      workflow = transition(workflow, "APP_REMOVED");
      const footprint = await invoke("resolve_footprint", { appId });
      if (!isCurrentRequest(workflow.app.entry.id, requestSeq, appId, seq)) return;
      workflow = transition(workflow, "SCAN_DONE", { footprint });
      selectedPaths = defaultSelectedPaths(footprint.artifacts, footprint.app_removed);
    } catch (error) {
      if (workflow.phase === "waiting" || workflow.phase === "scanning") {
        workflow = transition(workflow, "FAIL", { error: String(error), recoverPhase: "waiting" });
      }
    } finally {
      checking = false;
    }
  }

  function cancelUninstallWait() {
    if (workflow.phase !== "waiting") return;
    const app = workflow.app;
    const seq = ++requestSeq;
    stopPolling();
    statusNote = "";
    workflow = transition(workflow, "UNINSTALL_CANCELLED");
    workflow = transition(workflow, "RESCAN_START");
    analyzeApp(app, seq);
  }

  async function openArtifact(event) {
    const path = event.detail?.path;
    const artifact = workflow.footprint?.artifacts?.find((item) => item.path === path);
    if (!canOpenArtifact(artifact)) {
      pushToast("该项目不是可打开的文件系统目录", "warn");
      return;
    }
    try {
      await shellOpen(path);
    } catch (error) {
      pushToast("无法打开目录：" + error, "error");
    }
  }

  function togglePath(path) {
    if (workflow.phase !== "review") return;
    const next = new Set(selectedPaths);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    selectedPaths = next;
  }

  function openCleanupConfirm() {
    userDataAcknowledged = false;
    cleanupConfirm = true;
  }

  async function cleanSelected() {
    cleanupConfirm = false;
    const appId = workflow.app.entry.id;
    workflow = transition(workflow, "CLEAN_START");
    try {
      const result = await invoke("remove_residue", { appId, targets: [...selectedPaths] });
      let footprint = workflow.footprint;
      try {
        footprint = await invoke("resolve_footprint", { appId });
      } catch (_) {
        // 清理结果保持可见；复扫失败由本次清理结果中的警告说明。
      }
      selectedPaths = new Set();
      workflow = transition(workflow, "CLEAN_DONE", { result, footprint });
    } catch (error) {
      workflow = transition(workflow, "FAIL", { error: String(error), recoverPhase: "waiting" });
    }
  }

  async function undoRemoval() {
    if (!workflow.result?.undo_token || undoing) return;
    undoing = true;
    try {
      const message = await invoke("undo_removal", { undoToken: workflow.result.undo_token });
      const footprint = await invoke("resolve_footprint", { appId: workflow.app.entry.id });
      workflow = { ...workflow, phase: "review", footprint, result: null, error: "" };
      selectedPaths = new Set();
      pushToast(message, "ok");
    } catch (error) {
      pushToast("撤销失败：" + error, "error");
    } finally {
      undoing = false;
    }
  }

  function retryAfterError() {
    statusNote = "";
    const recovery = workflow.recoverPhase;
    const app = workflow.app;
    if (recovery === "waiting") {
      workflow = transition(workflow, "RETRY_WAITING");
      startPolling();
    } else if (recovery === "analyzing") {
      workflow = transition(workflow, "RETRY_ANALYSIS");
      const seq = ++requestSeq;
      analyzeApp(app, seq);
    } else {
      workflow = transition(workflow, "RETRY_OVERVIEW");
    }
  }

  async function finishWorkflow() {
    requestSeq += 1;
    stopPolling();
    workflow = createWorkflow();
    selectedPaths = new Set();
    statusNote = "";
    await load();
  }

  const fmtBytes = (value) => {
    const n = value || 0;
    if (n < 1024) return `${n} B`;
    if (n < 1048576) return `${(n / 1024).toFixed(1)} KB`;
    if (n < 1073741824) return `${(n / 1048576).toFixed(1)} MB`;
    return `${(n / 1073741824).toFixed(2)} GB`;
  };
  const fmtKb = (value) => fmtBytes((value || 0) * 1024);
  const scoreClass = (value) => (value >= 70 ? "danger" : value >= 40 ? "warn" : "ok");
  // "YYYYMMDD" -> "YYYY-MM-DD"，非法值返回空串
  const fmtInstallDate = (raw) => {
    const s = String(raw || "").trim();
    return /^\d{8}$/.test(s) ? `${s.slice(0, 4)}-${s.slice(4, 6)}-${s.slice(6, 8)}` : "";
  };
  const iconInitial = (name) => (name || "?").trim().charAt(0).toUpperCase();
  const iconHue = (name) => {
    let h = 0;
    for (const ch of name || "") h = (h * 31 + ch.codePointAt(0)) % 360;
    return h;
  };
</script>

<div class="uninstall-page">
  {#if !$elevated}
    <div class="admin-banner">
      <span>卸载 HKLM/系统盘应用及清理受保护位置的残留需要<strong>管理员权限</strong>。</span>
      <button class="elevate-btn" on:click={relaunchAdmin}>以管理员重启</button>
    </div>
  {/if}

  <div class="ua">
    <div class="ua-list">
      <div class="ua-toolbar">
        <input class="search" aria-label="搜索应用或发行商" placeholder="搜索应用 / 发行商" bind:value={search} disabled={flowLocked} />
        <button class="ghost sm" on:click={load} disabled={loading || flowLocked}>刷新</button>
      </div>
      <div class="sort-row" aria-label="应用排序">
        {#each [["score", "推荐卸载度"], ["size", "体积"], ["lastUsed", "最近用"], ["installed", "安装日期"], ["name", "名称"]] as [key, label]}
          <button class="sort" class:active={sortKey === key} on:click={() => setSort(key)} disabled={flowLocked}>
            {label}{#if sortKey === key}<span class="arrow">{sortDir === "asc" ? "↑" : "↓"}</span>{/if}
          </button>
        {/each}
      </div>
      <div class="filter-row" aria-label="筛选">
        {#each [["autostart", "仅自启"], ["caution", "仅谨慎"], ["unused", "无使用记录"], ["large", "≥1GB"]] as [key, label]}
          <button class="filter-chip" class:on={filters.has(key)} on:click={() => toggleFilter(key)} disabled={flowLocked} aria-pressed={filters.has(key)}>
            {label}
          </button>
        {/each}
      </div>
      <div class="rows" role="listbox" aria-busy={loading} tabindex="0" on:keydown={onRowKeydown} aria-label="应用列表（方向键切换）">
        {#if loading}
          <div class="empty">加载中…</div>
        {:else if listError}
          <div class="empty list-error"><span>{listError}</span><button class="ghost sm" on:click={load}>重试</button></div>
        {:else if filtered.length === 0}
          <div class="empty">无匹配应用</div>
        {:else}
          {#each filtered as app (app.entry.id)}
            <button
              class="row"
              class:sel={workflow.app?.entry.id === app.entry.id}
              class:locked={flowLocked && workflow.app?.entry.id !== app.entry.id}
              aria-disabled={flowLocked && workflow.app?.entry.id !== app.entry.id}
              data-app-id={app.entry.id}
              on:click={() => selectApp(app)}
            >
              <span class="app-icon">
                {#if icons.get(app.entry.id)}
                  <img src={icons.get(app.entry.id)} alt="" />
                {:else}
                  <span class="icon-fallback" style="--h:{iconHue(app.entry.name)}">{iconInitial(app.entry.name)}</span>
                {/if}
              </span>
              <span class="score-pill {scoreClass(app.score.removal_recommendation)}">{app.score.removal_recommendation}</span>
              <span class="row-body">
                <span class="row-top">
                  <span class="row-name" title={app.entry.name}>{app.entry.name}</span>
                  {#if app.score.caution}<span class="chip caution">谨慎</span>{/if}
                  {#if app.score.autostart}<span class="chip">自启</span>{/if}
                </span>
                <span class="row-sub">
                  {#if app.entry.version}<span class="mono">v{app.entry.version}</span><span>·</span>{/if}
                  {#if fmtInstallDate(app.entry.install_date)}<span class="mono">{fmtInstallDate(app.entry.install_date)}</span><span>·</span>{/if}
                  <span class="mono">{app.entry.estimated_size_kb > 0 ? fmtKb(app.entry.estimated_size_kb) : "—"}</span><span>·</span><span>{app.score.last_used_label}</span>
                  {#if app.entry.publisher}<span>·</span><span class="ellip">{app.entry.publisher}</span>{/if}
                </span>
                {#if app.score.reasons?.length}
                  <span class="row-reason" title={app.score.reasons.join("；")}>{app.score.reasons[0]}</span>
                {/if}
              </span>
            </button>
          {/each}
        {/if}
      </div>
      <div class="list-foot muted">{filtered.length} / {apps.length} 个应用 · 仅覆盖注册表卸载项（ARP），MSIX/Store 应用暂不在清单内</div>
    </div>

    <main class="ua-detail">
      <UninstallWorkflow
        {workflow}
        {selectedPaths}
        {selectedSize}
        {statusNote}
        {fmtBytes}
        {fmtKb}
        on:requestUninstall={requestUninstall}
        on:rescan={rescanAnalysis}
        on:check={() => checkRemoval(true)}
        on:cancelWait={cancelUninstallWait}
        on:openArtifact={openArtifact}
        on:toggle={(event) => togglePath(event.detail.path)}
        on:clean={openCleanupConfirm}
        on:undo={undoRemoval}
        on:finish={finishWorkflow}
        on:retry={retryAfterError}
      />
    </main>
  </div>
</div>

<Modal open={uninstallConfirm} title="确认卸载" on:close={() => (uninstallConfirm = false)}>
  <div class="confirm-copy">
    <p>即将卸载 <strong>{workflow.app?.entry.name}</strong>。厂商卸载器执行的删除不可由 Win-Top 撤销。</p>
    <p class:quiet-warning={capability?.mode === "quiet"}>{capability?.message}</p>
    <p class="muted">Win-Top 会保留当前分析作为卸载前基线；厂商卸载完成后才能清理残留。</p>
  </div>
  <div class="modal-actions">
    <button class="ghost" on:click={() => (uninstallConfirm = false)}>取消</button>
    <button class="primary" on:click={confirmUninstall}>{capability?.mode === "quiet" ? "确认静默卸载" : "打开厂商卸载器"}</button>
  </div>
</Modal>

<Modal open={cleanupConfirm} title="确认清理残留" on:close={() => (cleanupConfirm = false)}>
  <div class="confirm-copy">
    <p>将把选中的 <strong>{selectedPaths.size}</strong> 项（约 <strong>{fmtBytes(selectedSize)}</strong>）移入隔离区；注册表项会先导出备份。</p>
    {#if selectedUserData.length}
      <div class="user-data-warning">
        <strong>包含用户数据</strong>
        <ul>{#each selectedUserData as item}<li>{item.path}</li>{/each}</ul>
        <label><input type="checkbox" bind:checked={userDataAcknowledged} />我知道这些设置、存档或作品可能丢失</label>
      </div>
    {/if}
  </div>
  <div class="modal-actions">
    <button class="ghost" on:click={() => (cleanupConfirm = false)}>取消</button>
    <button class="danger-button" disabled={selectedUserData.length > 0 && !userDataAcknowledged} on:click={cleanSelected}>确认清理</button>
  </div>
</Modal>

<style>
  .uninstall-page { display: flex; flex-direction: column; gap: 14px; height: 100%; min-height: 0; overflow: hidden; }
  .admin-banner { display: flex; align-items: center; gap: var(--sp-4); flex-wrap: wrap; flex: 0 0 auto; padding: 10px 14px; border: 1px solid rgba(245,158,11,.35); border-radius: var(--radius-sm); background: rgba(245,158,11,.1); color: #fcd34d; font-size: 13px; }
  .elevate-btn { padding: 5px 12px; border: 1px solid rgba(245,158,11,.45); border-radius: var(--radius-sm); background: rgba(245,158,11,.1); color: var(--warn); font-family: inherit; font-size: 12px; cursor: pointer; }
  .ua { display: flex; gap: var(--sp-4); flex: 1; min-height: 0; }
  .ua-list { width: 360px; flex-shrink: 0; display: flex; flex-direction: column; border: 1px solid var(--border); border-radius: var(--radius); background: var(--surface); overflow: hidden; }
  .ua-toolbar { display: flex; gap: var(--sp-2); padding: var(--sp-3); border-bottom: 1px solid var(--border); }
  .search { min-width: 0; flex: 1; padding: 7px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--surface-2); color: var(--text); font-family: inherit; font-size: 13px; }
  .search:focus { outline: none; border-color: var(--accent); }
  .search:focus-visible, button:focus-visible { outline: 2px solid #a5b4fc; outline-offset: 2px; }
  .sort-row { display: flex; flex-wrap: wrap; row-gap: 2px; gap: 2px; padding: var(--sp-2) var(--sp-3); border-bottom: 1px solid var(--border); }
  .sort { flex: 0 0 auto; padding: 4px 7px; border: none; border-radius: var(--radius-sm); background: transparent; color: var(--text-muted); font-family: inherit; font-size: 12px; cursor: pointer; white-space: nowrap; }
  .sort:hover, .sort.active { background: var(--surface-2); color: var(--text); }
  .arrow { margin-left: 2px; }
  .filter-row { display: flex; flex-wrap: wrap; gap: 6px; padding: 6px var(--sp-3); border-bottom: 1px solid var(--border); }
  .filter-chip { padding: 3px 9px; border: 1px solid var(--border); border-radius: 999px; background: transparent; color: var(--text-muted); font-family: inherit; font-size: 11px; cursor: pointer; transition: border-color .15s ease, background .15s ease, color .15s ease; }
  .filter-chip:hover { border-color: var(--accent); color: var(--text); }
  .filter-chip.on { border-color: var(--accent); background: rgba(99, 102, 241, 0.16); color: var(--text); }
  .rows { flex: 1; min-height: 0; overflow-y: auto; }
  .rows:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  .row { display: flex; align-items: center; gap: 9px; width: 100%; padding: 9px 12px; border: none; border-bottom: 1px solid var(--border); background: transparent; color: var(--text); font-family: inherit; cursor: pointer; text-align: left; }
  .app-icon { width: 26px; height: 26px; flex-shrink: 0; border-radius: 6px; overflow: hidden; display: flex; align-items: center; justify-content: center; background: var(--surface-2); }
  .app-icon img { width: 100%; height: 100%; object-fit: contain; }
  .icon-fallback { width: 100%; height: 100%; display: flex; align-items: center; justify-content: center; font-size: 12px; font-weight: 700; color: #fff; background: hsl(var(--h, 220), 42%, 34%); }
  .row-reason { display: block; margin-top: 2px; font-size: 10px; color: var(--text-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .row { display: flex; align-items: center; gap: var(--sp-3); width: 100%; padding: 9px 12px; border: none; border-bottom: 1px solid var(--border); background: transparent; color: var(--text); font-family: inherit; cursor: pointer; text-align: left; }
  .row:hover { background: var(--surface-2); }
  .row.sel { background: var(--surface-2); box-shadow: inset 3px 0 0 var(--accent); }
  .row.locked { cursor: default; }
  .row-body { min-width: 0; flex: 1; }
  .row-top { display: flex; align-items: center; gap: 6px; }
  .row-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; font-weight: 500; }
  .row-sub { display: flex; align-items: center; gap: 5px; margin-top: 2px; color: var(--text-muted); font-size: 11px; white-space: nowrap; overflow: hidden; }
  .row-sub > span { flex-shrink: 0; }
  .row-sub .ellip { flex-shrink: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .list-foot { padding: 6px 12px; border-top: 1px solid var(--border); font-size: 11px; }
  .score-pill { min-width: 30px; flex-shrink: 0; padding: 3px 6px; border-radius: var(--radius-sm); text-align: center; font: 700 13px var(--font-mono); }
  .score-pill.ok { color: var(--ok); background: rgba(34,197,94,.12); }
  .score-pill.warn { color: var(--warn); background: rgba(245,158,11,.14); }
  .score-pill.danger { color: var(--danger); background: rgba(239,68,68,.14); }
  .chip { padding: 1px 6px; border: 1px solid var(--border); border-radius: 999px; background: var(--surface-2); color: var(--text-muted); font-size: 10px; }
  .chip.caution { border-color: rgba(245,158,11,.4); color: var(--warn); }
  .ua-detail { min-width: 0; flex: 1; overflow-y: auto; padding: 20px; border: 1px solid var(--border); border-radius: var(--radius); background: var(--surface); }
  .empty { display: grid; place-content: center; gap: 10px; padding: 30px; color: var(--text-muted); text-align: center; font-size: 13px; }
  .list-error { color: #fca5a5; }
  .muted { color: var(--text-muted); }
  .mono { font-family: var(--font-mono); font-variant-numeric: tabular-nums; }
  .ghost { padding: 7px 14px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: transparent; color: var(--text); font-family: inherit; font-size: 13px; cursor: pointer; }
  .ghost:hover { background: var(--surface-2); }
  .ghost.sm { padding: 5px 10px; font-size: 12px; }
  button:disabled, input:disabled { opacity: .55; cursor: default; }
  .confirm-copy p { margin: 0 0 11px; }
  .confirm-copy strong { color: var(--text); }
  .quiet-warning { padding: 9px 10px; border: 1px solid rgba(245,158,11,.38); border-radius: 8px; background: rgba(245,158,11,.08); color: #fcd34d; }
  .modal-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 14px; }
  .modal-actions button { min-height: 40px; padding: 8px 14px; }
  .primary { border: none; border-radius: var(--radius-sm); background: linear-gradient(135deg, var(--accent), #7c3aed); color: white; font-family: inherit; cursor: pointer; }
  .danger-button { border: 1px solid rgba(239,68,68,.55); border-radius: var(--radius-sm); background: rgba(239,68,68,.16); color: #fecaca; font-family: inherit; cursor: pointer; }
  .user-data-warning { margin-top: 12px; padding: 10px; border: 1px solid rgba(239,68,68,.4); border-radius: 9px; background: rgba(239,68,68,.07); color: #fecaca; }
  .user-data-warning ul { max-height: 110px; margin: 7px 0; padding-left: 20px; overflow: auto; font: 11px/1.6 var(--font-mono); }
  .user-data-warning label { display: flex; align-items: flex-start; gap: 7px; color: var(--text); font-size: 12px; }
  @media (max-width: 1050px) { .ua-list { width: 320px; } }
</style>
