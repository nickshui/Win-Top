<script>
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { pushToast, elevated, relaunchAdmin, metrics, diskReport, loadDiskReport } from "../stores.js";
  import Modal from "../components/Modal.svelte";

  export let navigate = () => {};

  // ===== 加速中心首页 =====
  let tab = "home";

  // 一键加速：串联垃圾扫描 + 清理 + 内存释放
  let boosting = false;
  let boostDone = false;
  let junkBytes = 0;           // 当前可清理垃圾字节
  let junkScanning = false;
  let lastBoost = null;        // { freedBytes, freedMb }

  // 进程 / 启动项 摘要
  let procCount = 0;
  let startupEnabledCount = 0;

  $: memLoad = $metrics?.mem_load ?? 0;
  $: sysVol = ($diskReport?.volumes ?? []).find((v) => /^c/i.test(v.drive)) ?? ($diskReport?.volumes ?? [])[0] ?? null;

  async function scanJunkQuiet() {
    junkScanning = true;
    try {
      const r = await invoke("scan_junk");
      junkBytes = r.total_bytes ?? 0;
    } catch (e) { junkBytes = 0; } finally { junkScanning = false; }
  }

  async function loadHomeSummary() {
    loadDiskReport();
    scanJunkQuiet();
    try {
      const procs = await invoke("get_processes");
      procCount = procs.length;
    } catch (e) {}
    try {
      const items = await invoke("list_startup");
      startupItems = items;
      startupEnabledCount = items.filter((s) => s.enabled).length;
    } catch (e) {}
  }

  async function oneClickBoost() {
    if (boosting) return;
    boosting = true;
    boostDone = false;
    try {
      const rep = await invoke("scan_junk");
      const ids = rep.categories
        .filter((c) => c.available && (!c.needs_admin || $elevated))
        .map((c) => c.id);
      const cleanR = ids.length ? await invoke("clean_junk", { ids }) : { freed_bytes: 0 };
      const boostR = await invoke("memory_boost");
      lastBoost = { freedBytes: cleanR.freed_bytes ?? 0, freedMb: boostR.freed_mb ?? 0 };
      pushToast(`加速完成：释放磁盘 ${fmtBytes(lastBoost.freedBytes)}、内存 ${lastBoost.freedMb.toFixed(0)} MB`, "ok");
      boostDone = true;
      await scanJunkQuiet();
    } catch (e) {
      pushToast("加速失败：" + e, "error");
    } finally {
      boosting = false;
    }
  }

  // ===== 系统体检 =====
  let healthLoading = false, healthReport = null;
  async function runHealthCheck() {
    healthLoading = true; healthReport = null;
    try { healthReport = await invoke("run_health_check"); }
    catch (e) { pushToast("体检失败：" + e, "error"); }
    finally { healthLoading = false; }
  }
  const scoreColor = (s) => (s >= 80 ? "ok" : s >= 60 ? "warn" : "danger");

  // 体检页「一键优化」：确认后直接执行清理+内存释放，完成后刷新体检分数
  let optConfirm = false, optimizing = false;
  function prepareOptimize() { optConfirm = true; }
  async function runOptimize() {
    optConfirm = false; optimizing = true;
    try {
      const scan = await invoke("scan_junk");
      const ids = scan.categories.filter(c => c.available && (!c.needs_admin || $elevated)).map(c => c.id);
      const cleanR = ids.length ? await invoke("clean_junk", { ids }) : { freed_bytes: 0 };
      const boostR = await invoke("memory_boost");
      pushToast(`已释放磁盘 ${fmtBytes(cleanR.freed_bytes)}，内存 ${boostR.freed_mb.toFixed(0)} MB`, "ok");
      // 重新体检，反映优化后的分数变化
      healthReport = await invoke("run_health_check");
      scanJunkQuiet();
    } catch (e) { pushToast("优化失败：" + e, "error"); }
    finally { optimizing = false; }
  }

  // ===== 垃圾清理 =====
  let scanLoading = false, cleanReport = null, selectedCats = new Set(), cleaning = false, cleanResult = null, cleanConfirm = false, createRpBeforeClean = true;
  async function scanJunk() {
    scanLoading = true; cleanReport = null; cleanResult = null;
    try {
      cleanReport = await invoke("scan_junk");
      selectedCats = new Set(cleanReport.categories.filter(c => c.available && (!c.needs_admin || $elevated)).map(c => c.id));
    } catch (e) { pushToast("扫描失败：" + e, "error"); } finally { scanLoading = false; }
  }
  function toggleCat(id) { const s = new Set(selectedCats); if (s.has(id)) s.delete(id); else s.add(id); selectedCats = s; }
  $: selectedBytes = cleanReport ? cleanReport.categories.filter(c => selectedCats.has(c.id)).reduce((s, c) => s + c.bytes, 0) : 0;
  async function doClean() {
    cleanConfirm = false; cleaning = true;
    try {
      if (createRpBeforeClean) { try { await invoke("create_restore_point", { description: "Win-Top 清理前自动还原点" }); } catch (e) { } }
      const cleanR = await invoke("clean_junk", { ids: [...selectedCats] });
      const boostR = await invoke("memory_boost");
      pushToast(`释放磁盘 ${fmtBytes(cleanR.freed_bytes)}，释放内存 ${boostR.freed_mb.toFixed(0)} MB`, "ok");
      cleanResult = cleanR;
      cleanReport = await invoke("scan_junk");
      selectedCats = new Set(cleanReport.categories.filter(c => c.available && (!c.needs_admin || $elevated)).map(c => c.id));
    } catch (e) { pushToast("优化失败：" + e, "error"); } finally { cleaning = false; }
  }

  // ===== 开机启动（保留原始设计） =====
  let startupItems = [], startupLoading = true, startupToggleTarget = null, startupPending = null;

  async function loadStartup() {
    startupLoading = true;
    try { startupItems = await invoke("list_startup"); }
    catch (e) { pushToast("读取启动项失败：" + e, "error"); }
    finally { startupLoading = false; }
  }

  function confirmToggle(item) {
    startupToggleTarget = item;
  }

  async function doToggle() {
    const item = startupToggleTarget;
    startupToggleTarget = null;
    startupPending = item.id;
    try {
      const r = await invoke("set_startup_enabled", { id: item.id, enabled: !item.enabled });
      if (r.success) {
        item.enabled = !item.enabled;
        startupItems = startupItems;
      }
      pushToast(r.message, r.success ? "ok" : "error");
    } catch (e) { pushToast("操作失败：" + e, "error"); }
    finally { startupPending = null; }
  }

  const fmtBytes = (n) => {
    if (!n) return "0 B";
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
    return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
  };
  const locLabel = (l) => ({ "HKCU-Run": "用户注册表", "HKLM-Run": "系统注册表", "User-Folder": "用户启动文件夹", "Common-Folder": "公共启动文件夹" })[l] || l;

  onMount(() => { loadStartup(); loadHomeSummary(); });
