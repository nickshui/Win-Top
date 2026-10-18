<script>
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { pushToast, diskIo, gpuProc } from "../stores.js";
  import Modal from "../components/Modal.svelte";

  let rows = [];
  let loading = true;
  let q = "";
  let sortKey = "cpu";
  let sortDir = -1;
  let timer;
  let viewMode = "list";

  // 刷新控制
  const intervals = [
    { label: "1s", ms: 1000 },
    { label: "2s", ms: 2000 },
    { label: "5s", ms: 5000 },
    { label: "暂停", ms: 0 },
  ];
  let refreshMs = 2000;
  // 交互时临时冻结：选中行 / 打开菜单时暂停刷新，避免行跳位难以选中
  let frozen = false;

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

  // ——— 列宽体系（唯一宽度真源） ———
  // cols 只在拖拽/键盘/装载时整体重建；宽度一律来自这里，DOM 里不再出现第二份列宽。
  // flex 列（操作）不写宽度，用来吸收容器余量 —— 其余列的「渲染宽 === 声明宽」，
  // 所以表格宽度永远是 max(容器宽, Σ列宽)，与窗口尺寸无关。
  const COLS_DEFAULT = [
    { key: "name", label: "进程",  w: 260, min: 120, max: 720, sort: "name" },
    { key: "pid",  label: "PID",   w: 58,  min: 48,  max: 140, sort: "pid",       num: true },
    { key: "cpu",  label: "CPU",   w: 66,  min: 48,  max: 120, sort: "cpu",       num: true },
    { key: "mem",  label: "内存",  w: 80,  min: 56,  max: 160, sort: "mem_mb",    num: true },
    { key: "gpu",  label: "GPU",   w: 62,  min: 48,  max: 120, sort: "gpu_pct",   num: true,
      hint: "最忙引擎占用（Task Manager 口径）" },
    // 显存列：只报「专用显存」。表头 hint 是口径说明，不是装饰 —— 少了它这个数字必然被误读成
    // 「显卡总占用」。实测（两个计数器放进同一次 Get-Counter，消除时序差，两次采样一致）：
    //   各进程**专用**相加 10.24 GiB vs 显卡自身 5.02 GiB → 2.04×
    //   各进程**共享**相加  2.31 GiB vs 显卡自身 2.31 GiB → 1.00×（分毫不差）
    // 共享侧完全吻合 ⇒ 读取方法没问题；是「专用」这个 per-process 计数器把跨进程共享的画面显存
    // 同时记在每个持有它的进程名下（dwm.exe 单进程就报得比整卡还多）。
    { key: "vram", label: "显存",  w: 88,  min: 64,  max: 160, sort: "vram_dedicated_mb", num: true,
      hint: "进程独占的显存（专用）。核显机器上专用显存恒为 0（核显没有独立显存），其占用体现在共享内存里，见详情面板。注意：各进程的专用显存不能相加得到显卡占用 —— 跨进程共享的画面显存会被同时记在每个持有它的进程名下（本机实测：各进程相加 10.24 GiB，而显卡自身只报 5.02 GiB；dwm.exe 单个进程就报 5.72 GiB，比整卡还多），dwm.exe / csrss.exe 尤其偏大。" },
    { key: "thr",  label: "线程",  w: 62,  min: 44,  max: 120, sort: "threads",   num: true },
    { key: "ior",  label: "磁盘R", w: 72,  min: 56,  max: 140, sort: "read_bps",  num: true },
    { key: "iow",  label: "磁盘W", w: 72,  min: 56,  max: 140, sort: "write_bps", num: true },
    { key: "act",  label: "操作",  w: 146, min: 128, max: 300, flex: true },
  ];
  const LS_KEY = "wintop.processes.colwidths.v1";
  const STEP = 8;                                                        // 键盘微调步长
  const clampW = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
  const sumW = COLS_DEFAULT.reduce((a, c) => a + c.w, 0);                // 966（含 显存 88）

  // 深树缩进压缩：depth ≤ 4 每级 18px，之后每级 6px。
  // 首列默认 260px，不压缩的话 depth 6 就要吃掉 108px，长进程名会被完全挤掉。
  const indentOf = (d) => Math.min(d, 4) * 18 + Math.max(0, d - 4) * 6;

  let cols = COLS_DEFAULT.map((c) => ({ ...c }));
  let tableMin = sumW;                                                   // <table> 的 min-width
  const refreshMin = () => { tableMin = cols.reduce((a, c) => a + c.w, 0); };
  const widthOf = (key) => cols.find((c) => c.key === key)?.w ?? 0;
  const defaultW = (key) => COLS_DEFAULT.find((c) => c.key === key)?.w ?? 0;

  const defaults = () => Object.fromEntries(COLS_DEFAULT.map((c) => [c.key, c.w]));

  // 只信任 COLS 里存在的 key；非有限数丢弃、越界夹取、解析失败静默回落默认值
  function loadWidths() {
    const out = defaults();
    let raw = null;
    try { raw = localStorage.getItem(LS_KEY); } catch { return out; }
    if (!raw) return out;
    let parsed;
    try { parsed = JSON.parse(raw); } catch { return out; }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return out;
    for (const c of COLS_DEFAULT) {
      if (c.flex) continue;
      const v = parsed[c.key];
      // 键缺失（旧版本存过、之后新增的列，例如「显存」）走这里 continue，
      // 保留 defaults() 里的默认宽度 —— 这就是新列对老 localStorage 的兼容路径，
      // 不会变成 undefined/NaN，也不需要清用户的存档或升级 key 版本。
      if (typeof v !== "number" || !Number.isFinite(v)) continue;
      out[c.key] = clampW(Math.round(v), c.min, c.max);
    }
    return out;
  }

  function saveWidths() {
    const out = {};
    for (const c of cols) if (!c.flex) out[c.key] = c.w;
    try { localStorage.setItem(LS_KEY, JSON.stringify(out)); } catch { /* 配额/隐私模式：退化为会话内生效 */ }
  }

  // 返回是否真的改变了宽度（不变则不重建对象、不重排）
  function applyWidth(key, w) {
    const c = COLS_DEFAULT.find((x) => x.key === key);
    if (!c || c.flex) return false;
    const next = clampW(Math.round(w), c.min, c.max);
    if (next === widthOf(key)) return false;
    cols = cols.map((x) => (x.key === key ? { ...x, w: next } : x));
    refreshMin();
    queueMeasure();
    return true;
  }

  // ——— 列宽拖拽（pointer capture，指针移出窗口也不丢） ———
  let drag = null;                       // { key, startX, startW }
  let pendingW = 0;
  let rafId = 0;
  let lastDown = { key: null, t: 0, moved: false };

  function onResizeDown(e, c) {
    if (c.flex || e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();

    // 双击复位：pointerdown 上的 preventDefault 会抑制后续兼容鼠标事件（含 dblclick），
    // 所以这里自己识别 350ms 内的第二次按下。
    // 必须叠加「上一次按下没有产生位移」：否则拖完一列后马上再拖同一列会被误判成
    // 双击而复位（渲染验证实测发现）。
    const now = Date.now();
    const isDbl = lastDown.key === c.key && now - lastDown.t < 350 && !lastDown.moved;
    lastDown = { key: c.key, t: now, moved: false };
    if (isDbl) { applyWidth(c.key, defaultW(c.key)); saveWidths(); return; }

    drag = { key: c.key, startX: e.clientX, startW: widthOf(c.key) };
    pendingW = drag.startW;
    e.currentTarget.setPointerCapture(e.pointerId);
    e.currentTarget.focus();
  }

  function onResizeMove(e) {
    if (!drag) return;
    if (Math.abs(e.clientX - drag.startX) > 2) lastDown.moved = true;  // 有位移即不算双击
    pendingW = drag.startW + (e.clientX - drag.startX);
    if (rafId) return;                   // rAF 合帧：一帧最多一次 Svelte 更新
    rafId = requestAnimationFrame(() => { rafId = 0; if (drag) applyWidth(drag.key, pendingW); });
  }

  function onResizeUp(e) {
    if (!drag) return;                   // pointerup 之后 lostpointercapture 还会来，这里挡掉
    const key = drag.key;
    if (rafId) { cancelAnimationFrame(rafId); rafId = 0; }
    drag = null;
    if (e?.currentTarget) {
      if (e.currentTarget.hasPointerCapture?.(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    }
    applyWidth(key, pendingW);           // 收尾帧若被取消，这里补上
    saveWidths();                        // 拖拽过程不写盘，落盘只发生在这里
  }

  function onResizeDblClick(e, c) {
    if (c.flex) return;
    e.preventDefault();
    e.stopPropagation();
    applyWidth(c.key, defaultW(c.key));
    saveWidths();
  }

  function onResizeKey(e, c) {
    if (c.flex) return;
    const cur = widthOf(c.key);
    let target;
    switch (e.key) {
      case "ArrowLeft": target = cur - STEP; break;
      case "ArrowRight": target = cur + STEP; break;
      case "Home": target = c.min; break;          // 最窄
      case "End": target = defaultW(c.key); break; // 恢复默认（与双击等价）
      default: return;
    }
    e.preventDefault();                  // Home/End/方向键不要滚动容器或页面
    e.stopPropagation();
    if (applyWidth(c.key, target)) saveWidths();
  }

  // ——— 单一滚动容器 + 顶部滑动条 ———
  // tbody 不再是滚动容器：表头与表体共享同一个布局上下文，吸顶与横向对齐由结构保证。
  let wrap = null;                       // .table-wrap（唯一滚动容器）
  let tableEl = null;
  let topBar = null;                     // .hscroll-top
  let spacerW = 0;
  let hOverflow = false;
  let sbw = 15;                          // 原生滚动条厚度探针值
  let ro = null;
  let measureRaf = 0;

  function measureScrollbarWidth() {
    try {
      const p = document.createElement("div");
      p.style.cssText = "position:absolute;top:-9999px;width:100px;height:100px;overflow:scroll";
      document.body.appendChild(p);
      const w = p.offsetWidth - p.clientWidth;
      p.remove();
      return w > 0 ? w : 15;
    } catch { return 15; }
  }

  // spacer 宽 = 主容器可滚动量 + 顶部条自身可视宽 → 两侧 scrollLeft 取值范围严格相等，
  // 同步逻辑可以直接赋值，不需要按比例换算（因而没有浮点误差与极值回弹）。
  function measure() {
    if (!wrap || !topBar) return;
    const over = wrap.scrollWidth - wrap.clientWidth;
    hOverflow = over > 1;
    spacerW = Math.max(0, over + topBar.clientWidth);
    if (Math.abs(topBar.scrollLeft - wrap.scrollLeft) >= 1) topBar.scrollLeft = wrap.scrollLeft;
  }

  function queueMeasure() {
    if (measureRaf) return;
    measureRaf = requestAnimationFrame(() => { measureRaf = 0; measure(); });
  }

  // 双向同步：值比较是防回环的根本保证（赋一个它已经持有的值不会再派发 scroll）
  function onTopScroll() {
    if (!wrap || !topBar) return;
    if (Math.abs(wrap.scrollLeft - topBar.scrollLeft) < 1) return;
    wrap.scrollLeft = topBar.scrollLeft;
  }

  function onWrapScroll() {
    if (!wrap || !topBar) return;
    if (Math.abs(topBar.scrollLeft - wrap.scrollLeft) < 1) return;
    topBar.scrollLeft = wrap.scrollLeft;
  }

  async function refresh() {
    if (frozen) return; // 冻结期间跳过（保持选中稳定）
    try {
      rows = await invoke("get_processes");
      loading = false;
      queueMeasure();   // 行数跨过视口高度时纵向滚动条出现/消失，重算一次横向溢出
    } catch (e) {
      pushToast("加载进程失败：" + e, "error");
    }
  }

  function restartTimer() {
    if (timer) clearInterval(timer);
    timer = null;
    if (refreshMs > 0) timer = setInterval(refresh, refreshMs);
  }

  function setInterval_(ms) {
    refreshMs = ms;
    restartTimer();
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

  onMount(() => {
    const saved = loadWidths();
    cols = cols.map((c) => (c.flex ? c : { ...c, w: saved[c.key] }));
    refreshMin();
    sbw = measureScrollbarWidth();
    refresh();
    restartTimer();
    if (typeof ResizeObserver !== "undefined") {
      ro = new ResizeObserver(() => queueMeasure());
      if (wrap) ro.observe(wrap);
      if (tableEl) ro.observe(tableEl);
    }
    queueMeasure();
  });

  onDestroy(() => {
    if (timer) clearInterval(timer);
    if (ro) { ro.disconnect(); ro = null; }
    if (measureRaf) { cancelAnimationFrame(measureRaf); measureRaf = 0; }
    if (rafId) { cancelAnimationFrame(rafId); rafId = 0; }
  });

  // 选中某行详情、或打开结束/优先级菜单时，冻结刷新以保证选中稳定。
  $: frozen = detailPid != null || termTarget != null || prioTarget != null;

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

  // GPU map：某 PID 不在 rows 里 = 它确实没在用 GPU（0%），只有 available === false
  // 才是「计数器不可用」，两者在 UI 上必须区分开。
  $: gpuMap = new Map((($gpuProc?.rows) ?? []).map((r) => [r.pid, r]));
  $: gpuOk = !!$gpuProc?.available;
  $: detailGpu = detailPid != null ? (gpuMap.get(detailPid) ?? null) : null;

  // 卡名映射：per_adapter 是冻结契约，只有 luid、没有 name，必须用同一 tick 的 adapters[] 反查。
  // 映射缺失（旧 payload、拔卡、后端未落地）时兜底 Adapter 0x<lo>，与概览卡片 adapterLabel 同一风格，
  // 绝不显示空串或 undefined。
  $: adapterByLuid = new Map((($gpuProc?.adapters) ?? []).map((a) => [a.luid, a]));
  const adapterLabel = (luid) => {
    const n = (adapterByLuid.get(luid)?.name ?? "").trim();
    if (n) return n;
    const lo = String(luid ?? "").split(":").pop().trim();
    return lo ? `Adapter ${lo}` : "Adapter（未知 LUID）";
  };
  // 详情面板里的显存一律 MB（与既有「GPU 显存」行同口径）；字段缺失记 0 而不是崩
  const mb1 = (v) => { const n = Number(v); return (Number.isFinite(n) ? n : 0).toFixed(1) + " MB"; };

  // ——— 显存（专用） ———
  // 只取 vram_dedicated_mb：共享内存混进来会把系统内存误报成显存（Qoder/Trae 这类
  // 进程专用为 0、共享几十 MB，混算后就成了"显存"）。0 显示 "0" 而不是 "0.0 MB"，省宽度。
  const fmtVram = (mb) => {
    const v = Number(mb);
    if (!Number.isFinite(v) || v <= 0) return "0";
    return v >= 1024 ? (v / 1024).toFixed(1) + " GB" : v.toFixed(1) + " MB";
  };
  // 单元格 title：胜出卡名 + 专用/共享。措辞必须点明这是**跨卡汇总**，
  // 否则会和详情面板里「GPU 逐卡分解」的逐卡数值混为一谈。
  const vramTitle = (p) => {
    if (!gpuOk) return "GPU 性能计数器不可用，显存数据不可用";
    const detail = `专用 ${(p.vram_dedicated_mb ?? 0).toFixed(1)} MB · 共享 ${(p.vram_shared_mb ?? 0).toFixed(1)} MB`;
    const label = p.gpu_luid ? adapterLabel(p.gpu_luid) : "";   // 无胜出卡（luid 空）→ 省略卡名段
    return (label ? `${label} · ${detail}` : detail) + "（该进程的汇总值，跨卡）";
  };

  // 合并进程列表与磁盘 I/O / GPU，使其可排序
  $: merged = filtered.map(p => {
    const g = gpuMap.get(p.pid);
    return {
      ...p,
      read_bps: diskIoMap.get(p.pid)?.read_bps ?? 0,
      write_bps: diskIoMap.get(p.pid)?.write_bps ?? 0,
      gpu_pct: g?.gpu_pct ?? 0,
      gpu_sum_pct: g?.gpu_sum_pct ?? 0,
      gpu_engtype: g?.engtype ?? "",
      gpu_engines: g?.engines ?? [],
      gpu_luid: g?.luid ?? "",                 // 胜出卡，用于显存列 tooltip 标注卡名
      // 不在 rows 里 = 该进程当前没占 GPU 内存 → 0；不是"未知"
      vram_dedicated_mb: g?.vram_dedicated_mb ?? 0,
      vram_shared_mb: g?.vram_shared_mb ?? 0,
    };
  }).sort((a, b) => {
    if (sortKey === "name") return sortDir * a.name.localeCompare(b.name);
    if (sortKey === "read_bps" || sortKey === "write_bps") return sortDir * (a[sortKey] - b[sortKey]);
    return sortDir * ((a[sortKey] ?? 0) - (b[sortKey] ?? 0));
  });

  // 空状态时表格不设 min-width，避免「没有内容的横向滚动条」
  $: emptyTable = loading || filtered.length === 0;
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
  <div class="refresh-group" title="刷新间隔">
    {#each intervals as it}
      <button
        class="int-btn"
        class:active={refreshMs === it.ms}
        on:click={() => setInterval_(it.ms)}
      >{it.label}</button>
    {/each}
  </div>
  {#if $gpuProc && !gpuOk}
    <span class="chip-warn" title={$gpuProc.reason || "GPU 性能计数器不可用"}>GPU 计数器不可用</span>
  {/if}
  <span class="count">
    {merged.length} / {rows.length} 进程
    {#if frozen}
      · <span class="frozen-tag">已冻结</span>
    {:else if refreshMs > 0}
      · 每 {refreshMs / 1000}s 刷新
    {:else}
      · 手动
    {/if}
  </span>
</div>

<div class="content-area">
  <div class="table-stack" class:with-detail={!!detailPid} style="--sbw:{sbw}px">
    <!-- 顶部滑动条：结构上在滚动容器之外，用 spacer 撑出与表格相同的宽度，双向同步 scrollLeft -->
    <div
      class="hscroll-top"
      class:idle={!hOverflow}
      bind:this={topBar}
      on:scroll={onTopScroll}
      aria-hidden="true"
      tabindex="-1"
    >
      <div class="hscroll-spacer" style="width:{spacerW}px"></div>
    </div>

    <div class="table-wrap" bind:this={wrap} on:scroll={onWrapScroll}>
      <table bind:this={tableEl} style="min-width:{emptyTable ? 0 : tableMin}px">
        <colgroup>
          {#each cols as c (c.key)}
            <col style={c.flex ? undefined : `width:${c.w}px`} />
          {/each}
        </colgroup>
        <thead>
          <tr>
            {#each cols as c (c.key)}
              <th
                class="col-{c.key}"
                class:num={c.num}
                class:sortable={!!c.sort}
                title={c.hint}
                on:click={() => { if (c.sort) setSort(c.sort); }}
              >
                <span class="th-label">{c.label}{c.sort ? arrow(c.sort) : ""}</span>
                {#if !c.flex}
                  <!-- 列宽手柄：8px 热区 + 1px 视觉分隔线；双击复位，方向键/Home/End 可调。
                       可聚焦的 separator 在 ARIA 1.2 里就是 widget role（Window Splitter 模式，
                       aria-valuenow/min/max 只对可聚焦 separator 合法），Svelte 的 role 表把它算作
                       non-interactive，两条 a11y 警告是误报，故显式忽略。 -->
                  <!-- svelte-ignore a11y-no-noninteractive-tabindex a11y-no-noninteractive-element-interactions -->
                  <span
                    class="col-resizer"
                    class:resizing={drag?.key === c.key}
                    role="separator"
                    aria-orientation="vertical"
                    tabindex="0"
                    aria-label="调整「{c.label}」列宽"
                    aria-valuenow={c.w}
                    aria-valuemin={c.min}
                    aria-valuemax={c.max}
                    title="左右拖动调整列宽，双击恢复默认"
                    on:pointerdown={(e) => onResizeDown(e, c)}
                    on:pointermove={onResizeMove}
                    on:pointerup={onResizeUp}
                    on:pointercancel={onResizeUp}
                    on:lostpointercapture={onResizeUp}
                    on:dblclick={(e) => onResizeDblClick(e, c)}
                    on:keydown={(e) => onResizeKey(e, c)}
                    on:click|stopPropagation={() => {}}
                  ></span>
                {/if}
              </th>
            {/each}
          </tr>
        </thead>
        <tbody>
          {#if loading}
            <tr><td colspan={cols.length} class="empty">加载中…</td></tr>
          {:else if filtered.length === 0}
            <tr><td colspan={cols.length} class="empty">无匹配进程</td></tr>
          {:else if viewMode === "tree"}
            {#each flatTree as p (p.pid)}
              <tr class="tree-row" class:active-row={detailPid === p.pid} on:click={() => showDetail(p.pid)}>
                <td class="col-name" title={p.name}>
                  <span style="padding-left:{indentOf(p._depth)}px">
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
                <td class="col-gpu num mono">
                  {#if gpuOk}<span class="cpu {sev(p.gpu_pct)}">{p.gpu_pct.toFixed(1)}%</span>{:else}—{/if}
                </td>
                <td class="col-vram num mono" title={vramTitle(p)}>{#if gpuOk}{fmtVram(p.vram_dedicated_mb)}{:else}—{/if}</td>
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
                <td class="col-gpu num mono">
                  {#if gpuOk}<span class="cpu {sev(p.gpu_pct)}">{p.gpu_pct.toFixed(1)}%</span>{:else}—{/if}
                </td>
                <td class="col-vram num mono" title={vramTitle(p)}>{#if gpuOk}{fmtVram(p.vram_dedicated_mb)}{:else}—{/if}</td>
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
          {#if gpuOk}
            <dt>GPU 占用</dt>
            <dd class="mono">
              {(detailGpu?.gpu_pct ?? 0).toFixed(1)}%
              <span class="muted gpu-eng">{detailGpu?.engtype || "—"}</span>
            </dd>
            <dt>引擎占用合计</dt>
            <dd class="mono" title="胜出适配器上全部引擎的占用之和；同一份工作可能被多个引擎计入，因此可超过 100%">
              {(detailGpu?.gpu_sum_pct ?? 0).toFixed(1)}%
            </dd>
            <dt>引擎分解</dt>
            <dd class="gpu-engines" title="只列该进程占用最高的前 5 个引擎（后端最多给 8 个），所以合计小于上面的「引擎占用合计」">
              {#if detailGpu?.engines?.length}
                {#each detailGpu.engines.slice(0, 5) as eng}
                  <span class="gpu-eng-item">{eng.engtype || "?"} {eng.pct.toFixed(1)}%</span>
                {/each}
              {:else}<span class="muted">—</span>{/if}
            </dd>
            <dt>GPU 显存</dt>
            <dd class="mono" title="该进程在胜出适配器上的显存占用（专用/共享）；数据来自 gpu-proc 事件，不额外查询">
              专用 {(detailGpu?.vram_dedicated_mb ?? 0).toFixed(1)} MB · 共享 {(detailGpu?.vram_shared_mb ?? 0).toFixed(1)} MB
            </dd>
            {#if Array.isArray(detailGpu?.per_adapter) && detailGpu.per_adapter.length}
              <dt>GPU 逐卡分解</dt>
              <dd class="gpu-per-adapter" title="该进程在各适配器上的占用（专用/共享）。上面「GPU 占用/GPU 显存」是跨卡汇总，这里是逐卡明细；只有该进程真正有实例的卡才会出现">
                {#each detailGpu.per_adapter as pa, i (pa?.luid ?? i)}
                  <div class="gpu-adapter-row">
                    <div class="gpu-adapter-head">
                      <span class="gpu-adapter-name" title={adapterLabel(pa?.luid)}>{adapterLabel(pa?.luid)}</span>
                      <span class="gpu-adapter-pct mono">{(pa?.gpu_pct ?? 0).toFixed(1)}%</span>
                    </div>
                    <div class="gpu-adapter-mem mono">
                      <span class="gpu-mem-part">专用 {mb1(pa?.vram_dedicated_mb)}</span>
                      <span class="gpu-mem-part">共享 {mb1(pa?.vram_shared_mb)}</span>
                    </div>
                  </div>
                {/each}
              </dd>
            {/if}
          {/if}
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
  .refresh-group { display: flex; border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: hidden; }
  .int-btn { border: none; background: transparent; color: var(--text-muted); font-family: inherit; font-size: 12px; padding: 6px 10px; cursor: pointer; border-left: 1px solid var(--border); }
  .int-btn:first-child { border-left: none; }
  .int-btn:hover { color: var(--text); background: var(--surface-2); }
  .int-btn.active { background: var(--accent); color: #fff; }
  .frozen-tag { color: var(--warn); }
  .chip-warn { font-size: 11px; padding: 3px 10px; border-radius: 999px; color: var(--warn); background: rgba(245, 158, 11, 0.12); border: 1px solid rgba(245, 158, 11, 0.45); white-space: nowrap; }

  .content-area { display: flex; gap: 0; }
  /* 表格列（顶部滑动条 + 滚动容器）整体作为一个 flex 子项 */
  .table-stack { flex: 1; min-width: 0; display: flex; flex-direction: column; }

  /* 顶部滑动条：位于滚动容器之外，自身不参与表格滚动；空闲态保留高度只做隐形，避免布局跳动 */
  .hscroll-top {
    flex: 0 0 auto;
    height: calc(var(--sbw, 15px) + 2px);
    overflow-x: auto;
    overflow-y: hidden;
    border: 1px solid var(--border);
    border-bottom: none;
    border-radius: var(--radius) var(--radius) 0 0;
    background: var(--surface);
  }
  .hscroll-top.idle { visibility: hidden; }
  .hscroll-spacer { height: 1px; }

  /* 单一滚动容器：表头 tbody 与所有行共享同一布局上下文 */
  .table-wrap {
    min-width: 0;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 0 0 var(--radius) var(--radius);
    overflow: auto;
    /* 表头 + 横向滚动条 + 表体一起装进这里。上限按"单一滚动容器"实测结构给：
       68 TopBar + 48 .view 上下内边距 + 77 工具栏（窄窗口换行后实测：35+12+14+16）
       + 17 顶部条（--sbw 15 + 2） = 210，再留 26 余量 → 236。
       窗口宽于约 1100px 时工具栏不换行（51px），此时余量 52px。
       取值偏大的代价只是少约 0.75 行（行高实测 40px），偏小则会让 .view 自己出现
       纵向滚动条（双层滚动）——所以宁可留余量。 */
    max-height: calc(100vh - 236px);
    scrollbar-gutter: stable;
  }
  .table-stack.with-detail .hscroll-top { border-radius: var(--radius) 0 0 0; border-right: none; }
  .table-stack.with-detail .table-wrap { border-radius: 0 0 0 var(--radius); border-right: none; }

  /* 暗色滚动条：只改颜色，不设 width/height —— 厚度必须保持平台默认值，
     否则 --sbw 探针值、gutter 宽与顶部条高度三者会互相错开。
     注意不要再设 scrollbar-width/scrollbar-color，那会让下面的 ::-webkit-* 整体失效。 */
  .table-wrap::-webkit-scrollbar,
  .hscroll-top::-webkit-scrollbar { background: transparent; }
  .table-wrap::-webkit-scrollbar-thumb,
  .hscroll-top::-webkit-scrollbar-thumb {
    background: var(--surface-2);
    border: 3px solid transparent;
    background-clip: content-box;
    border-radius: 999px;
  }
  .table-wrap::-webkit-scrollbar-thumb:hover,
  .hscroll-top::-webkit-scrollbar-thumb:hover { background-color: #475569; }
  .table-wrap::-webkit-scrollbar-corner,
  .hscroll-top::-webkit-scrollbar-corner { background: transparent; }

  /* border-collapse: separate：collapse 下 sticky 单元格的边框留在网格原位，
     会看到「分隔线不跟着表头走」；separate 让边框属于单元格自己。 */
  table { width: 100%; border-collapse: separate; border-spacing: 0; font-size: 13px; table-layout: fixed; }
  thead th {
    position: sticky; top: 0; z-index: 3;
    background: var(--surface-2);
    text-align: left;
    padding: 10px 10px 10px 8px;          /* 右侧 10px = 8px 手柄热区 + 2px 间隙 */
    font-weight: 500; color: var(--text-muted);
    cursor: default; user-select: none; white-space: nowrap;
    overflow: hidden;                     /* 列可拖窄，标签不得外溢 */
    border-bottom: 1px solid var(--border);
  }
  thead th.sortable { cursor: pointer; }
  thead th:hover { color: var(--text); }
  thead th:first-child { padding-left: 14px; }
  thead th:last-child { padding-right: 14px; }
  .th-label { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .col-resizer {
    position: absolute; top: 0; right: 0;
    width: 8px; height: 100%;
    cursor: col-resize;
    touch-action: none;
    z-index: 1;
  }
  .col-resizer::after {
    content: ""; position: absolute; top: 20%; right: 0;
    width: 1px; height: 60%; background: var(--border);
  }
  .col-resizer:hover::after,
  .col-resizer:focus-visible::after,
  .col-resizer.resizing::after { background: var(--accent); }
  .col-resizer:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }

  td { padding: 6px 8px; border-top: 1px solid var(--border); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  tbody tr:first-child td { border-top: none; }   /* 首行交给表头的 border-bottom，避免双线 */
  td:first-child { padding-left: 14px; }
  td:last-child { padding-right: 14px; }
  tbody tr:hover { background: var(--surface-2); }
  tbody tr.active-row { background: rgba(99, 102, 241, 0.12); }

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
  .gpu-eng { margin-left: 6px; font-size: 11px; }
  .gpu-engines { display: flex; flex-wrap: wrap; gap: 2px 10px; font-size: 11px; font-family: var(--font-mono); color: var(--text-muted); }

  /* 逐卡分解：360px 面板里每卡两行（卡名+% / 专用·共享），任何一段都不允许撑破面板 */
  .gpu-per-adapter { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
  .gpu-adapter-row { min-width: 0; }
  .gpu-adapter-head { display: flex; align-items: baseline; gap: 6px; min-width: 0; }
  .gpu-adapter-name { flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .gpu-adapter-pct { flex: 0 0 auto; font-size: 12px; }
  .gpu-adapter-mem { font-size: 11px; color: var(--text-muted); word-break: normal; }
  .gpu-mem-part { display: inline-block; white-space: nowrap; }
  .gpu-mem-part + .gpu-mem-part { margin-left: 8px; }

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
