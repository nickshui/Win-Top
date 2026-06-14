<script>
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { pushToast, netTraffic, netTrafficAvailable, relaunchAdmin } from "../stores.js";
  import Modal from "../components/Modal.svelte";

  let rows = [];
  let killTarget = null; // 选中要结束的端口行

  // 网络体检
  let checkup = null;
  let checkupLoading = false;
  let checkupOpen = true; // 结果折叠/展开
  async function runCheckup() {
    checkupLoading = true;
    try {
      checkup = await invoke("network_checkup");
      checkupOpen = true;
    } catch (e) {
      pushToast("网络体检失败：" + e, "error");
    } finally {
      checkupLoading = false;
    }
  }
  const latColor = (ms) => (ms == null ? "" : ms < 30 ? "ok" : ms < 100 ? "warn" : "danger");

  // 自定义检测（域名/IP[:端口]）
  let customInput = "";
  let customResult = null;
  let customLoading = false;
  async function runCustom() {
    if (!customInput.trim()) return;
    customLoading = true;
    try {
      customResult = await invoke("probe_target", { input: customInput.trim() });
    } catch (e) {
      pushToast("检测失败：" + e, "error");
    } finally {
      customLoading = false;
    }
  }

  // 下行测速
  let speed = null;
  let speedLoading = false;
  async function runSpeed() {
    speedLoading = true;
    try {
      speed = await invoke("speed_test");
      if (speed?.error) pushToast("测速失败：" + speed.error, "error");
    } catch (e) {
      pushToast("测速失败：" + e, "error");
    } finally {
      speedLoading = false;
    }
  }
  const speedColor = (m) => (m >= 50 ? "ok" : m >= 10 ? "warn" : "danger");
  let loading = true;
  let q = "";
  let proto = "all"; // all | TCP | UDP
  let family = "all"; // all | IPv4 | IPv6
  let listeningOnly = false;
  let timer;

  let tab = "traffic"; // traffic | ports

  // 进程流量表：本地排序状态 + 格式化
  let tSortKey = "down_bps";
  let tSortDir = -1; // -1 降序, 1 升序
  function setTSort(k) {
    if (tSortKey === k) tSortDir = -tSortDir;
    else {
      tSortKey = k;
      tSortDir = k === "name" ? 1 : -1;
    }
  }
  const tArrow = (k) => (tSortKey === k ? (tSortDir === -1 ? " ↓" : " ↑") : "");

  const fmtRate = (bps) => {
    if (!bps || bps < 1) return "0 B/s";
    if (bps < 1024) return `${bps.toFixed(0)} B/s`;
    if (bps < 1024 * 1024) return `${(bps / 1024).toFixed(1)} KB/s`;
    return `${(bps / 1024 / 1024).toFixed(2)} MB/s`;
  };
  const fmtBytes = (n) => {
    if (!n) return "0";
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
    return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
  };

  $: traffic = $netTraffic;
  $: trafficRows = (traffic?.rows ?? [])
    .slice()
    .sort((a, b) => {
      if (tSortKey === "name") return tSortDir * a.name.localeCompare(b.name);
      return tSortDir * (a[tSortKey] - b[tSortKey]);
    });
  $: top3 = (traffic?.rows ?? [])
    .slice()
    .sort((a, b) => b.down_bps - a.down_bps)
    .slice(0, 3);

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

