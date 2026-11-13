<script context="module">
  // 扫描结果缓存（模块级，切换视图后返回可恢复，避免重复扫描）
  let scanCache = null;
  // 进行中的扫描任务（切换视图再回来时可重新接上，避免并发重复扫描）
  let scanJob = null;
</script>

<script>
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { open as shellOpen } from "@tauri-apps/api/shell";
  import { open as openDialog } from "@tauri-apps/api/dialog";
  import { listen } from "@tauri-apps/api/event";
  import { pushToast, elevated, relaunchAdmin } from "../stores.js";

  let tab = "overview";
  let expandedDisk = -1; // 展开详情的物理盘 index

  // ===== 分区与物理磁盘 =====
  let volumes = [];
  let disks = [];
  let smartNote = "";
  let loading = true;
  let timer;

  function fmt(bytes) {
    if (!bytes) return "0 B";
    const u = ["B", "KB", "MB", "GB", "TB"];
    let i = 0;
    let v = bytes;
    while (v >= 1024 && i < u.length - 1) {
      v /= 1024;
      i++;
    }
    return `${v.toFixed(i >= 3 ? 1 : 0)} ${u[i]}`;
  }

  async function refresh() {
    try {
      const r = await invoke("get_disk_report");
      volumes = r.volumes;
      disks = r.disks;
      smartNote = r.smart_note;
      loading = false;
    } catch (e) {
      pushToast("加载磁盘信息失败：" + e, "error");
    }
  }

  onMount(() => {
    refresh();
    timer = setInterval(refresh, 15000);
    if (scanJob) {
      // 扫描仍在进行：重新接上任务状态
      analysisTarget = { kind: scanJob.kind, target: scanJob.target };
      scanDrive = scanJob.kind === "volume" ? scanJob.target : "";
      scanLoading = true;
      scanProgress = null;
      scanJob.promise
        .then(() => {
          scanLoading = false;
          restoreCache();
        })
        .catch((e) => {
          scanLoading = false;
          scanError = String(e);
        });
    } else {
      restoreCache();
    }
    listen("scan-progress", (e) => {
      if (!scanLoading) return;
      const p = e.payload || {};
      if (analysisTarget && p.target !== analysisTarget.target) return;
      scanProgress = p;
    }).then((u) => (unlistenProgress = u));
  });
  onDestroy(() => {
    clearInterval(timer);
    if (unlistenProgress) unlistenProgress();
  });

  const sev = (v) => (v >= 90 ? "danger" : v >= 75 ? "warn" : "ok");

  function selectTab(next) {
    tab = next;
  }

  // ===== 空间分析 =====
  // 扫描目标：{ kind: "volume"|"dir", target: "C:" | "D:\\path" }
  let analysisTarget = null;
  let scanDrive = "";
  let scanLoading = false;
  let scanResult = null;
  let scanError = null;
  let cancelRequested = false;
  let scanProgress = null;
  let scanAt = 0;
  let unlistenProgress = null;

  // TreeSize 式行内展开
  let expanded = new Set([""]);
  let childrenCache = new Map(); // parentRel -> DirSummary[]
  let sortedDirs = null; // 按路径排序的引用副本（前缀二分查找子目录，省内存）
  let filesCache = new Map(); // rel -> DirFileEntry[]
  let filesLoading = new Set();

  function restoreCache() {
    if (!scanCache) return;
    scanResult = scanCache.result;
    analysisTarget = { kind: scanCache.kind, target: scanCache.target };
    scanDrive = scanCache.kind === "volume" ? scanCache.target : "";
    scanAt = scanCache.at;
    expanded = new Set([""]);
    childrenCache = new Map();
    sortedDirs = null;
    filesCache = new Map();
    filesLoading = new Set();
    loadFiles("");
  }

  function clearResult() {
    scanResult = null;
    scanError = null;
    scanAt = 0;
    expanded = new Set([""]);
    childrenCache = new Map();
    sortedDirs = null;
    filesCache = new Map();
    filesLoading = new Set();
  }

  // 点分区卡片：进入空间分析并预选该盘（不自动扫描）
  function openAnalyzerForDrive(drive) {
    selectTab("analyzer");
    if (scanCache && scanCache.kind === "volume" && scanCache.target === drive) {
      restoreCache();
      return;
    }
    clearResult();
    scanDrive = drive;
    analysisTarget = { kind: "volume", target: drive };
  }

  function selectDrive(drive) {
    if (scanLoading) return;
    scanDrive = drive;
    analysisTarget = { kind: "volume", target: drive };
    if (!(scanResult && scanCache && scanCache.kind === "volume" && scanCache.target === drive)) {
      clearResult();
    }
  }

  async function startScan() {
    if (scanLoading) return;
    if (!analysisTarget) return;
    await doScan(analysisTarget.kind, analysisTarget.target);
  }

  async function pickFolderAndScan() {
    if (scanLoading) return;
    try {
      const picked = await openDialog({ directory: true, multiple: false, title: "选择要分析的文件夹" });
      if (!picked || Array.isArray(picked)) return;
      scanDrive = "";
      analysisTarget = { kind: "dir", target: picked };
      await doScan("dir", picked);
    } catch (e) {
      pushToast("选择文件夹失败：" + e, "error");
    }
  }

  async function doScan(kind, target) {
    if (scanJob) return; // 已有扫描进行中（例如切换视图后返回）
    scanLoading = true;
    scanResult = null;
    scanError = null;
    cancelRequested = false;
    scanProgress = null;
    expanded = new Set([""]);
    childrenCache = new Map();
    sortedDirs = null;
    filesCache = new Map();
    filesLoading = new Set();
    const job = { kind, target };
    job.promise = (kind === "volume"
      ? invoke("scan_volume", { drive: target, topN: 50 })
      : invoke("scan_directory", { dirPath: target, topN: 50 })
    )
      .then((result) => {
        scanCache = { kind, target, result, at: Date.now() };
        return result;
      })
      .finally(() => {
        if (scanJob === job) scanJob = null;
      });
    scanJob = job;
    try {
      const result = await job.promise;
      scanResult = result;
      scanAt = scanCache?.at || Date.now();
      if (result.errors > 0) {
        pushToast(`扫描完成，${result.errors} 个文件/目录无法访问`, "warn");
      }
      loadFiles("");
    } catch (e) {
      scanError = String(e);
      pushToast("扫描失败：" + e, "error");
    } finally {
      scanLoading = false;
      scanProgress = null;
    }
  }

  async function cancelScan() {
    if (cancelRequested) return;
    cancelRequested = true;
    try {
      await invoke("cancel_scan");
    } catch {}
  }

  // 目录的直接子目录：按路径排序后二分定位前缀，再过滤出直接子目录。
  // 只排序一次（引用副本），内存开销小；每个父目录的结果单独缓存。
  function childrenOf(rel) {
    if (childrenCache.has(rel)) return childrenCache.get(rel);
    const out = [];
    if (scanResult) {
      const seen = new Set();
      if (rel === "") {
        // 根的直接子目录：单次遍历即可，避免首次展示前排序全量路径
        for (const d of scanResult.dirs) {
          if (d.path.indexOf("\\") === -1 && !seen.has(d.path)) {
            seen.add(d.path);
            out.push(d);
          }
        }
      } else {
        if (!sortedDirs) {
          sortedDirs = scanResult.dirs.slice().sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
        }
        const prefix = rel + "\\";
        let lo = 0;
        let hi = sortedDirs.length;
        while (lo < hi) {
          const mid = (lo + hi) >> 1;
          if (sortedDirs[mid].path < prefix) lo = mid + 1;
          else hi = mid;
        }
        for (let i = lo; i < sortedDirs.length; i++) {
          const p = sortedDirs[i].path;
          if (!p.startsWith(prefix)) break;
          // 直接子目录：前缀之后不再含分隔符（按路径去重，防异常数据重复渲染）
          if (p.indexOf("\\", prefix.length) === -1 && !seen.has(p)) {
            seen.add(p);
            out.push(sortedDirs[i]);
          }
        }
      }
      out.sort((a, b) => b.total_size - a.total_size);
    }
    childrenCache.set(rel, out);
    return out;
  }

  function toggleExpand(rel) {
    const next = new Set(expanded);
    if (next.has(rel)) next.delete(rel);
    else {
      next.add(rel);
      loadFiles(rel);
    }
    expanded = next;
  }

  async function loadFiles(rel) {
    if (filesCache.has(rel) || filesLoading.has(rel)) return;
    filesLoading = new Set(filesLoading).add(rel);
    try {
      const files = await invoke("list_dir_files", { dirPath: absDir(rel), limit: 200 });
      filesCache = new Map(filesCache).set(rel, files);
    } catch {
      filesCache = new Map(filesCache).set(rel, []);
    } finally {
      const s = new Set(filesLoading);
      s.delete(rel);
      filesLoading = s;
    }
  }

  function basePath() {
    if (!analysisTarget) return "";
    if (analysisTarget.kind === "volume") {
      return analysisTarget.target.replace(/[:\\]*$/, "") + ":\\";
    }
    return analysisTarget.target.replace(/[\\/]+$/, "") + "\\";
  }

  function absDir(rel) {
    const base = basePath();
    if (!rel) return base;
    return base + rel;
  }

  function targetLabel() {
    if (!analysisTarget) return "";
    return analysisTarget.kind === "volume" ? `${analysisTarget.target} 整卷` : analysisTarget.target;
  }

  // 构造可见行（展开状态 + 按需文件）
  $: visibleRows = analysisTarget ? buildRows(scanResult, expanded, filesCache, filesLoading, targetLabel()) : [];

  function buildRows(result, expandedSet, fileCache, loadingSet, rootLabel) {
    if (!result) return [];
    const rows = [];
    const rootSize = result.root_size || 0;
    rows.push({
      key: "root",
      kind: "dir",
      depth: 0,
      rel: "",
      name: rootLabel,
      size: rootSize,
      fileCount: result.scanned,
      pct: null,
      modified: 0,
    });
    const pushLevel = (rel, parentSize, depth) => {
      for (const d of childrenOf(rel)) {
        const pct = parentSize > 0 ? (d.total_size / parentSize) * 100 : null;
        rows.push({
          key: "d|" + d.path,
          kind: "dir",
          depth,
          rel: d.path,
          name: d.path.split("\\").filter(Boolean).pop() || d.path,
          size: d.total_size,
          fileCount: d.file_count,
          pct,
          modified: d.modified_secs,
        });
        if (expandedSet.has(d.path)) {
          pushLevel(d.path, d.total_size, depth + 1);
          pushFiles(d.path, d.total_size, depth + 1);
        }
      }
    };
    const pushFiles = (rel, parentSize, depth) => {
      const files = fileCache.get(rel);
      if (!files) {
        if (loadingSet.has(rel)) {
          rows.push({ key: "loading|" + rel, kind: "loading", depth, name: "正在读取文件…" });
        }
        return;
      }
      const seenFiles = new Set();
      for (const f of files) {
        if (seenFiles.has(f.path)) continue;
        seenFiles.add(f.path);
        rows.push({
          key: "f|" + f.path,
          kind: "file",
          depth,
          rel,
          name: f.name,
          size: f.size,
          fileCount: null,
          pct: parentSize > 0 ? (f.size / parentSize) * 100 : null,
          modified: f.modified_secs,
          absPath: f.path,
        });
      }
    };
    if (expandedSet.has("")) {
      pushLevel("", rootSize, 1);
      pushFiles("", rootSize, 1);
    }
    return rows;
  }

  function openTreeRow(row) {
    if (row.kind === "loading") return;
    if (row.kind === "file") {
      openInExplorer(parentOfAbs(row.absPath));
      return;
    }
    toggleExpand(row.rel);
  }

  function openTreeFolder(row) {
    if (row.kind !== "dir") return;
    openInExplorer(absDir(row.rel));
  }

  // ===== 通用 =====
  function parentOfAbs(p) {
    const i = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
    if (i < 0) return p;
    // "C:\file" → "C:\"（盘根），避免返回 "C:" 这种驱动器相对路径
    if (i === 2 && /^[a-zA-Z]:[\\/]/.test(p)) return p.slice(0, 3);
    return i > 2 ? p.slice(0, i) : p;
  }

  async function openInExplorer(target) {
    try {
      await shellOpen(target);
    } catch (e) {
      pushToast("无法打开：" + target, "error");
    }
  }

  // 大文件路径：MFT 返回相对路径，walk 返回绝对路径
  function absPathFor(raw) {
    if (!raw) return basePath();
    if (/^[a-zA-Z]:[\\/]/.test(raw) || raw.startsWith("\\\\")) return raw;
    return absDir(raw);
  }

  function openLargeFile(raw) {
    openInExplorer(parentOfAbs(absPathFor(raw)));
  }

  const fmtSize = (n) => {
    if (!n) return "0 B";
    if (n < 1024) return n + " B";
    if (n < 1024 * 1024) return (n / 1024).toFixed(1) + " KB";
    if (n < 1024 * 1024 * 1024) return (n / 1024 / 1024).toFixed(1) + " MB";
    return (n / 1024 / 1024 / 1024).toFixed(2) + " GB";
  };

  // 文件数量：用千分位整数，避免 K/M 与容量单位混淆
  const fmtNum = (n) => (n || 0).toLocaleString("zh-CN");

  const fmtTime = (secs) => {
    if (!secs) return "—";
    const d = new Date(secs * 1000);
    return d.toLocaleDateString("zh-CN") + " " + d.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
  };

  const srcLabel = (s) => (s === "mft" ? "MFT 极速扫描" : "逐目录扫描");

  const fmtAgo = (ts) => {
    if (!ts) return "";
    const mins = Math.floor((Date.now() - ts) / 60000);
    if (mins < 1) return "刚刚";
    if (mins < 60) return `${mins} 分钟前`;
    return `${Math.floor(mins / 60)} 小时前`;
  };

  // ===== 物理盘寿命辅助 =====
  const fmtHours = (h) => {
    if (h == null || h <= 0) return "—";
    const years = h / 8766;
    if (years >= 1) return `${years.toFixed(1)} 年（${h} 小时）`;
    return `${Math.floor(h / 24)} 天（${h} 小时）`;
  };
  const fmtUnits = (units) => {
    if (!units) return "—";
    const bytes = units * 512000; // NVMe 数据单元 = 1000 × 512 B
    if (bytes >= 1e12) return (bytes / 1e12).toFixed(2) + " TB";
    return (bytes / 1e9).toFixed(1) + " GB";
  };
  // 累计写入字节 ÷ 盘容量 = 全盘写入次数（直观的磨损参考）
  const fullDiskWrites = (nvme, size) => {
    if (!nvme || !size) return null;
    return ((nvme.data_units_written * 512000) / size).toFixed(1);
  };
  const wearClass = (w) => (w >= 80 ? "danger" : w >= 50 ? "warn" : "ok");
  const nvmeWarnText = (cw) => {
    if (!cw) return "无";
    const items = [];
    if (cw & 1) items.push("备用空间低于阈值");
    if (cw & 2) items.push("温度超限");
    if (cw & 4) items.push("可靠性降低");
    if (cw & 8) items.push("已进入只读");
    if (cw & 16) items.push("易失缓存备份失败");
    if (cw & 32) items.push("持久存储只读");
    return items.length ? items.join("、") : `0x${cw.toString(16)}`;
  };
