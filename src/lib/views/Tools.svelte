<script>
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { pushToast, elevated, relaunchAdmin } from "../stores.js";
  import Modal from "../components/Modal.svelte";

  let tab = "firewall"; // firewall | restore | unlock | export

  // ===== Firewall =====
  let firewallRules = [];
  let firewallLoading = true;
  let firewallQ = "";
  let fwToggleTarget = null; // { name, enable }

  async function loadFirewall() {
    firewallLoading = true;
    try {
      firewallRules = await invoke("list_firewall_rules");
    } catch (e) {
      pushToast("加载防火墙规则失败：" + e, "error");
    } finally {
      firewallLoading = false;
    }
  }

  function confirmToggleRule(name, enable) {
    fwToggleTarget = { name, enable };
  }
  async function doToggleRule() {
    const t = fwToggleTarget;
    fwToggleTarget = null;
    try {
      const r = await invoke("toggle_firewall_rule", { name: t.name, enabled: t.enable });
      pushToast(r, "ok");
      loadFirewall();
    } catch (e) {
      pushToast("操作失败：" + e, "error");
    }
  }

  $: fwFiltered = firewallRules.filter(r => {
    if (!firewallQ) return true;
    const q = firewallQ.toLowerCase();
    return r.name.toLowerCase().includes(q) || r.local_port.includes(q);
  });

  // ===== Restore =====
  let restorePoints = [];
  let restoreLoading = true;
  let newRpDesc = "";
  let creatingRp = false;
  let rpConfirmOpen = false;

  async function loadRestore() {
    restoreLoading = true;
    try {
      restorePoints = await invoke("list_restore_points");
    } catch (e) {
      pushToast("加载还原点失败：" + e, "error");
    } finally {
      restoreLoading = false;
    }
  }

  function confirmCreateRp() {
    if (!newRpDesc.trim()) return;
    rpConfirmOpen = true;
  }
  async function doCreateRp() {
    rpConfirmOpen = false;
    creatingRp = true;
    try {
      const r = await invoke("create_restore_point", { description: newRpDesc.trim() });
      pushToast(r, "ok");
      newRpDesc = "";
      loadRestore();
    } catch (e) {
      pushToast("创建还原点失败：" + e, "error");
    } finally {
      creatingRp = false;
    }
  }

  // ===== File Unlock =====
  let unlockPath = "";
  let unlockResults = [];
  let unlockLoading = false;
  let unlockSearched = false;
  let killTarget = null; // { pid, process_name }

  async function findLocks() {
    if (!unlockPath.trim()) return;
    unlockLoading = true;
    unlockSearched = true;
    try {
      unlockResults = await invoke("find_file_locks", { filePath: unlockPath.trim() });
    } catch (e) {
      pushToast("查询失败：" + e, "error");
      unlockResults = [];
    } finally {
      unlockLoading = false;
    }
  }

  function confirmKill(info) {
    killTarget = info;
  }
  async function doKillProcess() {
    const t = killTarget;
    killTarget = null;
    try {
      const r = await invoke("terminate_process", { pid: t.pid });
      pushToast(r.message, r.success ? "ok" : "error");
      if (r.success) findLocks();
    } catch (e) {
      pushToast("结束进程失败：" + e, "error");
    }
  }

  // ===== Export =====
  let exportLoading = false;
  let exportResult = null;

  async function doExport(type) {
    exportLoading = true;
    exportResult = null;
    try {
      if (type === "json") {
        exportResult = await invoke("export_snapshot");
      } else {
        exportResult = await invoke("export_processes_csv");
      }
      if (exportResult.ok) {
        pushToast("已导出到：" + exportResult.path, "ok");
      }
    } catch (e) {
      pushToast("导出失败：" + e, "error");
    } finally {
      exportLoading = false;
    }
  }

  onMount(() => {
    loadFirewall();
    loadRestore();
  });
</script>

