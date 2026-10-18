<script>
  import { onMount, afterUpdate } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { metrics, cpuHistory, MAX_HISTORY, historyData, loadHistory, diskIo, gpuProc } from "../stores.js";

  $: cpu = $metrics?.cpu ?? 0;
  $: memLoad = $metrics?.mem_load ?? 0;
  $: memUsed = $metrics?.mem_used_gb ?? 0;
  $: memTotal = $metrics?.mem_total_gb ?? 0;
  $: memPageTotal = $metrics?.mem_page_total_gb ?? 0;
  $: memPageUsed = $metrics?.mem_page_used_gb ?? 0;
  $: diskRead = $metrics?.disk_read_bps ?? 0;
  $: diskWrite = $metrics?.disk_write_bps ?? 0;
  $: netTotal = $metrics?.net_total_bps ?? 0;
  // Aggregate per-process disk I/O for a more accurate total
  $: ioRows = $diskIo?.rows ?? [];
  $: totalDiskRead = ioRows.reduce((s, r) => s + (r.read_bps ?? 0), 0);
  $: totalDiskWrite = ioRows.reduce((s, r) => s + (r.write_bps ?? 0), 0);
  $: showDiskRead = totalDiskRead > 0 ? totalDiskRead : diskRead;
  $: showDiskWrite = totalDiskWrite > 0 ? totalDiskWrite : diskWrite;

  // ===== GPU 卡片（数据全部来自 $gpuProc store：不新增采集、不 invoke、不碰历史） =====
  // adapters 已由后端按 gpu_pct 降序，[0] 就是最忙的那块卡（跨适配器取 max，绝不求和）
  $: gpuAdapters = $gpuProc?.adapters ?? [];
  $: gpuTop = gpuAdapters[0] ?? null;
  $: gpuMax = gpuTop?.gpu_pct ?? 0;
  $: gpuOk = !!$gpuProc?.available;
  $: gpuAllRows = ($gpuProc?.rows) ?? [];

  // ── 卡片视图：总览（null）| 某张硬件卡的 luid | 其他软件/虚拟适配器组（OTHER_KEY） ──
  // 选中状态按 luid 保存、跨 tick 保持；该卡从 adapters 消失（拔卡/驱动重置）→ 回落总览且不复活。
  const OTHER_KEY = "__other__";
  let selKey = null; // null = 总览
  $: selAdapter = selKey && selKey !== OTHER_KEY ? gpuAdapters.find((a) => a.luid === selKey) ?? null : null;
  $: selValid = selKey === null || (selKey === OTHER_KEY ? gpuOther.length > 0 : !!selAdapter);
  // 渲染层用 viewKey 兜底：即使 afterUpdate 还没执行，界面也不会停在"指向不存在的卡"的状态
  $: viewKey = selValid ? selKey : null;
  $: viewAdapter = viewKey && viewKey !== OTHER_KEY ? gpuAdapters.find((a) => a.luid === viewKey) ?? null : null;
  $: viewIsOther = viewKey === OTHER_KEY;
  $: isOverview = viewKey === null;
  // 在 $: 里给 selKey 赋值会被编译器判为循环依赖（selAdapter 依赖 selKey，而该语句又读 selAdapter）→ 用 afterUpdate
  afterUpdate(() => {
    if (!selValid) selKey = null;
  });

  // Tab 条：总览 + 每个硬件卡一个 + 「其他 (N)」（N = 0 时不出现）
  $: gpuTabs = [
    { key: null, label: "总览", title: "所有适配器：取最忙的一块卡（不做跨卡求和）" },
    ...gpuHardware.map((a) => ({ key: a.luid, label: a._label, title: a._label })),
    ...(gpuOther.length
      ? [{ key: OTHER_KEY, label: `其他 (${gpuOther.length})`, title: "软件渲染 / DXGI 未列出的适配器" }]
      : []),
  ];
  $: activeTabId = "gpu-tab-" + (viewKey ?? "all");

  function selectTab(k) {
    selKey = k;
  }
  // 总览里点适配器行 = 切到它所属的 Tab（硬件卡 → 它自己；软件/虚拟 → 「其他」），不再就地筛选
  function goToAdapter(a) {
    selectTab(adapterKind(a) === "hardware" ? a.luid : OTHER_KEY);
  }
  // roving tabindex 下方向键必须能移动选中与焦点，否则其余 Tab 键盘不可达
  function onTabKey(e) {
    const i = gpuTabs.findIndex((t) => t.key === viewKey);
    if (i < 0) return;
    let n = null;
    if (e.key === "ArrowRight") n = (i + 1) % gpuTabs.length;
    else if (e.key === "ArrowLeft") n = (i - 1 + gpuTabs.length) % gpuTabs.length;
    else if (e.key === "Home") n = 0;
    else if (e.key === "End") n = gpuTabs.length - 1;
    if (n == null) return;
    e.preventDefault();
    const k = gpuTabs[n].key;
    selectTab(k);
    document.getElementById("gpu-tab-" + (k ?? "all"))?.focus();
  }

  // 当前视图的适配器 / 大数字 / 进程排行：总览 = 全局 TOP5；单卡（或其他组）= 该范围 TOP8
  $: viewAdapters = viewIsOther ? gpuOther : viewAdapter ? [viewAdapter] : [];
  $: viewPct = isOverview ? gpuMax : viewAdapters.reduce((m, a) => Math.max(m, a.gpu_pct ?? 0), 0);
  $: viewRows = isOverview
    ? gpuAllRows
    : viewIsOther
      ? gpuAllRows.filter((r) => gpuOther.some((a) => a.luid === r.luid))
      : gpuAllRows.filter((r) => r.luid === viewAdapter.luid);
  $: viewTopN = isOverview ? 5 : 8;
  $: viewRowsTop = [...viewRows].sort((a, b) => (b.gpu_pct ?? 0) - (a.gpu_pct ?? 0)).slice(0, viewTopN);
  // 三态必须分开：未就绪 / 计数器不可用 / 可用（可用但空闲是合法状态，不是错误）
  // 注意 idle 只看全局 adapters：若看被筛选后的列表，某张空闲卡的 Tab 会把整卡误判成空闲
  $: gpuState = !$gpuProc ? "loading" : !gpuOk ? "unavailable" : gpuAdapters.length === 0 ? "idle" : "ok";

  // ── 显存：只有「专用 > 0 且 容量 > 0」才给百分比与进度条 ──
  // 本机实测核显 Dedicated = 0 B（DXGI 容量仅 128 MiB carve-out）却确实在用共享内存，
  // 硬算百分比会显示 0%，读起来像"显存空着"——这是这张卡最需要避开的误读。
  const trimZeros = (s) => (s.includes(".") ? s.replace(/\.?0+$/, "") : s);
  const fmtMem = (bytes) => {
    const b = Number(bytes);
    if (!Number.isFinite(b)) return "—"; // 字段缺失（旧后端）≠ 0，不能显示成 0 B
    if (b <= 0) return "0 B";
    for (const [k, name] of [[1024 ** 3, "GB"], [1024 ** 2, "MB"], [1024, "KB"]]) {
      if (b >= k) { const v = b / k; return trimZeros(v >= 100 ? v.toFixed(0) : v.toFixed(2)) + " " + name; }
    }
    return b.toFixed(0) + " B";
  };
  const capText = (a) => (Number(a?.vram_capacity_bytes) > 0 ? fmtMem(a.vram_capacity_bytes) : "—");
  const vramPct = (a) => {
    const ded = Number(a?.vram_dedicated_bytes);
    const cap = Number(a?.vram_capacity_bytes);
    if (!(ded > 0) || !(cap > 0)) return null; // 返回 null = 不渲染百分比与进度条
    return Math.min(100, Math.max(0, Math.round((ded / cap) * 100)));
  };
  const vramTitle = (a) => {
    const pct = vramPct(a);
    if (pct != null) {
      return `专用显存 ${fmtMem(a.vram_dedicated_bytes)} / ${fmtMem(a.vram_capacity_bytes)}（${pct}%）· 共享内存 ${fmtMem(a.vram_shared_bytes)}`;
    }
    if (!(Number(a?.vram_capacity_bytes) > 0)) {
      return "该适配器未报告专用显存容量（DXGI 未匹配，或软件/虚拟适配器），此处不显示百分比以免误读" +
        (Number.isFinite(Number(a?.vram_shared_bytes)) ? `；共享内存 ${fmtMem(a.vram_shared_bytes)}` : "");
    }
    return "该适配器报告专用显存为 0（核显/软件适配器常见），此处不显示百分比以免误读；它当前的实际占用见「共享」";
  };

  // name 缺失时按设计 §3.2 兜底成 Adapter 0x<lo>（绝不显示空串）
  const adapterLabel = (a) => {
    const n = (a?.name ?? "").trim();
    if (n) return n;
    const parts = String(a?.luid ?? "").split(":");
    const lo = (parts[parts.length - 1] ?? "").trim();
    return lo ? `Adapter ${lo}` : "Adapter（未知 LUID）";
  };
  // kind 缺失按 hardware 处理：旧后端下不会把卡折叠没
  const adapterKind = (a) => (a?.kind === "software" || a?.kind === "virtual" ? a.kind : "hardware");

  // 每个适配器的主力引擎：rows 里同 luid 且 gpu_pct 最大的那一行的 engtype（可能是空串）
  $: engineByLuid = (() => {
    const m = new Map();
    for (const r of ($gpuProc?.rows) ?? []) {
      const cur = m.get(r.luid);
      if (!cur || (r.gpu_pct ?? 0) > (cur.gpu_pct ?? 0)) m.set(r.luid, r);
    }
    return m;
  })();
  $: gpuTopLabel = gpuTop ? adapterLabel(gpuTop) : "—";
  $: gpuTopEngine = gpuTop ? engineByLuid.get(gpuTop.luid)?.engtype || "—" : "—";

  // 呈现排序（§3.7）：hardware 在前 → 组内 gpu_pct 降序 → 同值按名称字典序（渲染稳定，不随 tick 抖动）
  $: gpuSorted = gpuAdapters
    .map((a) => ({
      ...a,
      _label: adapterLabel(a),
      _kind: adapterKind(a),
      _engine: engineByLuid.get(a.luid)?.engtype || "—",
    }))
    .sort((a, b) => {
      const ka = a._kind === "hardware" ? 0 : 1;
      const kb = b._kind === "hardware" ? 0 : 1;
      if (ka !== kb) return ka - kb;
      const d = (b.gpu_pct ?? 0) - (a.gpu_pct ?? 0);
      return d !== 0 ? d : a._label.localeCompare(b._label);
    });
  $: gpuHardware = gpuSorted.filter((a) => a._kind === "hardware");
  $: gpuOther = gpuSorted.filter((a) => a._kind !== "hardware");

  // 引擎分解文本（冻结契约：adapters[].engines = [{ engtype, pct }]）；0 的条目过滤掉，全 0 显示「空闲」
  const engText = (a) => {
    const list = (a?.engines ?? [])
      .filter((e) => Number(e?.pct) > 0)
      .map((e) => `${e?.engtype || "?"} ${Number(e.pct).toFixed(1)}%`);
    return list.length ? list.join(" · ") : "空闲";
  };
  $: viewEngText = viewAdapter ? engText(viewAdapter) : "";
  $: viewLabel = viewAdapter ? adapterLabel(viewAdapter) : viewIsOther ? `其他适配器（${gpuOther.length}）` : "";

  const sev = (v) => (v >= 85 ? "danger" : v >= 70 ? "warn" : "ok");
  const sevLabel = (v) => (v >= 85 ? "高负载" : v >= 70 ? "偏高" : "正常");

  // CPU sparkline (existing)
  $: spark = (() => {
    const h = $cpuHistory;
    if (h.length < 2) return "";
    const w = 320, ht = 70;
    const step = w / (MAX_HISTORY - 1);
    return h
      .map((v, i) => {
        const x = i * step;
        const y = ht - (Math.min(100, v) / 100) * ht;
        return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(" ");
  })();
  $: sparkArea = spark ? `${spark} L320,70 L0,70 Z` : "";

  // ===== History chart (interactive) =====
  const HW = 800, HH = 160; // viewBox 尺寸（等比，不再拉伸变形）

  // 可切换的指标系列
  const series = [
    { key: "cpu", label: "CPU", color: "#6366f1", unit: "%", max: () => 100,
      get: (d) => d.cpu ?? 0, fmt: (v) => v.toFixed(1) + "%" },
    { key: "mem", label: "内存", color: "#22c55e", unit: "%", max: () => 100,
      get: (d) => d.mem_load ?? 0, fmt: (v) => v.toFixed(0) + "%" },
    { key: "net", label: "网络", color: "#f59e0b", unit: "", max: (h) => Math.max(1, ...h.map((d) => (d.net_total_bps ?? 0) * 8)),
      get: (d) => (d.net_total_bps ?? 0) * 8, fmt: (v) => fmtBitsRaw(v) },
    // GPU：历史点里 adapters 恒为空数组（定长记录只存 available/gpu_pct/胜出 luid），
    // 所以这里只能用 gpu.gpu_pct；available:false 时返回 null → 路径构造器断线，绝不画成 0。
    // 固定按 0–100 归一化：若按数据 max 自适应，一张 5% 的卡会被画成满高。
    { key: "gpu", label: "GPU（每 2 秒采样）", color: "#38bdf8", unit: "%", max: () => 100,
      hint: "最忙引擎口径（跨适配器取 max，不做跨卡求和）；每 2 秒采样、1 秒记录点会重复成阶梯，不做插值",
      get: (d) => (d.gpu?.available ? (d.gpu.gpu_pct ?? 0) : null),
      fmt: (v) => (v == null ? "—" : v.toFixed(1) + "%") },
  ];
  let activeSeries = { cpu: true, mem: true, net: false, gpu: true };

  function toggleSeries(k) {
    activeSeries = { ...activeSeries, [k]: !activeSeries[k] };
  }

  // 每个系列的路径（按各自 max 归一化到 0..HH）。
  // null / 非有限值 = 该点不可用 → 断开折线（绝不画成 0）；面积按连续段分别闭合，不跨缺口连成一片。
  $: seriesPaths = (() => {
    const h = $historyData;
    if (h.length < 2) return [];
    const step = HW / (h.length - 1);
    const toLine = (seg) => seg.map((p, j) => `${j ? "L" : "M"}${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(" ");
    return series
      .filter((s) => activeSeries[s.key])
      .map((s) => {
        const raw = s.max(h);
        const mx = Number.isFinite(raw) && raw > 0 ? raw : 1; // 防 0/0 = NaN 把整条路径写成 NaN
        const segs = [];
        let cur = null;
        h.forEach((d, i) => {
          const v = s.get(d);
          if (v == null || !Number.isFinite(v)) { cur = null; return; } // 断线
          if (!cur) { cur = []; segs.push(cur); }
          cur.push({ x: i * step, y: HH - Math.min(1, Math.max(0, v / mx)) * HH });
        });
        const line = segs.map(toLine).join(" ");
        const area = segs
          .filter((seg) => seg.length > 1)
          .map((seg) => `${toLine(seg)} L${seg[seg.length - 1].x.toFixed(1)},${HH} L${seg[0].x.toFixed(1)},${HH} Z`)
          .join(" ");
        return { ...s, line, area, points: segs.reduce((a, seg) => a + seg.length, 0) };
      });
  })();

  // ——— 时间格式化：一律基于规范时间戳 ts_ms，不在 ts 字符串上做拼接推断 ———
  const pad2 = (n) => String(n).padStart(2, "0");
  const sampleMs = (d) => (Number.isFinite(d?.ts_ms) ? d.ts_ms : d?.ts ? Date.parse(d.ts) : NaN);
  // withDate: 长窗口（> 1 小时）需要日期，否则「昨天 14:00」与「今天 14:00」无法区分
  const fmtClock = (ms, withDate) => {
    if (!Number.isFinite(ms)) return "";
    const d = new Date(ms);
    if (isNaN(d.getTime())) return "";
    const t = `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
    return withDate ? `${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${t.slice(0, 5)}` : t;
  };
  // 覆盖时长的人话表述（分钟 / 小时 / 天）；去掉无意义的 ".0"（3 天，而不是 3.0 天）
  const fmtSpan = (ms) => {
    const m = Math.max(0, ms) / 60000;
    const trim1 = (v) => (Math.round(v * 10) / 10).toFixed(1).replace(/\.0$/, "");
    if (m < 60) return Math.max(1, Math.round(m)) + " 分钟";
    if (m < 1440) return trim1(m / 60) + " 小时";
    return trim1(m / 1440) + " 天";
  };

  // ——— hover 游标 ———
  let hoverIdx = null;
  let hoverX = 0;
  let chartEl;

  function onChartMove(e) {
    const h = $historyData;
    if (h.length < 2) return;
    const rect = chartEl.getBoundingClientRect();
    const rel = (e.clientX - rect.left) / rect.width; // 0..1
    const idx = Math.round(rel * (h.length - 1));
    hoverIdx = Math.max(0, Math.min(h.length - 1, idx));
    hoverX = (hoverIdx / (h.length - 1)) * 100; // 百分比，用于定位竖线/tooltip
  }
  function onChartLeave() {
    hoverIdx = null;
  }

  $: hoverSample = hoverIdx != null ? $historyData[hoverIdx] : null;
  $: hoverTime = hoverSample ? fmtClock(sampleMs(hoverSample), longWindow) : "";

  // 原始 bit/s 数值格式化（输入已是 bit/s）
  const fmtBitsRaw = (bps) => {
    if (bps < 1000) return bps.toFixed(0) + " bps";
    if (bps < 1e6) return (bps / 1e3).toFixed(1) + " Kbps";
    if (bps < 1e9) return (bps / 1e6).toFixed(1) + " Mbps";
    return (bps / 1e9).toFixed(1) + " Gbps";
  };

  // Byte/s formatting
  const fmtBytes = (v) => {
    if (v < 1024) return v.toFixed(1) + " B";
    if (v < 1024 * 1024) return (v / 1024).toFixed(1) + " KB";
    if (v < 1024 * 1024 * 1024) return (v / (1024 * 1024)).toFixed(1) + " MB";
    return (v / (1024 * 1024 * 1024)).toFixed(1) + " GB";
  };

  // Bits/s formatting (input in bytes/s, multiply by 8)
  const fmtBits = (v) => {
    const bps = v * 8;
    if (bps < 1000) return bps.toFixed(1) + " bps";
    if (bps < 1e6) return (bps / 1e3).toFixed(1) + " Kbps";
    if (bps < 1e9) return (bps / 1e6).toFixed(1) + " Mbps";
    return (bps / 1e9).toFixed(1) + " Gbps";
  };

  // Page file utilization
  $: pageFilePct = memPageTotal > 0 ? (memPageUsed / memPageTotal * 100) : 0;

  // Time ranges：≤ 1 小时走内存热路径（get_history），> 1 小时读磁盘长历史（get_history_span）
  const SPAN_MAX_POINTS = 800; // 与图表 HW = 800 同量级：30 天也不会把 86400 个点灌进 DOM
  const ranges = [
    { label: "1分", n: 60 },
    { label: "5分", n: 300 },
    { label: "15分", n: 900 },
    { label: "1小时", n: 3600 },
    { label: "6小时", n: 21600 },
    { label: "24小时", n: 86400 },
    { label: "7天", n: 604800 },
    { label: "30天", n: 2592000 },
  ];
  let activeRange = 60;
  let rangeLoading = false;
  let spanError = "";
  let histStatus = null;

  $: longWindow = activeRange > 3600;
  $: xFirstMs = $historyData.length ? sampleMs($historyData[0]) : NaN;
  $: xLastMs = $historyData.length ? sampleMs($historyData[$historyData.length - 1]) : NaN;

  async function loadRange(n) {
    rangeLoading = true;
    spanError = "";
    try {
      if (n <= 3600) {
        await loadHistory(n); // 既有内存热路径（后端 n ≤ 3600）
      } else {
        const arr = await invoke("get_history_span", { seconds: n, maxPoints: SPAN_MAX_POINTS });
        historyData.set(Array.isArray(arr) ? arr : []);
      }
    } catch (e) {
      historyData.set([]);
      spanError = String(e);
    } finally {
      rangeLoading = false;
    }
    loadStatus();
  }

  async function loadStatus() {
    try {
      histStatus = await invoke("get_history_status");
    } catch (e) {
      histStatus = { enabled: false, writable: false, reason: "读取记录状态失败：" + e };
    }
  }

  // 数据源状态行（enabled/writable/bytes/oldest_ms/newest_ms/dir 全部来自 get_history_status）
  $: statusLine = (() => {
    if (spanError) return "读取磁盘记录失败：" + spanError;
    if (!histStatus) return rangeLoading ? "正在读取磁盘记录状态…" : "";
    if (!histStatus.enabled || !histStatus.writable) return "未记录到磁盘（" + (histStatus.reason || "原因未知") + "）";
    const bytes = histStatus.bytes ?? 0;
    if (!bytes || !(histStatus.oldest_ms > 0)) return "磁盘记录为空（尚无数据）";
    // 落盘周期来自后端的 HistoryStatus.flush_secs（不要硬编码，否则后端改了这里会静默说谎）
    const flush = histStatus.flush_secs > 0 ? ` · 每 ${histStatus.flush_secs} 秒落盘` : "";
    return `磁盘记录 ${fmtBytes(bytes)} · 覆盖最近 ${fmtSpan(Date.now() - histStatus.oldest_ms)}${flush}`;
  })();

  $: statusTitle = histStatus
    ? "目录：" + (histStatus.dir || "—") +
      (histStatus.oldest_ms > 0
        ? " · 记录范围：" + fmtClock(histStatus.oldest_ms, true) + " → " + fmtClock(histStatus.newest_ms, true)
        : "")
    : "";

  function selectRange(n) {
    activeRange = n;
    hoverIdx = null;
    loadRange(n);
  }

  onMount(() => {
    loadRange(activeRange);
  });
</script>

<div class="grid">
  <!-- CPU 使用率 -->
  <article class="card">
    <div class="card-head">
      <span class="card-label">CPU 使用率</span>
      <span class="badge {sev(cpu)}">{sevLabel(cpu)}</span>
    </div>
    <div class="value">{cpu.toFixed(1)}<span class="unit">%</span></div>
    <svg class="spark" viewBox="0 0 320 70" preserveAspectRatio="none">
      {#if sparkArea}
        <path d={sparkArea} class="spark-area" />
        <path d={spark} class="spark-line" />
      {/if}
    </svg>
    <div class="card-foot">最近 {$cpuHistory.length}/{MAX_HISTORY} 个采样 · PDH _Total</div>
  </article>

  <!-- 内存占用 -->
  <article class="card">
    <div class="card-head">
      <span class="card-label">内存占用</span>
      <span class="badge {sev(memLoad)}">{memLoad}%</span>
    </div>
    <div class="value">{memUsed.toFixed(1)}<span class="unit">/ {memTotal.toFixed(1)} GB</span></div>
    <div class="bar">
      <div class="bar-fill {sev(memLoad)}" style="width:{memLoad}%"></div>
    </div>
    <div class="mem-page-row">
      <div>
        <span class="io-label">页面文件</span>
        <span class="io-value mono">{memPageUsed.toFixed(1)}<span class="unit-sm">/ {memPageTotal.toFixed(1)} GB</span></span>
      </div>
    </div>
    <div class="bar bar-sm">
      <div class="bar-fill {sev(pageFilePct)}" style="width:{pageFilePct}%"></div>
    </div>
    <div class="card-foot">GlobalMemoryStatusEx</div>
  </article>

  <!-- 磁盘 I/O -->
  <article class="card">
    <div class="card-head">
      <span class="card-label">磁盘 I/O</span>
    </div>
    <div class="io-row">
      <div>
        <span class="io-label">读取</span>
        <span class="io-value mono">{fmtBytes(showDiskRead)}/s</span>
      </div>
      <div>
        <span class="io-label">写入</span>
        <span class="io-value mono">{fmtBytes(showDiskWrite)}/s</span>
      </div>
    </div>
    <div class="card-foot">PDH PhysicalDisk(_Total)</div>
  </article>

  <!-- 网络吞吐 -->
  <article class="card">
    <div class="card-head">
      <span class="card-label">网络吞吐</span>
    </div>
    <div class="value-sm mono">{fmtBits(netTotal)}/s</div>
    <div class="bar">
      <div class="bar-fill ok" style="width:{Math.min(100, netTotal / 1e8 * 100)}%"></div>
    </div>
    <div class="card-foot">GetIfTable</div>
  </article>

  <!-- 每进程磁盘 I/O TOP 5 -->
  <article class="card">
    <div class="card-head">
      <span class="card-label">进程磁盘 I/O TOP 5</span>
    </div>
    {#if $diskIo?.rows?.length > 0}
      <div class="io-proc-list">
        {#each $diskIo.rows.slice(0, 5) as row}
          <div class="io-proc-row">
            <span class="io-proc-name" title="{row.name} ({row.pid})">{row.name}</span>
            <span class="io-proc-r">
              <span class="io-label-r">R</span> {fmtBytes(row.read_bps)}/s
            </span>
            <span class="io-proc-w">
              <span class="io-label-w">W</span> {fmtBytes(row.write_bps)}/s
            </span>
          </div>
        {/each}
      </div>
    {:else}
      <div class="io-empty">等待数据…</div>
    {/if}
    <div class="card-foot">GetProcessIoCounters · 每 2 秒</div>
  </article>

  <!-- 系统负载概览 -->
  <article class="card">
    <div class="card-head">
      <span class="card-label">系统负载</span>
    </div>
    <div class="io-row">
      <div>
        <span class="io-label">CPU</span>
        <span class="io-value mono">{cpu.toFixed(1)}%</span>
      </div>
      <div>
        <span class="io-label">内存</span>
        <span class="io-value mono">{memUsed.toFixed(1)} GB</span>
      </div>
    </div>
    <div class="io-row">
      <div>
        <span class="io-label">页面文件</span>
        <span class="io-value mono">{memPageUsed.toFixed(1)} GB</span>
      </div>
      <div>
        <span class="io-label">负载状态</span>
        <span class="badge {sev(Math.max(cpu, memLoad))}">{sevLabel(Math.max(cpu, memLoad))}</span>
      </div>
    </div>
    <div class="card-foot">综合指标</div>
  </article>

  <!-- GPU 占用（全宽：网格 2~4 列时，第 7 张卡只占 1 列会成孤卡） -->
  <article class="card card-wide">
    <div class="card-head">
      <span
        class="card-label"
        title="取最忙的一块 GPU 的最忙引擎（与任务管理器性能页口径一致）；各卡独立计量、不做跨卡求和"
      >GPU 占用</span>
      {#if gpuState === "ok"}
        <span class="badge {sev(viewPct)}">{sevLabel(viewPct)}</span>
      {:else if gpuState === "idle"}
        <span class="badge ok">空闲</span>
      {:else if gpuState === "unavailable"}
        <span class="badge danger">不可用</span>
      {/if}
    </div>

    {#if gpuState === "loading"}
      <div class="io-empty">等待数据…</div>
    {:else if gpuState === "unavailable"}
      <div class="io-empty" title={$gpuProc?.reason || ""}>GPU 计数器不可用{$gpuProc?.reason ? " · " + $gpuProc.reason : ""}</div>
    {:else}
      <!-- 每个硬件卡一个 Tab；software/virtual 合并成「其他 (N)」；没有卡时不显示空 Tab 条 -->
      {#if gpuTabs.length > 1}
        <div class="gpu-tabs" role="tablist" aria-label="GPU 视图">
          {#each gpuTabs as t (t.key ?? "__all__")}
            <button
              class="gpu-tab"
              class:active={viewKey === t.key}
              role="tab"
              id={"gpu-tab-" + (t.key ?? "all")}
              aria-selected={viewKey === t.key}
              aria-controls="gpu-tabpanel"
              tabindex={viewKey === t.key ? 0 : -1}
              title={t.title}
              on:click={() => selectTab(t.key)}
              on:keydown={onTabKey}
            >{t.label}</button>
          {/each}
        </div>
      {/if}

      <div class="gpu-cols" id="gpu-tabpanel" role="tabpanel" aria-labelledby={activeTabId} tabindex="0">
        <!-- 左栏 -->
        <div class="gpu-left">
          <div class="value">{viewPct.toFixed(1)}<span class="unit">%</span></div>

          {#if isOverview}
            {#if gpuState === "idle"}
              <div class="gpu-sub">空闲（无进程在用 GPU）</div>
            {:else}
              <div class="gpu-sub">最忙：{gpuTopLabel} · {gpuTopEngine}</div>
            {/if}
          {:else if viewAdapter}
            <div class="gpu-sub gpu-sub-wrap" title="该适配器各引擎的占用；同一份工作可能被多个引擎计入">
              引擎分解：<span class="gpu-eng-val">{viewEngText}</span>
            </div>
            <!-- 该卡显存块：沿用「专用 > 0 且 容量 > 0 才给百分比与进度条」的规则 -->
            <div class="gpu-adapter-mem gpu-card-mem" title={vramTitle(viewAdapter)}>
              <span class="vram-ded">专用 {fmtMem(viewAdapter.vram_dedicated_bytes)} / {capText(viewAdapter)}</span>
              {#if vramPct(viewAdapter) != null}
                <span class="vram-bar"><span class="vram-fill" style="width:{vramPct(viewAdapter)}%"></span></span>
                <span class="vram-pct">{vramPct(viewAdapter)}%</span>
              {/if}
              <span class="vram-shared">共享 {fmtMem(viewAdapter.vram_shared_bytes)}</span>
            </div>
          {:else}
            <div class="gpu-sub">其他适配器（{gpuOther.length}）：软件渲染或 DXGI 未列出</div>
          {/if}

          {#if isOverview || viewIsOther}
            <div class="gpu-adapters">
              {#each isOverview ? gpuHardware : gpuOther as a (a.luid)}
                <svelte:element
                  this={isOverview ? "button" : "div"}
                  role={isOverview ? "button" : undefined}
                  class="gpu-adapter-row"
                  title={vramTitle(a) + (isOverview ? "；点击切到该卡的 Tab" : "")}
                  on:click={isOverview ? () => goToAdapter(a) : undefined}
                >
                  <span class="gpu-adapter-name" title={a._label}>{a._label}</span>
                  <span class="gpu-adapter-eng">{a._engine}</span>
                  <span class="gpu-adapter-pct">{(a.gpu_pct ?? 0).toFixed(1)}%</span>
                  <span class="gpu-adapter-mem">
                    <span class="vram-ded">专用 {fmtMem(a.vram_dedicated_bytes)} / {capText(a)}</span>
                    {#if vramPct(a) != null}
                      <span class="vram-bar"><span class="vram-fill" style="width:{vramPct(a)}%"></span></span>
                      <span class="vram-pct">{vramPct(a)}%</span>
                    {/if}
                    <span class="vram-shared">共享 {fmtMem(a.vram_shared_bytes)}</span>
                  </span>
                </svelte:element>
              {/each}
              {#if isOverview && gpuOther.length > 0}
                <details class="gpu-more">
                  <summary title="软件渲染或 DXGI 未列出的适配器，共 {gpuOther.length} 个；按类别折叠，与占用高低无关，标题数字始终取最忙的一块卡，展开后点行可切到「其他」Tab"
                  >其他适配器 ({gpuOther.length})</summary>
                  {#each gpuOther as a (a.luid)}
                    <button
                      class="gpu-adapter-row"
                      title={vramTitle(a) + "；点击切到「其他」Tab"}
                      on:click={() => goToAdapter(a)}
                    >
                      <span class="gpu-adapter-name" title={a._label}>{a._label}</span>
                      <span class="gpu-adapter-eng">{a._engine}</span>
                      <span class="gpu-adapter-pct">{(a.gpu_pct ?? 0).toFixed(1)}%</span>
                      <span class="gpu-adapter-mem">
                        <span class="vram-ded">专用 {fmtMem(a.vram_dedicated_bytes)} / {capText(a)}</span>
                        {#if vramPct(a) != null}
                          <span class="vram-bar"><span class="vram-fill" style="width:{vramPct(a)}%"></span></span>
                          <span class="vram-pct">{vramPct(a)}%</span>
                        {/if}
                        <span class="vram-shared">共享 {fmtMem(a.vram_shared_bytes)}</span>
                      </span>
                    </button>
                  {/each}
                </details>
              {/if}
            </div>
          {/if}
        </div>

        <!-- 右栏：进程排行（总览 = 全局 TOP5；单卡/其他 = 该范围 TOP8） -->
        <div class="gpu-right">
          <div class="gpu-right-head">
            <span class="gpu-top-title" title={isOverview ? "全部适配器上的进程占用排行" : "只看当前 Tab 范围内的适配器（按 luid 过滤）"}>
              进程 GPU TOP {viewTopN}
            </span>
            {#if !isOverview}
              <button class="gpu-scope-all" on:click={() => selectTab(null)} title="回到总览">总览</button>
            {/if}
          </div>
          {#if viewRowsTop.length > 0}
            <div class="io-proc-list">
              {#each viewRowsTop as row (row.pid)}
                <div class="io-proc-row">
                  <span class="io-proc-name" title="{row.name} ({row.pid})">{row.name}</span>
                  <span class="gpu-proc-pct">{(row.gpu_pct ?? 0).toFixed(1)}%</span>
                </div>
              {/each}
            </div>
          {:else}
            <div class="io-empty">{isOverview ? "空闲（无进程在用 GPU）" : "该 GPU 当前无进程在用"}</div>
          {/if}
        </div>
      </div>
    {/if}
    <div class="card-foot">GPU Engine 计数器 · 每 2 秒 · 各卡独立计量，不跨卡求和</div>
  </article>
</div>

<!-- 历史趋势 -->
{#if $historyData.length > 0 || histStatus}
<section class="history-section">
  <div class="history-head">
    <h3 class="section-title">历史趋势</h3>
    <div class="history-controls">
      <div class="series-group">
        {#each series as s}
          <button
            class="series-btn"
            class:active={activeSeries[s.key]}
            style="--sc:{s.color}"
            title={s.hint}
            on:click={() => toggleSeries(s.key)}
          >
            <span class="series-dot"></span>{s.label}
          </button>
        {/each}
      </div>
      <div class="range-group">
        {#each ranges as r}
          <button
            class="range-btn"
            class:active={activeRange === r.n}
            on:click={() => selectRange(r.n)}
          >{r.label}</button>
        {/each}
      </div>
    </div>
  </div>
  {#if $historyData.length > 1}
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <div
    class="history-chart"
    bind:this={chartEl}
    on:mousemove={onChartMove}
    on:mouseleave={onChartLeave}
  >
    <svg viewBox="0 0 {HW} {HH}" preserveAspectRatio="none" class="history-svg">
      <!-- 水平网格线 25/50/75% -->
      {#each [0.25, 0.5, 0.75] as g}
        <line class="grid-line" x1="0" y1={HH * g} x2={HW} y2={HH * g} />
      {/each}
      {#each seriesPaths as sp}
        <path d={sp.area} fill={sp.color} fill-opacity="0.1" stroke="none" />
        <path d={sp.line} fill="none" stroke={sp.color} stroke-width="2" vector-effect="non-scaling-stroke" />
      {/each}
    </svg>

    {#if hoverIdx != null}
      <div class="cursor-line" style="left:{hoverX}%"></div>
    {/if}

    <div class="chart-labels">
      <span>0%</span>
      <span>50%</span>
      <span>100%</span>
    </div>

    {#if hoverSample}
      <div
        class="hover-tip"
        class:flip={hoverX > 60}
        style="left:{hoverX}%"
      >
        <div class="tip-time mono">{hoverTime}</div>
        {#each series as s}
          {#if activeSeries[s.key]}
            <div class="tip-row">
              <span class="tip-dot" style="background:{s.color}"></span>
              <span class="tip-label">{s.label}</span>
              <span class="tip-val mono">{s.fmt(s.get(hoverSample))}</span>
            </div>
          {/if}
        {/each}
      </div>
    {/if}
  </div>

  <!-- X 轴：长窗口显示日期（用 ts_ms 格式化），中间给出实际点数（说明抽稀后的规模） -->
  <div class="x-labels">
    <span>{fmtClock(xFirstMs, longWindow)}</span>
    <span>{$historyData.length} 点</span>
    <span>{fmtClock(xLastMs, longWindow)}</span>
  </div>
  {:else}
    <div class="io-empty">该时间窗暂无足够数据{rangeLoading ? "（读取中…）" : ""}</div>
  {/if}

  {#if statusLine}
    <div class="hist-status" title={statusTitle}>{statusLine}</div>
  {/if}
</section>
{/if}

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
    gap: var(--sp-4);
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: var(--sp-6);
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
  .card-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .card-label {
    font-size: 13px;
    color: var(--text-muted);
  }
  .value {
    font-family: var(--font-mono);
    font-size: 40px;
    font-weight: 600;
    line-height: 1;
    font-variant-numeric: tabular-nums;
  }
  .value-sm {
    font-family: var(--font-mono);
    font-size: 28px;
    font-weight: 600;
    line-height: 1;
    font-variant-numeric: tabular-nums;
  }
  .unit {
    font-size: 15px;
    color: var(--text-muted);
    margin-left: var(--sp-2);
    font-weight: 400;
  }
  .unit-sm {
    font-size: 12px;
    color: var(--text-muted);
    margin-left: var(--sp-1);
    font-weight: 400;
  }
  .badge {
    font-size: 11px;
    padding: 3px 10px;
    border-radius: 999px;
    border: 1px solid transparent;
    font-variant-numeric: tabular-nums;
  }
  .badge.ok {
    color: var(--ok);
    border-color: rgba(34, 197, 94, 0.4);
    background: rgba(34, 197, 94, 0.12);
  }
  .badge.warn {
    color: var(--warn);
    border-color: rgba(245, 158, 11, 0.4);
    background: rgba(245, 158, 11, 0.12);
  }
  .badge.danger {
    color: var(--danger);
    border-color: rgba(239, 68, 68, 0.4);
    background: rgba(239, 68, 68, 0.12);
  }
  .spark {
    width: 100%;
    height: 70px;
    display: block;
  }
  .spark-line {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2;
    vector-effect: non-scaling-stroke;
  }
  .spark-area {
    fill: rgba(99, 102, 241, 0.15);
    stroke: none;
  }
  .bar {
    height: 10px;
    background: var(--surface-2);
    border-radius: 999px;
    overflow: hidden;
  }
  .bar-sm {
    height: 6px;
  }
  .bar-fill {
    height: 100%;
    border-radius: 999px;
    transition: width 0.4s ease;
  }
  .bar-fill.ok {
    background: linear-gradient(90deg, var(--accent), var(--ok));
  }
  .bar-fill.warn {
    background: linear-gradient(90deg, var(--warn), #f97316);
  }
  .bar-fill.danger {
    background: linear-gradient(90deg, var(--danger), #dc2626);
  }
  .io-row {
    display: flex;
    gap: var(--sp-6);
    align-items: baseline;
  }
  .io-label {
    font-size: 11px;
    color: var(--text-muted);
    display: block;
    margin-bottom: 2px;
  }
  .io-value {
    font-variant-numeric: tabular-nums;
  }
  .mono {
    font-family: var(--font-mono);
  }
  .card-foot {
    font-size: 11px;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }
  .io-proc-list {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .io-proc-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: 12px;
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }
  .io-proc-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
  }
  .io-proc-r {
    color: var(--ok);
    min-width: 90px;
    text-align: right;
  }
  .io-proc-w {
    color: var(--warn);
    min-width: 90px;
    text-align: right;
  }
  .io-label-r {
    font-size: 10px;
    color: var(--ok);
    opacity: 0.7;
  }
  .io-label-w {
    font-size: 10px;
    color: var(--warn);
    opacity: 0.7;
  }
  .io-empty {
    font-size: 13px;
    color: var(--text-muted);
    padding: var(--sp-3) 0;
  }
  .mem-page-row {
    display: flex;
    gap: var(--sp-4);
  }

  /* ===== GPU 卡片 ===== */
  /* 全宽：网格列数随窗口变（2~4 列），1 列宽的第 7 张卡必然成为孤卡 */
  .card-wide {
    grid-column: 1 / -1;
  }
  /* 每个硬件卡一个 Tab；卡多时换行，不撑破卡片 */
  .gpu-tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding-bottom: 6px;
    border-bottom: 1px solid var(--border);
    margin-bottom: var(--sp-2);
  }
  .gpu-tab {
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 3px 10px;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--text-muted);
    font-family: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .gpu-tab:hover {
    color: var(--text);
    background: var(--surface-2);
  }
  .gpu-tab.active {
    color: var(--text);
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 16%, transparent);
  }
  .gpu-tab:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  /* 两栏。auto-fit + minmax：容器够宽就是两等分栏，不够宽自动堆叠成一栏
     （只有 2 个子项，多余轨道会被 auto-fit 折叠，所以不会出现第 3 栏） */
  .gpu-cols {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
    gap: var(--sp-3) var(--sp-6);
    align-items: start;
  }
  .gpu-left,
  .gpu-right {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    min-width: 0;
  }
  .gpu-sub {
    font-size: 12px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* 单卡 Tab 的引擎分解可能很长（最多 8 项），允许换行，不要 ellipsis 掉 */
  .gpu-sub-wrap {
    white-space: normal;
    overflow: visible;
    text-overflow: clip;
    line-height: 1.5;
  }
  .gpu-eng-val {
    color: var(--text);
    font-family: var(--font-mono);
  }
  /* 单卡 Tab 的显存块（与适配器行内的显存行同一套配色/规则） */
  .gpu-card-mem {
    margin-top: var(--sp-1);
    padding: 3px 6px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .gpu-adapters {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: var(--sp-1);
  }
  /* 适配器行 = 可点选按钮（两行网格：首行 名称/引擎/占用，次行显存） */
  .gpu-adapter-row {
    display: grid;
    grid-template-columns: 1fr auto auto;
    align-items: center;
    gap: 2px var(--sp-2);
    width: 100%;
    margin: 0;
    padding: 3px 6px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    text-align: left;
    cursor: pointer;
  }
  .gpu-adapter-row:hover {
    background: var(--surface-2);
  }
  .gpu-adapter-row.sel {
    background: color-mix(in srgb, var(--accent) 16%, transparent);
    border-color: var(--accent);
  }
  .gpu-adapter-row:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .gpu-adapter-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
  }
  .gpu-adapter-eng {
    font-size: 10px;
    color: var(--text-muted);
    min-width: 68px;
    text-align: right;
  }
  .gpu-adapter-pct {
    min-width: 56px;
    text-align: right;
    color: var(--text);
  }
  /* 显存行：窄栏时自动折行，不会撑破卡片 */
  .gpu-adapter-mem {
    grid-column: 1 / -1;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 2px var(--sp-2);
    font-size: 11px;
    color: var(--text-muted);
  }
  .vram-bar {
    width: 64px;
    height: 5px;
    border-radius: 999px;
    background: var(--surface-2);
    overflow: hidden;
  }
  .vram-fill {
    display: block;
    height: 100%;
    border-radius: 999px;
    background: linear-gradient(90deg, var(--accent), var(--ok));
  }
  .vram-pct {
    min-width: 30px;
    text-align: right;
  }
  .gpu-eng-break {
    display: flex;
    flex-wrap: wrap;
    gap: 2px var(--sp-2);
    margin-top: var(--sp-1);
    padding: 3px 6px;
    font-size: 11px;
    font-family: var(--font-mono);
    color: var(--text-muted);
    border-top: 1px dashed var(--border);
  }
  .gpu-eng-break-label {
    opacity: 0.8;
  }
  .gpu-eng-break-val {
    color: var(--text);
  }
  .gpu-more > summary {
    font-size: 12px;
    color: var(--text-muted);
    cursor: pointer;
  }
  .gpu-more > summary:hover,
  .gpu-more[open] > summary {
    color: var(--text);
  }
  .gpu-more > .gpu-adapter-row {
    margin-top: 4px;
    width: calc(100% - var(--sp-4));
    margin-left: var(--sp-4);
  }
  .gpu-right-head {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    min-width: 0;
  }
  .gpu-top-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--text-muted);
  }
  .gpu-scope-all {
    flex: 0 0 auto;
    padding: 1px 8px;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--text-muted);
    font-family: inherit;
    font-size: 11px;
    cursor: pointer;
  }
  .gpu-scope-all:hover {
    color: var(--text);
    border-color: var(--accent);
  }
  .gpu-scope-all:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .gpu-proc-pct {
    min-width: 56px;
    text-align: right;
    color: var(--accent);
  }

  /* History section */
  .history-section {
    margin-top: var(--sp-6);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: var(--sp-6);
  }
  .history-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--sp-3);
    flex-wrap: wrap; /* 8 个时间窗 + 4 个系列按钮：窄窗口整体换行，不挤压 */
    margin-bottom: var(--sp-4);
  }
  .section-title {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
  }
  .history-controls {
    display: flex;
    align-items: center;
    gap: var(--sp-3) var(--sp-4);
    flex-wrap: wrap;
  }
  .series-group {
    display: flex;
    gap: var(--sp-1);
    flex-wrap: wrap;
  }
  .series-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text-muted);
    font-family: inherit;
    font-size: 12px;
    padding: 4px 10px;
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: all 0.15s;
  }
  .series-btn .series-dot {
    width: 8px;
    height: 8px;
    border-radius: 999px;
    background: var(--sc);
    opacity: 0.35;
    transition: opacity 0.15s;
  }
  .series-btn.active {
    color: var(--text);
    border-color: var(--sc);
    background: color-mix(in srgb, var(--sc) 12%, transparent);
  }
  .series-btn.active .series-dot {
    opacity: 1;
  }
  .range-group {
    display: flex;
    gap: var(--sp-2);
    flex-wrap: wrap;
  }
  .range-btn {
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--text-muted);
    font-size: 12px;
    padding: 4px 12px;
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: all 0.15s;
  }
  .range-btn:hover {
    background: var(--border);
    color: var(--text);
  }
  .range-btn.active {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }
  .history-chart {
    position: relative;
  }
  .history-svg {
    width: 100%;
    height: 160px;
    display: block;
    cursor: crosshair;
  }
  .grid-line {
    stroke: var(--border);
    stroke-width: 1;
    stroke-dasharray: 3 4;
    vector-effect: non-scaling-stroke;
    opacity: 0.5;
  }
  .cursor-line {
    position: absolute;
    top: 0;
    bottom: 18px;
    width: 1px;
    background: var(--text-muted);
    opacity: 0.5;
    pointer-events: none;
    transform: translateX(-0.5px);
  }
  .hover-tip {
    position: absolute;
    top: 8px;
    transform: translateX(10px);
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 8px 10px;
    font-size: 12px;
    pointer-events: none;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.35);
    min-width: 120px;
    z-index: 2;
  }
  .hover-tip.flip {
    transform: translateX(calc(-100% - 10px));
  }
  .tip-time {
    font-size: 11px;
    color: var(--text-muted);
    margin-bottom: 5px;
  }
  .tip-row {
    display: flex;
    align-items: center;
    gap: 6px;
    line-height: 1.7;
  }
  .tip-dot {
    width: 8px;
    height: 8px;
    border-radius: 999px;
    flex-shrink: 0;
  }
  .tip-label {
    color: var(--text-muted);
  }
  .tip-val {
    margin-left: auto;
    font-variant-numeric: tabular-nums;
  }
  .chart-labels {
    display: flex;
    justify-content: space-between;
    font-size: 10px;
    color: var(--text-muted);
    font-family: var(--font-mono);
    margin-top: var(--sp-1);
  }
  /* X 轴时间标签：长窗口带日期 */
  .x-labels {
    display: flex;
    justify-content: space-between;
    font-size: 10px;
    color: var(--text-muted);
    font-family: var(--font-mono);
    margin-top: 2px;
  }
  /* 数据源状态行（仿 .card-foot 风格） */
  .hist-status {
    margin-top: var(--sp-3);
    font-size: 11px;
    color: var(--text-muted);
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