</script>

<div class="tabs" role="tablist">
  <button class="tab" class:active={tab === "overview"} on:click={() => selectTab("overview")} role="tab" aria-selected={tab === "overview"}>磁盘概况</button>
  <button class="tab" class:active={tab === "analyzer"} on:click={() => selectTab("analyzer")} role="tab" aria-selected={tab === "analyzer"}>空间分析</button>
</div>

{#if tab === "overview"}
  <div class="head">
    <h2 class="section-title">分区使用率</h2>
    <button class="ghost" on:click={refresh}>刷新</button>
  </div>

  {#if loading}
    <p class="muted">加载中…</p>
  {:else}
    <div class="vol-grid">
      {#each volumes as v}
        <div
          class="card vol-card"
          role="button"
          tabindex="0"
          title="点击进入该分区的空间分析"
          on:click={() => openAnalyzerForDrive(v.drive)}
          on:keydown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), openAnalyzerForDrive(v.drive))}
        >
          <div class="card-head">
            <span class="vol-name">
              <strong>{v.drive}</strong>
              <span class="vol-label">{v.label}</span>
            </span>
            <span class="pct {sev(v.used_pct)}">{v.used_pct.toFixed(0)}%</span>
          </div>
          <div class="bar">
            <div class="bar-fill {sev(v.used_pct)}" style="width:{Math.min(100, v.used_pct)}%"></div>
          </div>
          <div class="vol-meta">
            <span>{v.fs} · {v.drive_type}</span>
            <span>可用 {fmt(v.free)} / 共 {fmt(v.total)}</span>
          </div>
          <span class="vol-action">空间分析 ›</span>
        </div>
      {/each}
    </div>

    <div class="head" style="margin-top: var(--sp-8)">
      <h2 class="section-title">物理磁盘健康</h2>
    </div>

    {#if smartNote}
      <div class="note-box warn">{smartNote}</div>
    {:else if disks.length === 0}
      <p class="muted">未检测到物理磁盘。</p>
    {:else}
      <div class="disk-list">
        {#each disks as d}
          <div
            class="card disk-row"
            class:open={expandedDisk === d.index}
            role="button"
            tabindex="0"
            title="点击查看详情"
            on:click={() => (expandedDisk = expandedDisk === d.index ? -1 : d.index)}
            on:keydown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                expandedDisk = expandedDisk === d.index ? -1 : d.index;
              }
            }}
          >
            <div class="disk-main">
              <span class="chev">{expandedDisk === d.index ? "▾" : "▸"}</span>
              <span class="health {d.healthy ? 'ok' : 'bad'}" title={d.status}></span>
              <div>
                <div class="disk-model">{d.model || "未知型号"}</div>
                <div class="disk-sub">
                  {d.media || "—"} · {d.interface || "—"}{d.firmware ? ` · FW ${d.firmware}` : ""}{d.serial ? ` · SN ${d.serial}` : ""}{#if d.volumes?.length} · 分区 {d.volumes.join(" ")}{/if}
                </div>
              </div>
            </div>
            <div class="disk-right">
              {#if d.wear != null}
                <span class="wear-badge {wearClass(d.wear)}">磨损 {d.wear}%</span>
              {/if}
              {#if d.temperature != null}
                <span class="temp">{d.temperature}°C</span>
              {/if}
              <span class="disk-size">{d.size > 0 ? fmt(d.size) : "—"}</span>
              <span class="status-badge {d.healthy ? 'ok' : 'bad'}">{d.status}</span>
            </div>
          </div>
          {#if expandedDisk === d.index}
            <div class="disk-detail">
              <div class="dd-section">
                <h4 class="dd-title">基本信息</h4>
                <div class="kv-grid">
                  <span class="kv"><i>型号</i><b>{d.model || "—"}</b></span>
                  <span class="kv"><i>固件</i><b>{d.firmware || "—"}</b></span>
                  <span class="kv"><i>序列号</i><b>{d.serial || "—"}</b></span>
                  <span class="kv"><i>接口 / 介质</i><b>{d.interface || "—"} / {d.media || "—"}</b></span>
                  <span class="kv"><i>容量</i><b>{d.size > 0 ? fmt(d.size) : "—"}</b></span>
                  <span class="kv"><i>承载分区</i><b>{d.volumes?.length ? d.volumes.join(" ") : "—"}</b></span>
                </div>
              </div>

              <div class="dd-section">
                <h4 class="dd-title">健康与寿命</h4>
                <div class="kv-grid">
                  <span class="kv"><i>SMART 状态</i><b class={d.healthy ? "ok-text" : "bad-text"}>{d.status}</b></span>
                  <span class="kv"><i>通电时长</i><b>{fmtHours(d.power_on_hours)}</b></span>
                  <span class="kv"><i>当前温度</i><b>{d.temperature != null ? d.temperature + "°C" : "—"}</b></span>
                  <span class="kv"><i>历史最高温度</i><b>{d.temperature_max != null ? d.temperature_max + "°C" : "—"}</b></span>
                  <span class="kv"><i>通电次数</i><b>{d.power_cycles ?? "—"}</b></span>
                  <span class="kv"><i>磁头加载/卸载</i><b>{d.load_unload_cycles ?? "—"}</b></span>
                  <span class="kv"><i>制造日期</i><b>{d.manufacture_date || "—"}</b></span>
                  <span class="kv"><i>累计读写错误</i><b>{d.read_errors != null || d.write_errors != null ? `读 ${d.read_errors ?? 0} / 写 ${d.write_errors ?? 0}` : "—"}</b></span>
                </div>
                {#if d.wear != null}
                  <div class="wear-row">
                    <span class="wear-label">磨损程度</span>
                    <div class="wear-track"><div class="wear-fill {wearClass(d.wear)}" style="width:{Math.min(100, d.wear)}%"></div></div>
                    <span class="mono {wearClass(d.wear)}-text">{d.wear}%</span>
                  </div>
                {:else}
                  <p class="dd-note">该盘未提供磨损百分比（多数机械盘无此指标）。</p>
                {/if}
                <p class="dd-note">已用寿命由硬盘自行上报（整数百分比，0% 表示低于 1%）；「全盘写入次数」= 累计写入量 ÷ 容量，可直观参考磨损程度。</p>
              </div>

              {#if d.nvme}
                <div class="dd-section">
                  <h4 class="dd-title">NVMe 健康日志</h4>
                  <div class="kv-grid">
                    <span class="kv"><i>健康警告</i><b>{nvmeWarnText(d.nvme.critical_warning)}</b></span>
                    <span class="kv"><i>已用寿命</i><b>{d.nvme.percentage_used}%</b></span>
                    <span class="kv"><i>可用备用空间</i><b>{d.nvme.available_spare}%（阈值 {d.nvme.available_spare_threshold}%）</b></span>
                    <span class="kv"><i>累计读取</i><b>{fmtUnits(d.nvme.data_units_read)}</b></span>
                    <span class="kv"><i>累计写入</i><b>{fmtUnits(d.nvme.data_units_written)}</b></span>
                    {#if fullDiskWrites(d.nvme, d.size)}
                      <span class="kv"><i>≈ 全盘写入</i><b>{fullDiskWrites(d.nvme, d.size)} 次</b></span>
                    {/if}
                    <span class="kv"><i>异常掉电</i><b>{d.nvme.unsafe_shutdowns} 次</b></span>
                    <span class="kv"><i>介质错误</i><b>{d.nvme.media_errors} 次</b></span>
                  </div>
                </div>
              {/if}

              {#if d.smart_attributes?.length}
                <details class="dd-section">
                  <summary class="dd-title clickable">SMART 属性明细（{d.smart_attributes.length} 项）</summary>
                  <div class="attr-wrap">
                    <table class="attr-table">
                      <thead><tr><th>ID</th><th>属性</th><th class="num">当前</th><th class="num">最差</th><th class="num">原始值</th></tr></thead>
                      <tbody>
                        {#each d.smart_attributes as a}
                          <tr>
                            <td class="mono">{a.id}</td>
                            <td>{a.name}</td>
                            <td class="num mono">{a.value}</td>
                            <td class="num mono">{a.worst}</td>
                            <td class="num mono">{a.raw}</td>
                          </tr>
                        {/each}
                      </tbody>
                    </table>
                  </div>
                </details>
              {/if}

              <p class="dd-note">数据来源：{d.data_source}。</p>
            </div>
          {/if}
        {/each}
      </div>
      <p class="muted small">
        健康状态来自 Win32_DiskDrive（SMART 预测：OK / Degraded / Pred Fail）。寿命与温度来自 NVMe 健康日志 / SATA SMART / MSFT_StorageReliabilityCounter，需管理员权限——未显示即表示未提权或该盘不支持。点击磁盘行可查看详情。
      </p>
    {/if}
  {/if}
{:else}
  <!-- ===== 空间分析 ===== -->
  <section class="analyzer">
    <div class="drive-picker">
      {#each volumes as v}
        <button
          class="drive-chip"
          class:active={analysisTarget?.kind === "volume" && scanDrive === v.drive}
          on:click={() => selectDrive(v.drive)}
          disabled={scanLoading}
        >
          <span class="dc-drive mono">{v.drive}</span>
          <span class="dc-meta">{fmt(v.free)} 可用 / {fmt(v.total)}</span>
        </button>
      {/each}
    </div>

    <div class="analyzer-actions">
      <button class="primary" on:click={startScan} disabled={scanLoading || !analysisTarget}>
        {scanLoading ? "扫描中…" : scanResult ? "重新扫描" : "开始扫描"}
      </button>
      <button class="ghost" on:click={pickFolderAndScan} disabled={scanLoading}>扫描文件夹…</button>
      {#if !$elevated}
        <span class="elev-hint">
          MFT 极速扫描需管理员权限，当前将使用逐目录扫描（较慢）。
          <button class="link" on:click={relaunchAdmin}>以管理员重启</button>
        </span>
      {/if}
    </div>

    {#if scanLoading}
      <div class="scan-card">
        <div class="scan-line">
          <span class="scan-target">正在扫描 {analysisTarget?.target}</span>
          {#if scanProgress?.percent != null}
            <span class="mono">{scanProgress.percent.toFixed(0)}%</span>
          {/if}
        </div>
        <div class="progress" class:indeterminate={scanProgress?.percent == null}>
          <div class="progress-fill" style={scanProgress?.percent != null ? `width:${scanProgress.percent}%` : ""}></div>
        </div>
        <div class="scan-sub muted">
          {#if scanProgress}
            已处理 <b class="mono">{fmtNum(scanProgress.scanned)}</b>{#if scanProgress.total} / <span class="mono">{fmtNum(scanProgress.total)}</span>{:else} 项{/if}
            · {srcLabel(scanProgress.source)}
          {:else}
            准备中…
          {/if}
        </div>
        <button class="ghost" on:click={cancelScan} disabled={cancelRequested}>
          {cancelRequested ? "正在停止…" : "取消扫描"}
        </button>
      </div>
    {/if}

    {#if scanError}
      <div class="note">{scanError}</div>
    {/if}

    {#if !scanResult && !scanLoading && !scanError}
      <div class="center-cta">
        <p class="muted">选择上方磁盘（或扫描指定文件夹），然后点击「开始扫描」。扫描结果会保留，切换页面再回来无需重扫。</p>
      </div>
    {/if}

    {#if scanResult && !scanLoading}
      {#if scanResult.cancelled}
        <div class="note-box warn">扫描已手动取消，以下为取消前已收集到的部分结果。</div>
      {/if}

      <div class="scan-summary-row">
        <div class="scan-summary">
          <div class="sum-item"><span class="sum-val mono">{fmtNum(scanResult.scanned)}</span><span class="sum-label">已扫描文件（个）</span></div>
          <div class="sum-item"><span class="sum-val mono">{(scanResult.elapsed_ms / 1000).toFixed(2)}s</span><span class="sum-label">耗时</span></div>
          <div class="sum-item"><span class="sum-val src-tag" class:mft={scanResult.source === "mft"}>{srcLabel(scanResult.source)}</span><span class="sum-label">{scanAt ? `扫描于 ${fmtAgo(scanAt)}` : "扫描方式"}</span></div>
          {#if scanResult.errors > 0}
            <div class="sum-item"><span class="sum-val mono danger-text">{scanResult.errors}</span><span class="sum-label">无法访问</span></div>
          {/if}
        </div>
      </div>

      <div class="tree-wrap">
        <div class="tree-head">
          <span class="th-name">名称</span>
          <span class="th-num">大小</span>
          <span class="th-num">文件数</span>
          <span class="th-num">占父级</span>
          <span class="th-time">修改时间</span>
          <span class="th-act"></span>
        </div>
        <div class="tree-body">
          {#each visibleRows as row (row.key)}
            {#if row.kind === "loading"}
              <div class="tree-row loading" style="--depth:{row.depth}">
                <span class="tr-name"><span class="tr-icon">…</span>{row.name}</span>
              </div>
            {:else}
              <div class="tree-row" class:file={row.kind === "file"} style="--depth:{row.depth}">
                <span class="tr-name">
                  {#if row.kind === "dir"}
                    <button class="twisty" class:open={expanded.has(row.rel)} on:click={() => toggleExpand(row.rel)} aria-label={expanded.has(row.rel) ? "收起" : "展开"}>▸</button>
                    <span class="tr-icon">📁</span>
                  {:else}
                    <span class="twisty-spacer"></span>
                    <span class="tr-icon">📄</span>
                  {/if}
                  <button class="tr-label" title={row.kind === "dir" ? absDir(row.rel) : row.absPath} on:click={() => openTreeRow(row)}>{row.name}</button>
                </span>
                <span class="tr-num mono">{fmtSize(row.size)}</span>
                <span class="tr-num mono">{row.kind === "dir" ? fmtNum(row.fileCount) : "—"}</span>
                <span class="tr-num mono">{row.pct != null ? row.pct.toFixed(1) + "%" : "—"}</span>
                <span class="tr-time mono">{fmtTime(row.modified)}</span>
                <span class="tr-act">
                  <button class="row-act" title={row.kind === "dir" ? "在资源管理器中打开目录" : "在资源管理器中打开所在目录"} on:click={() => (row.kind === "dir" ? openTreeFolder(row) : openInExplorer(parentOfAbs(row.absPath)))}>打开</button>
                </span>
              </div>
            {/if}
          {/each}
        </div>
      </div>

      {#if scanResult.large_files?.length > 0}
        <section class="files-section">
          <h4 class="section-subtitle">最大文件 TOP {scanResult.large_files.length}</h4>
          <div class="table-wrap small-table">
            <table>
              <thead>
                <tr><th>文件路径</th><th class="num">大小</th><th class="num">修改时间</th><th class="col-fact">操作</th></tr>
              </thead>
              <tbody>
                {#each scanResult.large_files as f (f.path)}
                  <tr>
                    <td class="file-path mono" title={f.path}>{f.path}</td>
                    <td class="num mono">{fmtSize(f.size)}</td>
                    <td class="num mono">{fmtTime(f.modified_secs)}</td>
                    <td class="col-fact">
                      <button class="row-act" title="在资源管理器中定位" on:click={() => openLargeFile(f.path)}>定位</button>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </section>
      {/if}
    {/if}
  </section>
{/if}

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: var(--sp-4);
  }
  .section-title {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
  }
  .muted {
    color: var(--text-muted);
    font-size: 13px;
  }
  .muted.small {
    font-size: 11px;
    margin-top: var(--sp-3);
  }

  .vol-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: var(--sp-4);
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: var(--sp-4);
  }
  .vol-card {
    cursor: pointer;
    transition: border-color 0.15s ease, background 0.15s ease;
  }
  .vol-card:hover, .vol-card:focus-visible {
    border-color: var(--accent);
    background: var(--surface-2);
    outline: none;
  }
  .vol-action {
    display: block;
    margin-top: 6px;
    text-align: right;
    font-size: 11px;
    color: var(--text-muted);
    transition: color 0.15s ease;
  }
  .vol-card:hover .vol-action, .vol-card:focus-visible .vol-action {
    color: var(--accent);
  }
  .card-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: var(--sp-3);
  }
  .vol-name strong {
    font-size: 16px;
    font-family: var(--font-mono);
  }
  .vol-label {
    color: var(--text-muted);
    font-size: 13px;
    margin-left: 8px;
  }
  .pct {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    font-weight: 600;
  }
  .pct.ok {
    color: var(--text);
  }
  .pct.warn {
    color: var(--warn);
  }
  .pct.danger {
    color: var(--danger);
  }
  .bar {
    height: 10px;
    background: var(--surface-2);
    border-radius: 999px;
    overflow: hidden;
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
  .vol-meta {
    display: flex;
    justify-content: space-between;
    margin-top: var(--sp-3);
    font-size: 12px;
    color: var(--text-muted);
  }

  .disk-list {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .disk-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: var(--sp-3) var(--sp-4);
    cursor: pointer;
    transition: border-color 0.15s ease, background 0.15s ease;
  }
  .disk-row:hover, .disk-row:focus-visible {
    border-color: var(--accent);
    background: var(--surface-2);
    outline: none;
  }
  .disk-row.open {
    border-color: var(--accent);
  }
  .chev {
    width: 14px;
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: 11px;
  }
  .disk-main {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
  }
  .health {
    width: 10px;
    height: 10px;
    border-radius: 999px;
    flex-shrink: 0;
  }
  .health.ok {
    background: var(--ok);
    box-shadow: 0 0 8px rgba(34, 197, 94, 0.6);
  }
  .health.bad {
    background: var(--danger);
    box-shadow: 0 0 8px rgba(239, 68, 68, 0.6);
  }
  .disk-model {
    font-size: 14px;
    font-weight: 500;
  }
  .disk-sub {
    font-size: 12px;
    color: var(--text-muted);
    font-family: var(--font-mono);
    margin-top: 2px;
  }
  .disk-right {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
  }
  .disk-size {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    color: var(--text);
  }
  .temp {
    font-family: var(--font-mono);
    font-size: 12px;
    padding: 3px 8px;
    border-radius: 999px;
    color: #93c5fd;
    background: rgba(59, 130, 246, 0.14);
  }
  .status-badge {
    font-size: 11px;
    padding: 3px 10px;
    border-radius: 999px;
  }
  .status-badge.ok {
    color: var(--ok);
    background: rgba(34, 197, 94, 0.12);
  }
  .status-badge.bad {
    color: var(--danger);
    background: rgba(239, 68, 68, 0.12);
  }
  .wear-badge {
    font-size: 11px;
    padding: 3px 8px;
    border-radius: 999px;
    font-family: var(--font-mono);
  }
  .wear-badge.ok {
    color: var(--ok);
    background: rgba(34, 197, 94, 0.12);
  }
  .wear-badge.warn {
    color: var(--warn);
    background: rgba(245, 158, 11, 0.14);
  }
  .wear-badge.danger {
    color: var(--danger);
    background: rgba(239, 68, 68, 0.14);
  }
  .ok-text {
    color: var(--ok);
  }
  .bad-text {
    color: var(--danger);
  }
  .warn-text {
    color: var(--warn);
  }
  .danger-text {
    color: var(--danger);
  }

  /* 物理盘详情 */
  .disk-detail {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    padding: var(--sp-4);
    margin-top: -4px;
  }
  .dd-section + .dd-section {
    margin-top: var(--sp-4);
    padding-top: var(--sp-4);
    border-top: 1px solid var(--border);
  }
  .dd-title {
    margin: 0 0 var(--sp-3);
    font-size: 13px;
    font-weight: 600;
  }
  .dd-title.clickable {
    cursor: pointer;
  }
  .kv-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
    gap: 8px var(--sp-4);
  }
  .kv {
    display: flex;
    gap: 8px;
    font-size: 12px;
    min-width: 0;
  }
  .kv i {
    font-style: normal;
    color: var(--text-muted);
    flex-shrink: 0;
    min-width: 84px;
  }
  .kv b {
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .wear-row {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin-top: var(--sp-3);
    font-size: 12px;
  }
  .wear-label {
    color: var(--text-muted);
    flex-shrink: 0;
  }
  .wear-track {
    flex: 1;
    max-width: 320px;
    height: 8px;
    border-radius: 999px;
    background: var(--surface-2);
    overflow: hidden;
  }
  .wear-fill {
    height: 100%;
    border-radius: 999px;
  }
  .wear-fill.ok {
    background: var(--ok);
  }
  .wear-fill.warn {
    background: var(--warn);
  }
  .wear-fill.danger {
    background: var(--danger);
  }
  .dd-note {
    margin: var(--sp-3) 0 0;
    font-size: 11px;
    color: var(--text-muted);
    line-height: 1.6;
  }
  .attr-wrap {
    margin-top: var(--sp-3);
    max-height: 280px;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .attr-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
  }
  .attr-table th, .attr-table td {
    padding: 4px 10px;
    text-align: left;
    border-top: 1px solid var(--border);
    white-space: nowrap;
  }
  .attr-table th {
    position: sticky;
    top: 0;
    background: var(--surface-2);
    color: var(--text-muted);
    font-weight: 500;
  }
  .attr-table td.num, .attr-table th.num {
    text-align: right;
  }
  .note-box {
    font-size: 12px;
    line-height: 1.6;
    padding: 10px 12px;
    border-radius: 8px;
  }
  .note-box.warn {
    color: #fcd34d;
    background: rgba(245, 158, 11, 0.1);
    border: 1px solid rgba(245, 158, 11, 0.35);
  }
  .ghost {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
    padding: 6px 14px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .ghost:hover {
    background: var(--surface-2);
  }
  .primary {
    border: none;
    background: linear-gradient(135deg, var(--accent), #7c3aed);
    color: #fff;
    font-family: inherit;
    font-size: 13px;
    font-weight: 600;
    padding: 8px 18px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .primary:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .link {
    border: none;
    background: transparent;
    color: var(--accent);
    font-family: inherit;
    font-size: 12px;
    cursor: pointer;
    padding: 0 2px;
    text-decoration: underline;
  }

  /* ===== Tabs ===== */
  .tabs {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    margin-bottom: var(--sp-4);
    border-bottom: 1px solid var(--border);
  }
  .tab {
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
  }
  .tab:hover {
    color: var(--text);
  }
  .tab.active {
    color: var(--text);
    border-bottom-color: var(--accent);
  }

  /* ===== 空间分析 ===== */
  .mono {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }
  .drive-picker {
    display: flex;
    gap: var(--sp-3);
    flex-wrap: wrap;
    margin-bottom: var(--sp-3);
  }
  .drive-chip {
    display: flex;
    flex-direction: column;
    gap: 2px;
    align-items: flex-start;
    min-width: 140px;
    padding: 10px 16px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--surface);
    color: var(--text);
    font-family: inherit;
    cursor: pointer;
    transition: border-color 0.15s ease, background 0.15s ease;
  }
  .drive-chip:hover:not(:disabled) {
    border-color: var(--accent);
  }
  .drive-chip.active {
    border-color: var(--accent);
    background: rgba(99, 102, 241, 0.12);
  }
  .drive-chip:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .dc-drive {
    font-size: 16px;
    font-weight: 700;
  }
  .dc-meta {
    font-size: 11px;
    color: var(--text-muted);
  }
  .analyzer-actions {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    flex-wrap: wrap;
    margin-bottom: var(--sp-4);
  }
  .elev-hint {
    font-size: 12px;
    color: var(--text-muted);
  }

  .scan-card {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    padding: var(--sp-4);
    margin-bottom: var(--sp-4);
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
  .scan-line {
    display: flex;
    justify-content: space-between;
    font-size: 13px;
  }
  .scan-target {
    color: var(--text);
  }
  .progress {
    height: 10px;
    border-radius: 999px;
    background: var(--surface-2);
    overflow: hidden;
    position: relative;
  }
  .progress-fill {
    height: 100%;
    border-radius: 999px;
    background: linear-gradient(90deg, var(--accent), var(--ok));
    transition: width 0.2s ease;
  }
  .progress.indeterminate .progress-fill {
    width: 30%;
    animation: slide 1.2s ease-in-out infinite;
  }
  @keyframes slide {
    0% { margin-left: 0; }
    50% { margin-left: 70%; }
    100% { margin-left: 0; }
  }
  .scan-sub {
    font-size: 12px;
  }
  .scan-card .ghost {
    align-self: flex-start;
  }

  .center-cta {
    padding: var(--sp-8);
    text-align: center;
  }
  .note {
    padding: var(--sp-3);
    margin-bottom: var(--sp-4);
    border: 1px solid rgba(245, 158, 11, 0.35);
    background: rgba(245, 158, 11, 0.1);
    border-radius: var(--radius-sm);
    font-size: 13px;
    color: #fcd34d;
  }
  .scan-summary-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-4);
    margin-bottom: var(--sp-4);
  }
  .scan-summary {
    display: flex;
    gap: var(--sp-8);
  }
  .sum-item {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .sum-val {
    font-size: 24px;
    font-weight: 700;
  }
  .src-tag {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-muted);
    align-self: flex-start;
    padding: 4px 10px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    margin-top: 2px;
  }
  .src-tag.mft {
    color: var(--ok);
    background: rgba(34, 197, 94, 0.12);
  }
  .sum-label {
    font-size: 12px;
    color: var(--text-muted);
  }
  .danger-text {
    color: var(--danger);
  }

  .tree-wrap {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--surface);
    margin-bottom: var(--sp-6);
  }
  .tree-head, .tree-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 96px 72px 68px 130px 56px;
    align-items: center;
    gap: var(--sp-2);
  }
  .tree-head {
    background: var(--surface-2);
    padding: 8px 14px;
    color: var(--text-muted);
    font-size: 12px;
  }
  .tree-body {
    max-height: calc(100vh - 430px);
    min-height: 120px;
    overflow: auto;
  }
  .tree-row {
    padding: 3px 14px;
    border-top: 1px solid var(--border);
    font-size: 13px;
  }
  .tree-row:hover {
    background: var(--surface-2);
  }
  .tree-row.file {
    color: var(--text-muted);
  }
  .tree-row.loading {
    color: var(--text-muted);
    font-size: 12px;
  }
  .tr-name {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    padding-left: calc(var(--depth, 0) * 16px);
  }
  .twisty {
    width: 16px;
    height: 16px;
    flex-shrink: 0;
    border: none;
    background: transparent;
    color: var(--text-muted);
    font-size: 10px;
    cursor: pointer;
    transition: transform 0.12s ease;
    padding: 0;
  }
  .twisty.open {
    transform: rotate(90deg);
  }
  .twisty-spacer {
    width: 16px;
    flex-shrink: 0;
  }
  .tr-icon {
    flex-shrink: 0;
    font-size: 12px;
  }
  .tr-label {
    border: none;
    background: transparent;
    color: inherit;
    font-family: inherit;
    font-size: 13px;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: pointer;
    min-width: 0;
    padding: 2px 4px;
    border-radius: 4px;
  }
  .tr-label:hover {
    color: var(--accent);
    background: var(--surface);
  }
  .th-num, .tr-num {
    text-align: right;
  }
  .th-time, .tr-time {
    text-align: right;
    white-space: nowrap;
  }
  .tr-act {
    text-align: right;
  }
  .row-act {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-muted);
    font-family: inherit;
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 6px;
    cursor: pointer;
  }
  .row-act:hover {
    background: var(--surface);
    color: var(--accent);
    border-color: var(--accent);
  }

  .section-subtitle {
    font-size: 14px;
    font-weight: 600;
    margin: 0 0 var(--sp-3);
  }
  .files-section {
    margin-bottom: var(--sp-6);
  }

  .small-table tbody {
    max-height: 400px;
  }
  .file-path {
    max-width: 0;
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
    padding: 8px 14px;
    font-weight: 500;
    color: var(--text-muted);
  }
  th.num {
    text-align: right;
  }
  tbody {
    display: block;
    max-height: 500px;
    overflow-y: auto;
  }
  thead, tbody tr {
    display: table;
    width: 100%;
    table-layout: fixed;
  }
  td {
    padding: 6px 14px;
    border-top: 1px solid var(--border);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  td.num {
    text-align: right;
  }
  tbody tr:hover {
    background: var(--surface-2);
  }
</style>