{#if !$elevated}
  <div class="admin-banner">
    <span>防火墙管理、系统还原和文件解锁需要<strong>管理员权限</strong>。</span>
    <button class="primary" on:click={relaunchAdmin}>以管理员重启</button>
  </div>
{/if}

<div class="tabs" role="tablist">
  <button class="tab" class:active={tab === "firewall"} on:click={() => (tab = "firewall")} role="tab" aria-selected={tab === "firewall"}>防火墙</button>
  <button class="tab" class:active={tab === "restore"} on:click={() => (tab = "restore")} role="tab" aria-selected={tab === "restore"}>系统还原</button>
  <button class="tab" class:active={tab === "unlock"} on:click={() => (tab = "unlock")} role="tab" aria-selected={tab === "unlock"}>文件解锁</button>
  <button class="tab" class:active={tab === "export"} on:click={() => (tab = "export")} role="tab" aria-selected={tab === "export"}>导出报告</button>
</div>

{#if tab === "firewall"}
  <section class="section">
    <div class="toolbar">
      <input class="search" bind:value={firewallQ} placeholder="按名称 / 端口过滤" />
      <span class="count">{fwFiltered.length} / {firewallRules.length} 条规则</span>
      <button class="ghost" on:click={loadFirewall} disabled={firewallLoading}>刷新</button>
    </div>
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th class="col-name">名称</th>
            <th>方向</th>
            <th>操作</th>
            <th>协议</th>
            <th>本地端口</th>
            <th>远程IP</th>
            <th class="col-sw">状态</th>
          </tr>
        </thead>
        <tbody>
          {#if firewallLoading}
            <tr><td colspan="7" class="empty">加载中…</td></tr>
          {:else if fwFiltered.length === 0}
            <tr><td colspan="7" class="empty">无匹配规则</td></tr>
          {:else}
            {#each fwFiltered as fw (fw.name)}
              <tr>
                <td class="col-name" title={fw.name}>{fw.name}</td>
                <td>{fw.direction}</td>
                <td><span class="badge-sm" class:allow={fw.action === "Allow"}>{fw.action}</span></td>
                <td>{fw.protocol}</td>
                <td class="mono">{fw.local_port}</td>
                <td class="mono">{fw.remote_ip}</td>
                <td class="col-sw">
                  <button class="switch" class:on={fw.enabled} on:click={() => confirmToggleRule(fw.name, !fw.enabled)}>
                    {fw.enabled ? "已启用" : "已禁用"}
                  </button>
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </div>
  </section>
{:else if tab === "restore"}
  <section class="section">
    <div class="rp-create">
      <input class="search" bind:value={newRpDesc} placeholder="还原点描述（如：安装 XX 前）" style="flex:1" />
      <button class="primary" on:click={confirmCreateRp} disabled={creatingRp || !newRpDesc.trim()}>
        {creatingRp ? "创建中…" : "创建还原点"}
      </button>
    </div>
    <div class="table-wrap" style="margin-top:var(--sp-4)">
      <table>
        <thead>
          <tr><th>序号</th><th>描述</th><th>创建时间</th><th>类型</th></tr>
        </thead>
        <tbody>
          {#if restoreLoading}
            <tr><td colspan="4" class="empty">加载中…</td></tr>
          {:else if restorePoints.length === 0}
            <tr><td colspan="4" class="empty">无可用还原点</td></tr>
          {:else}
            {#each restorePoints as rp (rp.sequence)}
              <tr>
                <td class="num mono">{rp.sequence}</td>
                <td>{rp.description}</td>
                <td class="mono">{rp.creation_time}</td>
                <td>{rp.restore_point_type}</td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </div>
  </section>
{:else if tab === "unlock"}
  <section class="section">
    <div class="unlock-bar">
      <input class="search" bind:value={unlockPath} placeholder="输入文件路径（如 C:\path\to\file.dll）" style="flex:1" />
      <button class="primary" on:click={findLocks} disabled={unlockLoading || !unlockPath.trim()}>
        {unlockLoading ? "查询中…" : "查找占用进程"}
      </button>
    </div>
    {#if unlockSearched}
      <div class="unlock-result" style="margin-top:var(--sp-4)">
        {#if unlockResults.length === 0}
          <p class="muted">未找到占用该文件的进程（或文件未被锁定）。</p>
        {:else}
          <p>找到 {unlockResults.length} 个进程占用此文件：</p>
          <div class="bg-list">
            {#each unlockResults as info (info.pid)}
              <div class="bg-item">
                <span class="bg-name">{info.process_name}</span>
                <span class="muted mono">PID {info.pid}</span>
                <button class="danger" on:click={() => confirmKill(info)}>结束进程</button>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </section>
{:else if tab === "export"}
  <section class="section">
    <div class="export-cards">
      <article class="export-card">
        <h3>系统快照 (JSON)</h3>
        <p class="muted">导出进程列表、端口连接、磁盘报告、启动项的完整 JSON 快照到桌面。</p>
        <button class="primary" on:click={() => doExport("json")} disabled={exportLoading}>
          {exportLoading ? "导出中…" : "导出 JSON"}
        </button>
      </article>
      <article class="export-card">
        <h3>进程列表 (CSV)</h3>
        <p class="muted">导出当前进程列表为 CSV 文件到桌面，可用 Excel 打开分析。</p>
        <button class="primary" on:click={() => doExport("csv")} disabled={exportLoading}>
          {exportLoading ? "导出中…" : "导出 CSV"}
        </button>
      </article>
    </div>
  </section>
{/if}

<!-- 防火墙切换确认 -->
<Modal open={!!fwToggleTarget} title="切换防火墙规则" on:close={() => (fwToggleTarget = null)}>
  {#if fwToggleTarget}
    <p>将<strong>{fwToggleTarget.enable ? "启用" : "禁用"}</strong>规则 <strong>{fwToggleTarget.name}</strong>。</p>
    <p class="muted">此操作需要管理员权限并立即生效。</p>
    <div class="modal-actions">
      <button class="ghost" on:click={() => (fwToggleTarget = null)}>取消</button>
      <button class="primary" on:click={doToggleRule}>确认{fwToggleTarget.enable ? "启用" : "禁用"}</button>
    </div>
  {/if}
</Modal>

<!-- 还原点创建确认 -->
<Modal open={rpConfirmOpen} title="创建系统还原点" on:close={() => (rpConfirmOpen = false)}>
  <p>将创建还原点：<strong>{newRpDesc}</strong></p>
  <p class="muted">创建还原点需要管理员权限，可能需要几秒时间。</p>
  <div class="modal-actions">
    <button class="ghost" on:click={() => (rpConfirmOpen = false)}>取消</button>
    <button class="primary" on:click={doCreateRp}>确认创建</button>
  </div>
</Modal>

<!-- 文件解锁进程终止确认 -->
<Modal open={!!killTarget} title="结束进程确认" on:close={() => (killTarget = null)}>
  {#if killTarget}
    <p>将结束进程 <strong>{killTarget.process_name}</strong>（PID {killTarget.pid}）。</p>
    <p class="muted">未保存的数据可能丢失，此操作不可撤销。</p>
    <div class="modal-actions">
      <button class="ghost" on:click={() => (killTarget = null)}>取消</button>
      <button class="danger" on:click={doKillProcess}>确认结束</button>
    </div>
  {/if}
</Modal>

<style>
  .admin-banner {
    display: flex; align-items: center; gap: var(--sp-4); flex-wrap: wrap;
    padding: 10px 14px; margin-bottom: var(--sp-4);
    border: 1px solid rgba(245, 158, 11, 0.35);
    background: rgba(245, 158, 11, 0.1);
    border-radius: var(--radius-sm); font-size: 13px; color: #fcd34d;
  }
  .tabs {
    display: flex; align-items: center; gap: var(--sp-1);
    margin-bottom: var(--sp-4); border-bottom: 1px solid var(--border);
  }
  .tab {
    border: none; background: transparent; color: var(--text-muted);
    font-family: inherit; font-size: 14px; font-weight: 500;
    padding: 10px 16px; cursor: pointer;
    border-bottom: 2px solid transparent; margin-bottom: -1px;
  }
  .tab:hover { color: var(--text); }
  .tab.active { color: var(--text); border-bottom-color: var(--accent); }

  .section { }
  .toolbar {
    display: flex; align-items: center; gap: var(--sp-3); margin-bottom: var(--sp-4);
  }
  .search {
    flex: 0 0 280px; padding: 8px 12px; border-radius: var(--radius-sm);
    border: 1px solid var(--border); background: var(--surface); color: var(--text);
    font-family: inherit; font-size: 13px;
  }
  .search:focus-visible { outline: 2px solid var(--accent); outline-offset: 1px; }
  .count {
    font-size: 12px; color: var(--text-muted); font-family: var(--font-mono);
  }
  .table-wrap {
    border: 1px solid var(--border); border-radius: var(--radius);
    overflow: hidden; background: var(--surface);
  }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  thead th {
    position: sticky; top: 0; background: var(--surface-2);
    text-align: left; padding: 10px 14px; font-weight: 500;
    color: var(--text-muted); white-space: nowrap;
  }
  tbody { display: block; max-height: calc(100vh - 280px); overflow-y: auto; }
  thead, tbody tr { display: table; width: 100%; table-layout: fixed; }
  td { padding: 8px 14px; border-top: 1px solid var(--border);
       overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  tbody tr:hover { background: var(--surface-2); }
  .col-name { width: 280px; }
  .col-sw { width: 100px; text-align: right; }
  .num { text-align: right; }
  .mono { font-family: var(--font-mono); font-variant-numeric: tabular-nums; }
  .empty { text-align: center; color: var(--text-muted); padding: 40px; }
  .muted { color: var(--text-muted); }

  .primary {
    border: none; background: linear-gradient(135deg, var(--accent), #7c3aed);
    color: #fff; font-family: inherit; font-size: 13px; padding: 8px 16px;
    border-radius: var(--radius-sm); cursor: pointer;
  }
  .primary:disabled { opacity: 0.6; cursor: default; }
  .ghost {
    border: 1px solid var(--border); background: transparent; color: var(--text);
    font-family: inherit; font-size: 13px; padding: 8px 16px;
    border-radius: var(--radius-sm); cursor: pointer;
  }
  .ghost:hover { background: var(--surface-2); }
  .ghost:disabled { opacity: 0.5; cursor: default; }
  .danger {
    border: 1px solid rgba(239, 68, 68, 0.4); background: transparent;
    color: var(--danger); font-family: inherit; font-size: 12px;
    padding: 6px 12px; border-radius: 8px; cursor: pointer;
  }
  .danger:hover { background: rgba(239, 68, 68, 0.12); }

  .switch {
    border: 1px solid var(--border); background: var(--surface-2);
    color: var(--text-muted); font-family: inherit; font-size: 12px;
    padding: 4px 12px; border-radius: 999px; cursor: pointer;
  }
  .switch.on { color: var(--ok); border-color: rgba(34, 197, 94, 0.4); background: rgba(34, 197, 94, 0.12); }
  .badge-sm {
    font-size: 11px; padding: 2px 8px; border-radius: 999px;
    border: 1px solid var(--border);
  }
  .badge-sm.allow { color: var(--ok); border-color: rgba(34, 197, 94, 0.4); background: rgba(34, 197, 94, 0.12); }

  .rp-create { display: flex; gap: var(--sp-3); align-items: center; }
  .unlock-bar { display: flex; gap: var(--sp-3); align-items: center; }
  .bg-list { display: flex; flex-direction: column; gap: 6px; }
  .bg-item {
    display: flex; align-items: center; gap: var(--sp-3); padding: 8px 12px;
    border: 1px solid var(--border); border-radius: var(--radius-sm);
    background: var(--surface); font-size: 13px;
  }
  .bg-name { flex: 1; font-weight: 500; }

  .export-cards {
    display: grid; grid-template-columns: repeat(auto-fit, minmax(320px, 1fr)); gap: var(--sp-4);
  }
  .export-card {
    background: var(--surface); border: 1px solid var(--border);
    border-radius: var(--radius); padding: var(--sp-6);
    display: flex; flex-direction: column; gap: var(--sp-3);
  }
  .export-card h3 { margin: 0; font-size: 15px; }
  .export-card p { margin: 0; font-size: 13px; }
  .modal-actions { display: flex; justify-content: flex-end; gap: var(--sp-2); margin-top: var(--sp-3); }
</style>