<section class="nettools">
  <header class="nt-head">
    <h2 class="section-title">网络工具</h2>
    <button
      class="nt-collapse"
      on:click={() => (checkupOpen = !checkupOpen)}
      aria-expanded={checkupOpen}
    >
      {checkupOpen ? "收起" : "展开"}
      <span class="chev" class:open={checkupOpen}>▾</span>
    </button>
  </header>

  {#if checkupOpen}
    <!-- 动作栏：体检 / 测速 / 自定义检测 -->
    <div class="nt-actions">
      <button class="primary" on:click={runCheckup} disabled={checkupLoading}>
        {checkupLoading ? "体检中…" : "一键体检"}
      </button>
      <button class="primary alt" on:click={runSpeed} disabled={speedLoading}>
        {speedLoading ? "测速中…" : "下行测速"}
      </button>
      <div class="nt-custom">
        <input
          class="search"
          bind:value={customInput}
          on:keydown={(e) => e.key === "Enter" && runCustom()}
          placeholder="域名 / IP[:端口]，如 github.com:443"
          aria-label="自定义检测目标"
        />
        <button class="ghost" on:click={runCustom} disabled={customLoading}>
          {customLoading ? "检测中…" : "检测"}
        </button>
      </div>
    </div>

    {#if customResult}
      <div class="custom-result">
        {#if customResult.error}
          <span class="cr-err">{customResult.error}</span>
        {:else}
          <span class="cr-item">解析 <b class="mono">{customResult.resolved.join(", ")}</b></span>
          {#if customResult.ping}
            <span class="cr-item">延迟
              {#if customResult.ping.ok}
                <b class="lat {latColor(customResult.ping.latency_ms)}">{customResult.ping.latency_ms} ms</b>
              {:else}<b class="lat danger">超时</b>{/if}
            </span>
          {/if}
          {#if customResult.tcp}
            <span class="cr-item">端口 {customResult.tcp.port}
              {#if customResult.tcp.ok}
                <b class="lat ok">通 · {customResult.tcp.latency_ms} ms</b>
              {:else}<b class="lat danger">不通</b>{/if}
            </span>
          {/if}
        {/if}
      </div>
    {/if}

    {#if speed && !speed.error}
      <div class="speed-bar">
        <span class="ck-label">下行带宽</span>
        <span class="speed-val {speedColor(speed.down_mbps)}">
          {speed.down_mbps.toFixed(1)}<small> Mbps</small>
        </span>
        <span class="speed-meta mono">
          {(speed.bytes / 1e6).toFixed(1)} MB · {speed.secs.toFixed(1)}s
          {#if speed.streams}· {speed.streams} 路并发{/if}
        </span>
      </div>
    {/if}

  {#if checkup}
    <div class="checkup-grid">
      <!-- 延迟 -->
      <div class="ck-card">
        <div class="ck-label">延迟 (ICMP)</div>
        <div class="ping-list">
          {#each checkup.pings as p}
            <div class="ping-row">
              <span class="ping-name">{p.label}<span class="ping-tgt">{p.target}</span></span>
              {#if p.ok}
                <span class="lat {latColor(p.latency_ms)}">{p.latency_ms} ms</span>
              {:else}
                <span class="lat danger" title={p.error}>—</span>
              {/if}
            </div>
          {/each}
        </div>
      </div>

      <!-- IP / DNS -->
      <div class="ck-card">
        <div class="ck-label">公网 IP</div>
        <div class="ck-value mono">{checkup.public_ip ?? "获取失败"}</div>
        <div class="ck-label" style="margin-top:12px">DNS 解析</div>
        <div class="ck-value mono">{checkup.dns_ms != null ? `${checkup.dns_ms} ms` : "失败"}</div>
      </div>

      <!-- 网卡 -->
      <div class="ck-card wide">
        <div class="ck-label">本机网卡</div>
        <div class="adapters">
          {#each checkup.adapters as a}
            <div class="adapter">
              <span class="ad-name">{a.name}</span>
              <span class="ad-ips mono">{a.ips.join(", ")}</span>
              {#if a.gateway}<span class="ad-gw mono">网关 {a.gateway}</span>{/if}
            </div>
          {/each}
        </div>
      </div>
    </div>
  {:else if !checkupLoading}
    <p class="ck-hint">
      「一键体检」：本机 IP / 网关 / 公网 IP / 延迟（国内+海外）/ DNS 解析 ·
      「下行测速」：带宽 Mbps ·
      右侧输入框可检测任意 域名/IP/端口。
    </p>
  {/if}
  {/if}
</section>

<div class="tabs" role="tablist">
  <button
    class="tab"
    class:active={tab === "traffic"}
    role="tab"
    aria-selected={tab === "traffic"}
    on:click={() => (tab = "traffic")}
  >
    进程流量
  </button>
  <button
    class="tab"
    class:active={tab === "ports"}
    role="tab"
    aria-selected={tab === "ports"}
    on:click={() => (tab = "ports")}
  >
    端口连接 <span class="tab-badge">{rows.length}</span>
  </button>
  <span class="tab-meta">
    {#if tab === "traffic"}
      {#if $netTrafficAvailable && traffic}
        <span class="total mono">↓ {fmtRate(traffic.total_down_bps)}　↑ {fmtRate(traffic.total_up_bps)}</span>
      {/if}
    {:else}
      <span class="count">{filtered.length} / {rows.length} · 监听 {listenCount} · 每 3s 刷新</span>
    {/if}
  </span>
</div>

{#if tab === "traffic"}
  {#if !$netTrafficAvailable}
    <div class="gate">
      <p>每进程网络流量监测基于实时 ETW，需要<strong>管理员权限</strong>。</p>
      <button class="primary" on:click={relaunchAdmin}>以管理员重启</button>
    </div>
  {:else if !traffic}
    <p class="ck-hint">正在采集流量数据…</p>
  {:else}
    {#if top3.length > 0}
      <div class="top-strip">
        <span class="ts-label">Top</span>
        {#each top3 as t (t.pid)}
          <span class="ts-pill" title={t.name}>
            <span class="ts-name">{t.name}</span>
            <span class="ts-rate mono">↓ {fmtRate(t.down_bps)}</span>
          </span>
        {/each}
      </div>
    {/if}

    <div class="table-wrap tab-table sortable">
      <table>
        <thead>
          <tr>
            <th class="col-proc" on:click={() => setTSort("name")}>进程{tArrow("name")}</th>
            <th class="num" on:click={() => setTSort("pid")}>PID{tArrow("pid")}</th>
            <th class="num" on:click={() => setTSort("down_bps")}>↓ 下载{tArrow("down_bps")}</th>
            <th class="num" on:click={() => setTSort("up_bps")}>↑ 上传{tArrow("up_bps")}</th>
            <th class="num" on:click={() => setTSort("recv_total")}>累计↓{tArrow("recv_total")}</th>
            <th class="num" on:click={() => setTSort("sent_total")}>累计↑{tArrow("sent_total")}</th>
          </tr>
        </thead>
        <tbody>
          {#if trafficRows.length === 0}
            <tr><td colspan="6" class="empty">暂无有流量的进程</td></tr>
          {:else}
            {#each trafficRows as t (t.pid)}
              <tr>
                <td class="col-proc" title={t.name}>{t.name}</td>
                <td class="num mono">{t.pid}</td>
                <td class="num mono">{fmtRate(t.down_bps)}</td>
                <td class="num mono">{fmtRate(t.up_bps)}</td>
                <td class="num mono">{fmtBytes(t.recv_total)}</td>
                <td class="num mono">{fmtBytes(t.sent_total)}</td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </div>
  {/if}
{:else}
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
  </div>

  <div class="table-wrap tab-table">
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
{/if}

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

  /* 网络工具 */
  .nettools {
    margin-bottom: var(--sp-6);
    padding: var(--sp-4) var(--sp-6);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
  }
  .nt-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .section-title {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }
  .nt-collapse {
    display: flex;
    align-items: center;
    gap: 6px;
    background: rgba(99, 102, 241, 0.12);
    border: 1px solid rgba(99, 102, 241, 0.45);
    color: #c7d2fe;
    font-family: inherit;
    font-size: 13px;
    font-weight: 500;
    padding: 6px 14px;
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background 0.15s ease, border-color 0.15s ease;
  }
  .nt-collapse:hover {
    background: rgba(99, 102, 241, 0.2);
    border-color: var(--accent);
  }
  .nt-collapse .chev {
    color: #c7d2fe;
  }
  .nt-actions {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    flex-wrap: wrap;
    margin-top: var(--sp-4);
  }
  .nt-custom {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin-left: auto;
  }
  .nt-custom .search {
    flex: 0 0 300px;
  }
  .nt-custom .ghost {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
    padding: 8px 16px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .nt-custom .ghost:hover {
    background: var(--surface-2);
  }
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
  .primary.alt {
    background: linear-gradient(135deg, #0ea5e9, var(--accent));
  }
  .primary:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .speed-bar {
    display: flex;
    align-items: baseline;
    gap: var(--sp-3);
    margin-top: var(--sp-4);
    padding: var(--sp-3) var(--sp-4);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .speed-val {
    font-family: var(--font-mono);
    font-size: 24px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
  }
  .speed-val small {
    font-size: 13px;
    font-weight: 400;
  }
  .speed-val.ok {
    color: var(--ok);
  }
  .speed-val.warn {
    color: var(--warn);
  }
  .speed-val.danger {
    color: var(--danger);
  }
  .speed-meta {
    color: var(--text-muted);
    font-size: 12px;
  }
  .ck-hint {
    margin: var(--sp-3) 0 0;
    font-size: 13px;
    color: var(--text-muted);
  }
  .checkup-grid {
    margin-top: var(--sp-4);
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: var(--sp-3);
  }
  .ck-card {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: var(--sp-3) var(--sp-4);
  }
  .ck-card.wide {
    grid-column: 1 / -1;
  }
  .ck-label {
    font-size: 12px;
    color: var(--text-muted);
  }
  .ck-value {
    font-size: 18px;
    font-weight: 600;
    margin-top: 4px;
  }
  .mono {
    font-family: var(--font-mono);
  }
  .ping-list {
    margin-top: var(--sp-2);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .ping-row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    font-size: 13px;
  }
  .ping-name {
    color: var(--text);
  }
  .ping-tgt {
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 11px;
    margin-left: 8px;
  }
  .lat {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    font-weight: 600;
  }
  .lat.ok {
    color: var(--ok);
  }
  .lat.warn {
    color: var(--warn);
  }
  .lat.danger {
    color: var(--danger);
  }
  .adapters {
    margin-top: var(--sp-2);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .adapter {
    display: flex;
    gap: var(--sp-3);
    align-items: baseline;
    flex-wrap: wrap;
    font-size: 13px;
  }
  .ad-name {
    font-weight: 500;
    min-width: 140px;
  }
  .ad-ips {
    color: #93c5fd;
  }
  .ad-gw {
    color: var(--text-muted);
    font-size: 12px;
  }

  .chev {
    font-size: 11px;
    transition: transform 0.15s ease;
    transform: rotate(-90deg);
  }
  .chev.open {
    transform: rotate(0deg);
  }
  .custom-result {
    margin-top: var(--sp-3);
    padding: var(--sp-2) var(--sp-3);
    background: var(--bg);
    border-radius: var(--radius-sm);
    display: flex;
    gap: var(--sp-4);
    align-items: baseline;
    flex-wrap: wrap;
    font-size: 13px;
    color: var(--text-muted);
  }
  .cr-item b {
    color: var(--text);
    margin-left: 4px;
  }
  .cr-err {
    color: var(--danger);
  }
  /* 标签页 */
  .tabs {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    margin-bottom: var(--sp-4);
    border-bottom: 1px solid var(--border);
  }
  .tab {
    position: relative;
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
    transition: color 0.15s ease, border-color 0.15s ease;
  }
  .tab:hover {
    color: var(--text);
  }
  .tab.active {
    color: var(--text);
    border-bottom-color: var(--accent);
  }
  .tab:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .tab-badge {
    font-size: 11px;
    color: var(--text-muted);
    background: var(--surface-2);
    padding: 1px 7px;
    border-radius: 999px;
    margin-left: 2px;
    font-variant-numeric: tabular-nums;
  }
  .tab-meta {
    margin-left: auto;
    display: flex;
    align-items: center;
  }
  .total {
    font-size: 14px;
    font-weight: 600;
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  /* 标签内表格：单表全高，独占视图滚动 */
  .tab-table tbody {
    max-height: calc(100vh - 260px);
  }
  .sortable thead th {
    cursor: pointer;
    user-select: none;
  }
  .sortable thead th:hover {
    color: var(--text);
  }
  .gate {
    display: flex;
    align-items: center;
    gap: var(--sp-4);
    flex-wrap: wrap;
    font-size: 13px;
    color: var(--text-muted);
    padding: var(--sp-6);
    border: 1px dashed var(--border);
    border-radius: var(--radius);
    background: var(--surface);
  }
  /* Top 流量进程紧凑条 */
  .top-strip {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    flex-wrap: wrap;
    margin-bottom: var(--sp-3);
  }
  .ts-label {
    font-size: 11px;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .ts-pill {
    display: inline-flex;
    align-items: baseline;
    gap: 8px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 4px 12px;
    font-size: 12px;
    max-width: 260px;
  }
  .ts-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
  }
  .ts-rate {
    color: var(--accent);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }
</style>