</script>

{#if !$elevated}
  <div class="admin-banner">
    <span>部分功能需要<strong>管理员权限</strong>。</span>
    <button class="elevate-btn" on:click={relaunchAdmin}>以管理员重启</button>
  </div>
{/if}

<div class="tabs">
  <button class="tab" class:active={tab === "home"} on:click={() => (tab = "home")}>总览</button>
  <button class="tab" class:active={tab === "health"} on:click={() => (tab = "health")}>系统体检</button>
  <button class="tab" class:active={tab === "cleanup"} on:click={() => (tab = "cleanup")}>垃圾清理</button>
  <button class="tab" class:active={tab === "startup"} on:click={() => (tab = "startup")}>
    开机启动
    <span class="tab-badge">{startupItems.filter(s => s.enabled).length}</span>
  </button>
</div>

<!-- ===== 总览：加速中心 ===== -->
{#if tab === "home"}
  <div class="boost-card">
    <div class="boost-head">
      <div>
        <h3 class="boost-title">一键加速</h3>
        <p class="boost-sub">清理内存和临时文件，减少电脑卡顿</p>
      </div>
    </div>
    <div class="boost-metrics">
      <div class="bm">
        <div class="bm-bar"><div class="bm-fill {memLoad >= 85 ? 'danger' : memLoad >= 70 ? 'warn' : 'ok'}" style="height:{Math.min(100, memLoad)}%"></div></div>
        <div class="bm-text"><span class="bm-num mono">{memLoad}%</span><span class="bm-label">内存占用</span></div>
      </div>
      <div class="bm">
        <div class="bm-bar"><div class="bm-fill warn" style="height:{junkBytes > 0 ? Math.min(100, junkBytes / (2 * 1024 * 1024 * 1024) * 100) : 0}%"></div></div>
        <div class="bm-text"><span class="bm-num mono">{junkScanning ? "…" : fmtBytes(junkBytes)}</span><span class="bm-label">临时文件</span></div>
      </div>
    </div>
    <button class="boost-btn" class:done={boostDone && !boosting} on:click={oneClickBoost} disabled={boosting}>
      {#if boosting}加速中…{:else if boostDone}&#10003; 加速完成{:else}一键加速{/if}
    </button>
    {#if boostDone && lastBoost}
      <p class="boost-result">本次释放磁盘 <b>{fmtBytes(lastBoost.freedBytes)}</b> · 内存 <b>{lastBoost.freedMb.toFixed(0)} MB</b></p>
    {/if}
  </div>

  <div class="entry-grid">
    <button class="entry" on:click={() => (tab = "health")}>
      <div class="entry-head"><span class="entry-name">全面体检</span></div>
      <div class="entry-info">
        {#if healthReport}<span class="entry-strong {scoreColor(healthReport.score)}">{healthReport.score} 分</span>
        {:else}<span class="entry-hint">未检测</span>{/if}
      </div>
    </button>

    <button class="entry" on:click={() => navigate("process")}>
      <div class="entry-head"><span class="entry-name">进程管理</span></div>
      <div class="entry-info"><span class="entry-strong">{procCount}</span><span class="entry-unit">个进程</span></div>
    </button>

    <button class="entry" on:click={() => (tab = "cleanup")}>
      <div class="entry-head"><span class="entry-name">深度清理</span></div>
      <div class="entry-info">
        {#if sysVol}<span class="entry-strong mono">{fmtBytes(sysVol.total - sysVol.free)}</span><span class="entry-unit mono">/ {fmtBytes(sysVol.total)}</span>
        {:else}<span class="entry-hint">读取中…</span>{/if}
      </div>
    </button>

    <button class="entry" on:click={() => (tab = "startup")}>
      <div class="entry-head"><span class="entry-name">开机管理</span></div>
      <div class="entry-info"><span class="entry-strong">{startupEnabledCount}</span><span class="entry-unit">项启用</span></div>
    </button>
  </div>

<!-- ===== 系统体检 ===== -->
{:else if tab === "health"}
  {#if !healthReport}
    <div class="center-cta">
      <p class="muted">一键扫描垃圾、启动项、资源占用。</p>
      <button class="primary" on:click={runHealthCheck} disabled={healthLoading}>{healthLoading ? "扫描中…" : "开始体检"}</button>
    </div>
  {:else}
    <div class="health-top">
      <div class="health-score {scoreColor(healthReport.score)}">{healthReport.score}<span>分</span></div>
      <div class="health-meta">
        <div>垃圾可清理 <b>{fmtBytes(healthReport.junk_mb * 1024 * 1024)}</b></div>
        <div>自启程序 <b>{healthReport.startup_count} 项</b></div>
        <div>高占用进程 <b>{healthReport.heavy_procs.length} 个</b></div>
      </div>
      <div class="health-actions">
        <button class="ghost" on:click={runHealthCheck} disabled={healthLoading || optimizing}>重新体检</button>
        <button class="primary" on:click={prepareOptimize} disabled={optimizing || healthLoading}>{optimizing ? "优化中…" : "一键优化"}</button>
      </div>
    </div>
    {#if healthReport.issues.length > 0}
      <div class="issue-list">
        {#each healthReport.issues as issue}
          <div class="issue-row"><span>{issue.title}</span><span class="issue-detail">{issue.detail}</span></div>
        {/each}
      </div>
    {/if}
    {#if healthReport.suggestions.length > 0}
      <div class="health-tips">{#each healthReport.suggestions as s}<div class="tip">{s}</div>{/each}</div>
    {/if}
  {/if}

  <Modal open={optConfirm} title="确认一键优化" on:close={() => (optConfirm = false)}>
    {#if healthReport}
      <p>将执行以下操作：</p>
      <ul class="opt-list">
        <li>清理系统垃圾（约 {fmtBytes(healthReport.junk_mb * 1024 * 1024)}）</li>
        <li>释放进程占用的物理内存</li>
      </ul>
      <p class="muted">清理为删除操作、不可撤销；被占用的文件会自动跳过。完成后将重新体检。</p>
      <div class="modal-actions">
        <button class="ghost" on:click={() => (optConfirm = false)}>取消</button>
        <button class="primary" on:click={runOptimize}>确认优化</button>
      </div>
    {/if}
  </Modal>

<!-- ===== 垃圾清理 ===== -->
{:else if tab === "cleanup"}
  {#if !cleanReport}
    <div class="center-cta">
      <p class="muted">扫描临时文件、缓存、回收站，一键释放磁盘空间。</p>
      <button class="primary" on:click={scanJunk} disabled={scanLoading}>{scanLoading ? "扫描中…" : "扫描垃圾"}</button>
    </div>
  {:else}
    <div class="clean-bar">
      <span class="clean-total">{fmtBytes(selectedBytes)}<span class="muted"> 可清理</span></span>
      <div class="clean-actions">
        <label class="rp-toggle"><input type="checkbox" bind:checked={createRpBeforeClean} />创建还原点</label>
        <button class="ghost" on:click={scanJunk} disabled={scanLoading}>重新扫描</button>
        <button class="primary" on:click={() => (cleanConfirm = true)} disabled={cleaning || selectedCats.size === 0}>
          {cleaning ? "清理中…" : "一键优化"}
        </button>
      </div>
    </div>
    <div class="cats">
      {#each cleanReport.categories as c (c.id)}
        <label class="cat" class:off={!c.available || (c.needs_admin && !$elevated)}>
          <input type="checkbox" checked={selectedCats.has(c.id)} disabled={!c.available || (c.needs_admin && !$elevated)} on:change={() => toggleCat(c.id)} />
          <span class="cat-label">{c.label}{#if c.needs_admin && !$elevated}<span class="lock">&#128274;</span>{/if}</span>
          <span class="cat-size">{fmtBytes(c.bytes)}</span>
        </label>
      {/each}
    </div>
    {#if cleanResult}<div class="clean-result">已释放 <b>{fmtBytes(cleanResult.freed_bytes)}</b></div>{/if}
  {/if}
  <Modal open={cleanConfirm} title="确认优化" on:close={() => (cleanConfirm = false)}>
    <p>清理 {selectedCats.size} 个分类共 {fmtBytes(selectedBytes)}，同时释放内存。不可撤销。{createRpBeforeClean ? "已勾选还原点。" : ""}</p>
    <div class="modal-actions"><button class="ghost" on:click={() => (cleanConfirm = false)}>取消</button><button class="primary" on:click={doClean}>确认</button></div>
  </Modal>

<!-- ===== 开机启动 ===== -->
{:else if tab === "startup"}
  <div class="table-wrap">
    <table>
      <thead><tr><th class="col-name">名称</th><th>位置</th><th class="col-cmd">命令</th><th class="col-sw">状态</th></tr></thead>
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
                  class="toggle-switch"
                  class:on={item.enabled}
                  class:pending={startupPending === item.id}
                  on:click={() => confirmToggle(item)}
                  aria-label={item.enabled ? '禁用' : '启用'}
                  disabled={startupPending === item.id}
                >
                  <span class="toggle-knob"></span>
                </button>
              </td>
            </tr>
          {/each}
        {/if}
      </tbody>
    </table>
  </div>

  <Modal open={!!startupToggleTarget} title={startupToggleTarget?.enabled ? '禁用启动项' : '启用启动项'} on:close={() => (startupToggleTarget = null)}>
    {#if startupToggleTarget}
      <p>
        {startupToggleTarget.enabled ? '禁用' : '启用'}
        <strong> {startupToggleTarget.name}</strong>
        （{locLabel(startupToggleTarget.location)}）。
        {startupToggleTarget.enabled ? '该程序将不再随系统自启。' : '该程序将随系统自启。'}
      </p>
      {#if startupToggleTarget.location.startsWith('HKLM')}
        <p class="muted">此操作需要管理员权限。</p>
      {/if}
      <div class="modal-actions">
        <button class="ghost" on:click={() => (startupToggleTarget = null)}>取消</button>
        <button class="primary" on:click={doToggle}>确认{startupToggleTarget.enabled ? '禁用' : '启用'}</button>
      </div>
    {/if}
  </Modal>
{/if}

<style>
  .admin-banner { display: flex; align-items: center; gap: var(--sp-4); flex-wrap: wrap; padding: 10px 14px; margin-bottom: var(--sp-4); border: 1px solid rgba(245,158,11,0.35); background: rgba(245,158,11,0.1); border-radius: var(--radius-sm); font-size: 13px; color: #fcd34d; }
  .elevate-btn { font-size: 12px; padding: 5px 12px; border-radius: var(--radius-sm); border: 1px solid rgba(245,158,11,0.45); background: rgba(245,158,11,0.1); color: var(--warn); font-family: inherit; cursor: pointer; }
  .elevate-btn:hover { background: rgba(245,158,11,0.18); }

  .tabs { display: flex; gap: 0; margin-bottom: var(--sp-4); border-bottom: 1px solid var(--border); }
  .tab { border: none; background: transparent; color: var(--text-muted); font-family: inherit; font-size: 14px; padding: 10px 18px; cursor: pointer; border-bottom: 2px solid transparent; margin-bottom: -1px; display: flex; align-items: center; gap: 6px; }
  .tab:hover { color: var(--text); }
  .tab.active { color: var(--text); border-bottom-color: var(--accent); }
  .tab-badge { font-size: 11px; color: var(--text-muted); background: var(--surface-2); padding: 1px 7px; border-radius: 999px; font-variant-numeric: tabular-nums; }

  .center-cta { display: flex; flex-direction: column; align-items: center; gap: var(--sp-4); padding: var(--sp-8); text-align: center; }
  .muted { color: var(--text-muted); font-size: 13px; }
  .mono { font-family: var(--font-mono); font-variant-numeric: tabular-nums; }

  /* 加速中心：一键加速主卡 */
  .boost-card { border: 1px solid var(--border); border-radius: var(--radius); background: var(--surface); padding: var(--sp-6); margin-bottom: var(--sp-4); }
  .boost-head { display: flex; justify-content: space-between; align-items: flex-start; }
  .boost-title { margin: 0; font-size: 16px; font-weight: 600; }
  .boost-sub { margin: 4px 0 0; font-size: 13px; color: var(--text-muted); }
  .boost-metrics { display: flex; gap: var(--sp-8); margin: var(--sp-4) 0; }
  .bm { display: flex; align-items: center; gap: var(--sp-3); }
  .bm-bar { width: 4px; height: 40px; border-radius: 999px; background: var(--surface-2); overflow: hidden; display: flex; flex-direction: column; justify-content: flex-end; }
  .bm-fill { width: 100%; border-radius: 999px; transition: height 0.4s ease; }
  .bm-fill.ok { background: var(--accent); }
  .bm-fill.warn { background: var(--warn); }
  .bm-fill.danger { background: var(--danger); }
  .bm-text { display: flex; flex-direction: column; }
  .bm-num { font-size: 20px; font-weight: 700; line-height: 1.1; }
  .bm-label { font-size: 12px; color: var(--text-muted); }
  .boost-btn { width: 100%; border: none; background: linear-gradient(135deg, var(--accent), #7c3aed); color: #fff; font-family: inherit; font-size: 15px; font-weight: 600; padding: 12px; border-radius: var(--radius-sm); cursor: pointer; transition: opacity 0.15s ease; }
  .boost-btn:hover { opacity: 0.92; }
  .boost-btn:disabled { opacity: 0.6; cursor: default; }
  .boost-btn.done { background: rgba(34, 197, 94, 0.16); color: var(--ok); border: 1px solid rgba(34, 197, 94, 0.4); }
  .boost-result { margin: var(--sp-3) 0 0; font-size: 13px; color: var(--text-muted); text-align: center; }
  .boost-result b { color: var(--text); }

  /* 加速中心：功能宫格 */
  .entry-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: var(--sp-3); }
  .entry { text-align: left; display: flex; flex-direction: column; gap: var(--sp-2); padding: var(--sp-4); border: 1px solid var(--border); border-radius: var(--radius); background: var(--surface); color: var(--text); font-family: inherit; cursor: pointer; transition: border-color 0.15s ease, background 0.15s ease; }
  .entry:hover { border-color: var(--accent); background: var(--surface-2); }
  .entry-head { display: flex; align-items: center; }
  .entry-name { font-size: 14px; font-weight: 500; }
  .entry-info { display: flex; align-items: baseline; gap: 6px; }
  .entry-strong { font-size: 18px; font-weight: 700; }
  .entry-strong.ok { color: var(--ok); }
  .entry-strong.warn { color: var(--warn); }
  .entry-strong.danger { color: var(--danger); }
  .entry-unit { font-size: 12px; color: var(--text-muted); }
  .entry-hint { font-size: 13px; color: var(--text-muted); }

  /* 体检 */
  .health-top { display: flex; align-items: center; gap: var(--sp-6); margin-bottom: var(--sp-4); }
  .health-score { width: 64px; height: 64px; border-radius: 999px; display: flex; flex-direction: column; align-items: center; justify-content: center; border: 3px solid; font-size: 22px; font-weight: 700; font-family: var(--font-mono); flex-shrink: 0; }
  .health-score span { font-size: 10px; font-weight: 400; }
  .health-score.ok { border-color: var(--ok); color: var(--ok); }
  .health-score.warn { border-color: var(--warn); color: var(--warn); }
  .health-score.danger { border-color: var(--danger); color: var(--danger); }
  .health-meta { display: flex; flex-direction: column; gap: 4px; font-size: 13px; flex: 1; }
  .health-actions { display: flex; flex-direction: column; gap: var(--sp-2); flex-shrink: 0; }
  .issue-list { margin-bottom: var(--sp-4); }
  .issue-row { padding: var(--sp-2) 0; font-size: 13px; display: flex; gap: var(--sp-3); border-bottom: 1px solid var(--border); }
  .issue-row:last-child { border-bottom: none; }
  .issue-detail { color: var(--text-muted); font-size: 12px; }
  .health-tips { border: 1px solid var(--border); border-radius: var(--radius); padding: var(--sp-3) var(--sp-4); background: var(--surface); }
  .tip { font-size: 13px; color: var(--text-muted); padding: 2px 0 2px 14px; position: relative; }
  .tip::before { content: ""; position: absolute; left: 0; top: 9px; width: 5px; height: 5px; border-radius: 999px; background: var(--accent); }
  .opt-list { margin: var(--sp-2) 0; padding-left: 18px; font-size: 13px; color: var(--text); line-height: 1.7; }

  /* 清理 */
  .clean-bar { display: flex; align-items: center; justify-content: space-between; gap: var(--sp-4); padding: var(--sp-4); border: 1px solid var(--border); border-radius: var(--radius); background: var(--surface); margin-bottom: var(--sp-4); }
  .clean-total { font-size: 22px; font-weight: 700; font-family: var(--font-mono); }
  .clean-actions { display: flex; align-items: center; gap: var(--sp-3); }
  .rp-toggle { font-size: 12px; color: var(--text-muted); display: flex; align-items: center; gap: 4px; cursor: pointer; white-space: nowrap; }
  .cats { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: var(--sp-2); }
  .cat { display: flex; align-items: center; gap: var(--sp-2); padding: 9px 12px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--surface); cursor: pointer; font-size: 13px; }
  .cat.off { opacity: 0.45; cursor: default; }
  .cat input { accent-color: var(--accent); }
  .cat-label { flex: 1; }
  .cat-size { color: var(--text-muted); font-family: var(--font-mono); font-size: 12px; }
  .lock { font-size: 11px; }
  .clean-result { margin-top: var(--sp-3); padding: var(--sp-3); font-size: 14px; }

  /* 表 */
  .table-wrap { border: 1px solid var(--border); border-radius: var(--radius); overflow: hidden; background: var(--surface); }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  thead th { position: sticky; top: 0; background: var(--surface-2); text-align: left; padding: 10px 14px; font-weight: 500; color: var(--text-muted); white-space: nowrap; }
  tbody { display: block; max-height: calc(100vh - 260px); overflow-y: auto; }
  thead, tbody tr { display: table; width: 100%; table-layout: fixed; }
  td { padding: 8px 14px; border-top: 1px solid var(--border); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  tbody tr:hover { background: var(--surface-2); }
  .col-name { width: auto; }
  .col-cmd { max-width: 0; }
  .col-sw { width: 64px; text-align: center; }
  .empty { text-align: center; color: var(--text-muted); padding: 40px; }

  /* Toggle 开关（原始设计） */
  .toggle-switch {
    position: relative;
    width: 44px;
    height: 24px;
    border-radius: 999px;
    border: none;
    background: var(--surface-2);
    cursor: pointer;
    padding: 0;
    transition: background 0.25s ease;
  }
  .toggle-switch:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .toggle-switch.on { background: rgba(99, 102, 241, 0.9); }
  .toggle-knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 20px;
    height: 20px;
    border-radius: 999px;
    background: var(--text);
    transition: transform 0.25s ease;
  }
  .toggle-switch.on .toggle-knob { transform: translateX(20px); }
  .toggle-switch.pending { opacity: 0.6; }
  .toggle-switch.pending .toggle-knob { animation: pulse 0.6s ease-in-out infinite alternate; }
  @keyframes pulse { from { transform: scale(1); } to { transform: scale(0.85); } }

  .primary { border: none; background: linear-gradient(135deg, var(--accent), #7c3aed); color: #fff; font-family: inherit; font-size: 13px; padding: 8px 16px; border-radius: var(--radius-sm); cursor: pointer; }
  .primary:disabled { opacity: 0.6; cursor: default; }
  .ghost { border: 1px solid var(--border); background: transparent; color: var(--text); font-family: inherit; font-size: 13px; padding: 7px 14px; border-radius: var(--radius-sm); cursor: pointer; }
  .ghost:hover { background: var(--surface-2); }
  .modal-actions { display: flex; justify-content: flex-end; gap: var(--sp-2); margin-top: var(--sp-3); }
</style>
