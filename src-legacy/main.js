const data = window.dashboardData;

if (data) {
  const { metrics, aiCheckup } = data;

  document.getElementById("cpu-value").textContent = metrics.cpu.value;
  document.getElementById("cpu-sub").textContent = metrics.cpu.summary;

  document.getElementById("memory-value").textContent = metrics.memory.value;
  document.getElementById("memory-sub").textContent = metrics.memory.summary;

  document.getElementById("network-value").textContent = metrics.network.value;
  document.getElementById("network-sub").textContent = metrics.network.summary;

  document.getElementById("ai-score").textContent = aiCheckup.score;
  document.getElementById("ai-score-sub").textContent = aiCheckup.summary;

  const [issue1, issue2, issue3] = aiCheckup.issues;
  document.getElementById("ai-issue-1").textContent = issue1.label;
  document.getElementById("ai-issue-1-tag").textContent = issue1.tag;

  document.getElementById("ai-issue-2").textContent = issue2.label;
  document.getElementById("ai-issue-2-tag").textContent = issue2.tag;

  document.getElementById("ai-issue-3").textContent = issue3.label;
  document.getElementById("ai-issue-3-tag").textContent = issue3.tag;

  const monitorList = document.getElementById("monitor-list");
  const monitorUpdated = document.getElementById("monitor-updated");
  const monitorHistory = document.getElementById("monitor-history");
  const monitorRangeButtons = Array.from(document.querySelectorAll(".monitor-range-btn"));
  const processList = document.getElementById("process-list");
  const processCount = document.getElementById("process-count");
  const detailName = document.getElementById("detail-name");
  const detailPid = document.getElementById("detail-pid");
  const detailCpu = document.getElementById("detail-cpu");
  const detailMemory = document.getElementById("detail-memory");
  const detailPath = document.getElementById("detail-path");
  const detailStatus = document.getElementById("process-status");
  const terminateButton = document.getElementById("terminate-process");
  const priorityButton = document.getElementById("set-priority");
  const portList = document.getElementById("port-list");
  const portCount = document.getElementById("port-count");
  const diskList = document.getElementById("disk-list");
  const diskCount = document.getElementById("disk-count");
  const toolGrid = document.getElementById("tool-grid");
  const toolCount = document.getElementById("tool-count");
  const toolLog = document.getElementById("tool-log");
  const toolHint = document.getElementById("tool-hint");
  const viewActionLogsButton = document.getElementById("view-action-logs");
  const exportActionLogsButton = document.getElementById("export-action-logs");
  const aiModel = document.getElementById("ai-model");
  const aiProvider = document.getElementById("ai-provider");
  const aiCliList = document.getElementById("ai-cli-list");
  const aiCliLog = document.getElementById("ai-cli-log");
  const aiCliNew = document.getElementById("ai-cli-new");
  const aiCliConfig = document.getElementById("ai-cli-config");
  const modal = document.getElementById("confirm-modal");
  const modalTitle = document.getElementById("modal-title");
  const modalBody = document.getElementById("modal-body");
  const modalCancel = document.getElementById("modal-cancel");
  const modalConfirm = document.getElementById("modal-confirm");
  const tauriInvoke = window?.__TAURI__?.invoke;
  const monitorHistoryStore = [];
  const monitorRangeMinutes = {
    1: 60,
    5: 300,
    15: 900
  };
  let currentRange = 1;

  const openModal = ({ title, body, onConfirm }) => {
    modalTitle.textContent = title;
    modalBody.textContent = body;
    modal.classList.remove("hidden");

    const confirmHandler = () => {
      onConfirm();
      closeModal();
    };

    modalConfirm.addEventListener("click", confirmHandler, { once: true });
  };

  const closeModal = () => {
    modal.classList.add("hidden");
  };

  modalCancel.addEventListener("click", closeModal);

  const openPriorityPicker = ({ name, pid, current, options }) => {
    modalTitle.textContent = "选择优先级";
    modalBody.innerHTML = "";
    const desc = document.createElement("p");
    desc.textContent = `为 ${name}（PID ${pid}）设置优先级，当前：${current}`;
    modalBody.appendChild(desc);
    const list = document.createElement("div");
    list.className = "priority-options";
    const restore = () => {
      modalConfirm.style.display = "";
      modalBody.innerHTML = "";
    };
    options.forEach((opt) => {
      const btn = document.createElement("button");
      btn.className = opt === current ? "primary" : "ghost";
      btn.textContent = opt;
      btn.addEventListener("click", () => {
        closeModal();
        restore();
        setPriority(opt);
      });
      list.appendChild(btn);
    });
    modalBody.appendChild(list);
    modalConfirm.style.display = "none";
    modal.classList.remove("hidden");
    modalCancel.addEventListener("click", restore, { once: true });
  };

  const renderProcessDetail = (detail) => {
    detailName.textContent = detail.name;
    detailPid.textContent = detail.pid;
    detailCpu.textContent = detail.cpu;
    detailMemory.textContent = detail.memory;
    detailPath.textContent = detail.path;
    detailStatus.textContent = "等待操作…";
  };

  const renderProcessRows = (items) => {
    processList.innerHTML = "";
    processCount.textContent = items.length.toString();

    items.forEach((item) => {
      const row = document.createElement("li");
      row.title = `${item.name} · PID ${item.pid}`;

      const name = document.createElement("span");
      name.className = "list-name";
      name.textContent = item.name;

      const metric = document.createElement("span");
      metric.className = "list-metric";
      const cpuText = typeof item.cpu === "number" ? item.cpu.toFixed(0) : item.cpu;
      metric.textContent = `${cpuText}% · ${item.memory}`;

      row.addEventListener("click", () => fetchProcessDetail(item.pid));

      row.appendChild(name);
      row.appendChild(metric);
      processList.appendChild(row);
    });
  };

  const SYSTEM_PROC_NAMES = new Set([
    "System", "Idle", "System Idle", "svchost.exe", "services.exe",
    "lsass.exe", "wininit.exe", "winlogon.exe", "csrss.exe", "smss.exe",
    "dwm.exe", "fontdrvhost.exe", "spoolsv.exe", "dllhost.exe",
    "RuntimeBroker.exe", "SearchIndexer.exe", "MsMpEng.exe",
    "NisSrv.exe", "SecurityHealthService.exe", "WUDFHost.exe"
  ]);

  const COMMON_PORTS = new Set([
    20, 21, 22, 23, 25, 53, 80, 110, 143, 443, 465, 587, 993, 995,
    1433, 1521, 2375, 2376, 3000, 3306, 3389, 4200, 5000, 5173, 5432,
    5672, 5984, 6379, 6443, 7474, 8000, 8080, 8443, 8888, 9000, 9200,
    9300, 11211, 15672, 27017, 50000
  ]);

  const isSystemProcess = (name) => SYSTEM_PROC_NAMES.has(name) || name.startsWith("PID ");

  let portRawItems = [];
  const portFilterState = {
    query: "",
    category: "all",
    grouped: false
  };

  const applyPortFilters = (items) => {
    return items.filter((it) => {
      if (portFilterState.category === "user" && isSystemProcess(it.process)) return false;
      if (portFilterState.category === "system" && !isSystemProcess(it.process)) return false;
      if (portFilterState.category === "common" && !COMMON_PORTS.has(it.port)) return false;
      if (portFilterState.query) {
        const q = portFilterState.query.toLowerCase();
        const hay = `${it.port} ${it.process.toLowerCase()} ${it.pid} ${it.protocol.toLowerCase()}`;
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  };

  const buildPortRow = (item) => {
    const row = document.createElement("div");
    row.className = "port-item";
    if (isSystemProcess(item.process)) row.classList.add("is-system");
    row.title = `${item.process} · PID ${item.pid} · ${item.protocol}`;

    const info = document.createElement("div");
    const port = document.createElement("div");
    port.className = "port";
    port.textContent = `:${item.port} ${item.protocol}`;
    if (COMMON_PORTS.has(item.port)) {
      const tag = document.createElement("span");
      tag.className = "port-tag";
      tag.textContent = "常用";
      port.appendChild(tag);
    }
    const meta = document.createElement("div");
    meta.className = "port-sub";
    meta.textContent = `${item.process} · PID ${item.pid}`;
    info.appendChild(port);
    info.appendChild(meta);

    const btn = document.createElement("button");
    btn.className = "ghost";
    btn.textContent = "详情";

    row.addEventListener("click", () => fetchProcessDetail(item.pid));
    row.appendChild(info);
    row.appendChild(btn);
    return row;
  };

  const renderPortGrouped = (items) => {
    const groups = new Map();
    items.forEach((it) => {
      const key = `${it.process}|${it.pid}`;
      if (!groups.has(key)) groups.set(key, { process: it.process, pid: it.pid, ports: [] });
      groups.get(key).ports.push(it);
    });
    const sorted = [...groups.values()].sort((a, b) => b.ports.length - a.ports.length);
    sorted.forEach((g) => {
      const groupEl = document.createElement("div");
      groupEl.className = "port-group";
      if (isSystemProcess(g.process)) groupEl.classList.add("is-system");

      const header = document.createElement("div");
      header.className = "port-group-header";
      const left = document.createElement("div");
      left.innerHTML = `<strong>${g.process}</strong> <span class="port-group-count">${g.ports.length}</span>`;
      const right = document.createElement("div");
      right.className = "port-sub";
      right.textContent = `PID ${g.pid}`;
      header.appendChild(left);
      header.appendChild(right);

      const content = document.createElement("div");
      content.className = "port-group-content collapsed";
      g.ports.sort((a, b) => a.port - b.port).forEach((p) => {
        const chip = document.createElement("span");
        chip.className = "port-chip";
        if (COMMON_PORTS.has(p.port)) chip.classList.add("common");
        chip.textContent = `:${p.port} ${p.protocol}`;
        chip.addEventListener("click", () => fetchProcessDetail(p.pid));
        content.appendChild(chip);
      });
      header.addEventListener("click", () => content.classList.toggle("collapsed"));

      groupEl.appendChild(header);
      groupEl.appendChild(content);
      portList.appendChild(groupEl);
    });
  };

  const renderPortRows = (items) => {
    if (items !== portRawItems) portRawItems = items;
    const filtered = applyPortFilters(portRawItems);
    portList.innerHTML = "";
    portCount.textContent = `${filtered.length} / ${portRawItems.length}`;
    if (filtered.length === 0) {
      const empty = document.createElement("div");
      empty.className = "port-empty";
      empty.textContent = "无匹配端口";
      portList.appendChild(empty);
      return;
    }
    if (portFilterState.grouped) {
      renderPortGrouped(filtered);
    } else {
      filtered.forEach((item) => portList.appendChild(buildPortRow(item)));
    }
  };

  const renderDiskRows = (items) => {
    diskList.innerHTML = "";
    diskCount.textContent = items.length.toString();
    items.forEach((item) => {
      const row = document.createElement("div");
      row.className = "disk-item";
      const header = document.createElement("div");
      header.className = "disk-item-header";
      const label = document.createElement("span");
      label.className = "disk-name";
      const removableTag = item.removable ? " · 可移动" : "";
      label.textContent = `${item.mount_point} (${item.name}) · ${item.file_system}${removableTag}`;
      const pct = document.createElement("span");
      pct.className = "disk-pct";
      pct.textContent = `${Math.round(item.used_percent)}%`;
      header.appendChild(label);
      header.appendChild(pct);
      const bar = document.createElement("div");
      bar.className = "disk-bar";
      const fill = document.createElement("div");
      fill.className = "disk-bar-fill";
      fill.style.width = `${Math.min(100, Math.max(0, item.used_percent))}%`;
      if (item.used_percent >= 85) fill.classList.add("danger");
      else if (item.used_percent >= 70) fill.classList.add("warn");
      bar.appendChild(fill);
      const meta = document.createElement("div");
      meta.className = "disk-meta-row";
      meta.textContent = `可用 ${item.available} / 总计 ${item.total}`;
      row.appendChild(header);
      row.appendChild(bar);
      row.appendChild(meta);
      diskList.appendChild(row);
    });
  };

  const fetchDiskOverview = async () => {
    if (!tauriInvoke) {
      renderDiskRows(data.diskOverview);
      return;
    }
    const items = await tauriInvoke("get_disk_overview");
    renderDiskRows(items);
  };

  const renderToolCards = (items) => {
    toolGrid.innerHTML = "";
    toolCount.textContent = items.length.toString();

    items.forEach((item) => {
      const card = document.createElement("button");
      card.className = "tool";
      const shellLabel = item.shell ? item.shell.toUpperCase() : "CMD";
      const badge = item.requiresAdmin
        ? `<span class="tool-badge">需要管理员权限</span>`
        : `<span class="tool-badge tool-badge--ok">普通权限</span>`;
      card.innerHTML = `<strong>${item.name}</strong><br /><span>${item.description}</span><span class="tool-command">${shellLabel} · ${item.command}</span>${badge}`;
      card.addEventListener("click", () => {
        executeToolCommand(item);
      });
      toolGrid.appendChild(card);
    });
  };

  const updateToolLog = (message) => {
    const timestamp = new Date().toLocaleTimeString();
    toolLog.textContent = `[${timestamp}] ${message}`;
  };

  const renderAiCliSessions = (dataSource) => {
    aiModel.textContent = dataSource.model;
    aiProvider.textContent = dataSource.provider;
    aiCliList.innerHTML = "";

    dataSource.sessions.forEach((session, index) => {
      const row = document.createElement("div");
      row.className = "ai-cli-session";
      if (index === 0) {
        row.classList.add("active");
        aiCliLog.textContent = session.transcript.join("\n");
      }
      row.innerHTML = `<span>${session.title}</span><span>${session.updatedAt}</span>`;
      row.addEventListener("click", () => {
        document.querySelectorAll(".ai-cli-session").forEach((item) => {
          item.classList.remove("active");
        });
        row.classList.add("active");
        aiCliLog.textContent = session.transcript.join("\n");
      });
      aiCliList.appendChild(row);
    });
  };

  const renderMonitorRows = (overview, updatedAt) => {
    monitorList.innerHTML = "";

    overview.forEach((item) => {
      const row = document.createElement("div");
      row.className = "monitor-row";

      const label = document.createElement("div");
      label.className = "monitor-label";
      label.textContent = item.label;

      const bar = document.createElement("div");
      bar.className = "monitor-bar";

      const fill = document.createElement("div");
      fill.className = "monitor-bar-fill";
      fill.style.width = `${Math.round(item.value * 100)}%`;

      const value = document.createElement("div");
      value.className = "monitor-value";
      value.textContent = item.display;

      bar.appendChild(fill);
      row.appendChild(label);
      row.appendChild(bar);
      row.appendChild(value);
      monitorList.appendChild(row);
    });

    monitorUpdated.textContent = updatedAt;
  };

  const updateMonitorHistory = (overview, updatedAt) => {
    const snapshot = {
      time: updatedAt,
      cpu: overview.find((item) => item.label.includes("CPU"))?.display ?? "--",
      memory: overview.find((item) => item.label.includes("内存"))?.display ?? "--",
      disk: overview.find((item) => item.label.includes("磁盘"))?.display ?? "--",
      network: overview.find((item) => item.label.includes("网络"))?.display ?? "--"
    };
    monitorHistoryStore.push(snapshot);

    if (monitorHistoryStore.length > monitorRangeMinutes[15]) {
      monitorHistoryStore.splice(0, monitorHistoryStore.length - monitorRangeMinutes[15]);
    }
    renderMonitorHistory();
  };

  const renderMonitorHistory = () => {
    const windowSize = monitorRangeMinutes[currentRange];
    const visible = monitorHistoryStore.slice(-windowSize);
    if (visible.length === 0) {
      monitorHistory.textContent = "趋势：等待数据…";
      return;
    }
    const last = visible[visible.length - 1];
    monitorHistory.textContent = `趋势(${currentRange}分钟，${visible.length}个样本)：CPU ${last.cpu} · 内存 ${last.memory} · 磁盘 ${last.disk} · 网络 ${last.network}`;
  };

  const formatNow = () => {
    const now = new Date();
    return `${now.getHours().toString().padStart(2, "0")}:${now
      .getMinutes()
      .toString()
      .padStart(2, "0")}:${now.getSeconds().toString().padStart(2, "0")}`;
  };

  const randomizeMonitor = () => {
    data.monitorOverview = data.monitorOverview.map((item) => {
      const delta = (Math.random() - 0.5) * 0.06;
      const nextValue = Math.min(0.95, Math.max(0.05, item.value + delta));
      return {
        ...item,
        value: Number(nextValue.toFixed(2)),
        display: `${Math.round(nextValue * 100)}%`
      };
    });
  };

  const fetchMonitorSnapshot = async () => {
    if (!tauriInvoke) {
      randomizeMonitor();
      const currentTime = formatNow();
      renderMonitorRows(data.monitorOverview, currentTime);
      updateMonitorHistory(data.monitorOverview, currentTime);
      return;
    }

    const snapshot = await tauriInvoke("get_monitor_snapshot");
    renderMonitorRows(snapshot.overview, snapshot.updated_at);
    updateMonitorHistory(snapshot.overview, snapshot.updated_at);
  };

  fetchMonitorSnapshot();
  setInterval(fetchMonitorSnapshot, 1000);

  const fetchProcessOverview = async () => {
    if (!tauriInvoke) {
      renderProcessRows(data.processOverview);
      renderProcessDetail(data.processDetail);
      return;
    }

    const processes = await tauriInvoke("get_process_overview");
    renderProcessRows(processes);
  };

  const fetchProcessDetail = async (pid) => {
    if (!tauriInvoke) {
      renderProcessDetail(data.processDetail);
      return;
    }

    const detail = await tauriInvoke("get_process_detail", { pid });
    renderProcessDetail(detail);
  };

  const fetchPortOverview = async () => {
    if (!tauriInvoke) {
      renderPortRows(data.portOverview);
      return;
    }

    const result = await tauriInvoke("get_port_overview");
    if (result.error) {
      portCount.textContent = "0";
      portList.innerHTML = `<div class="port-error">${result.error}</div>`;
      return;
    }
    renderPortRows(result.items);
  };

  const fetchToolbox = async () => {
    if (!tauriInvoke) {
      renderToolCards(data.toolbox);
      toolHint.textContent = "提示：当前为静态模式，命令不会实际执行。";
      return;
    }

    const tools = await tauriInvoke("get_toolbox_items");
    renderToolCards(tools);
    toolHint.textContent = "提示：请谨慎执行系统命令。";
  };

  const executeToolCommand = async (tool) => {
    openModal({
      title: "执行命令确认",
      body: `${tool.name} 将执行命令：${tool.command}`,
      onConfirm: async () => {
        if (!tauriInvoke) {
          updateToolLog(`【模拟执行】${tool.command}\n${tool.description}`);
          return;
        }

        const result = await tauriInvoke("run_toolbox_command", {
          id: tool.id
        });
        updateToolLog(result.message);
      }
    });
  };

  const refreshActionLogs = async () => {
    if (!tauriInvoke) {
      updateToolLog("静态模式下无后端操作日志。");
      return;
    }
    const logs = await tauriInvoke("get_action_logs");
    if (!logs.length) {
      updateToolLog("暂无操作日志。");
      return;
    }
    const latest = logs.slice(-5).map((item) => {
      const status = item.success ? "成功" : "失败";
      return `[${item.timestamp}] ${item.module}/${item.action}(${item.target}) ${status} - ${item.message}`;
    });
    toolLog.textContent = latest.join("\n");
  };

  const exportActionLogs = async () => {
    if (!tauriInvoke) {
      updateToolLog("静态模式下不支持日志导出。");
      return;
    }
    const result = await tauriInvoke("export_action_logs", { format: "json" });
    updateToolLog(result.message);
  };

  const terminateProcess = async () => {
    const pid = Number(detailPid.textContent);
    if (!tauriInvoke) {
      detailStatus.textContent = `已触发结束进程（模拟），PID: ${pid}`;
      return;
    }

    const result = await tauriInvoke("terminate_process", { pid });
    detailStatus.textContent = result.message;
  };

  const setPriority = async (level) => {
    const pid = Number(detailPid.textContent);
    if (!Number.isFinite(pid) || pid <= 0) {
      detailStatus.textContent = "请先选择进程。";
      return;
    }
    if (!tauriInvoke) {
      data.processActions.priority.current = level;
      detailStatus.textContent = `已模拟设置优先级为 ${level}，PID: ${pid}`;
      return;
    }
    const result = await tauriInvoke("set_process_priority", { pid, level });
    if (result.success) {
      data.processActions.priority.current = level;
    }
    detailStatus.textContent = result.message;
  };

  fetchProcessOverview();
  fetchProcessDetail(data.processDetail.pid);
  fetchPortOverview();
  fetchDiskOverview();
  fetchToolbox();
  renderAiCliSessions(data.aiCli);

  terminateButton.addEventListener("click", () => {
    openModal({
      title: "结束进程确认",
      body: `将结束进程 ${detailName.textContent}（PID ${detailPid.textContent}）。该操作可能导致数据丢失。`,
      onConfirm: terminateProcess
    });
  });

  priorityButton.addEventListener("click", () => {
    const pid = Number(detailPid.textContent);
    if (!Number.isFinite(pid) || pid <= 0) {
      detailStatus.textContent = "请先选择进程。";
      return;
    }
    openPriorityPicker({
      name: detailName.textContent,
      pid,
      current: data.processActions.priority.current,
      options: data.processActions.priority.options
    });
  });

  aiCliNew.addEventListener("click", () => {
    updateToolLog("AI CLI：新建会话功能待实现。");
  });

  aiCliConfig.addEventListener("click", () => {
    updateToolLog("AI CLI：模型配置入口待实现。");
  });

  monitorRangeButtons.forEach((button) => {
    button.addEventListener("click", () => {
      currentRange = Number(button.dataset.range || "1");
      monitorRangeButtons.forEach((item) => item.classList.remove("active"));
      button.classList.add("active");
      renderMonitorHistory();
    });
  });

  document.getElementById("refresh-ports").addEventListener("click", fetchPortOverview);
  document.getElementById("refresh-disks").addEventListener("click", fetchDiskOverview);

  const portSearchInput = document.getElementById("port-search");
  portSearchInput.addEventListener("input", () => {
    portFilterState.query = portSearchInput.value.trim();
    renderPortRows(portRawItems);
  });

  const portChips = Array.from(document.querySelectorAll("[data-port-filter]"));
  portChips.forEach((chip) => {
    chip.addEventListener("click", () => {
      portChips.forEach((c) => c.classList.remove("active"));
      chip.classList.add("active");
      portFilterState.category = chip.dataset.portFilter;
      renderPortRows(portRawItems);
    });
  });

  const portGroupToggle = document.getElementById("port-group-toggle");
  portGroupToggle.addEventListener("change", () => {
    portFilterState.grouped = portGroupToggle.checked;
    renderPortRows(portRawItems);
  });

  viewActionLogsButton.addEventListener("click", refreshActionLogs);
  exportActionLogsButton.addEventListener("click", exportActionLogs);

  const navTargetToPanelId = {
    overview: "panel-overview",
    monitor: "panel-monitor",
    process: "panel-process",
    network: "panel-network",
    disk: "panel-disk",
    toolbox: "panel-toolbox",
    ai: "ai-cli-panel"
  };

  const scrollToPanelId = (panelId) => {
    const target = document.getElementById(panelId);
    if (target) {
      target.scrollIntoView({ behavior: "smooth", block: "start" });
      target.classList.add("panel-flash");
      setTimeout(() => target.classList.remove("panel-flash"), 900);
    }
  };

  const navItems = Array.from(document.querySelectorAll("#sidebar-nav .nav-item"));
  navItems.forEach((item) => {
    item.addEventListener("click", () => {
      navItems.forEach((n) => n.classList.remove("active"));
      item.classList.add("active");
      const targetKey = item.dataset.target;
      const panelId = navTargetToPanelId[targetKey];
      if (panelId) {
        scrollToPanelId(panelId);
      }
      if (targetKey === "disk") {
        fetchDiskOverview();
      }
    });
  });

  document.getElementById("topbar-ai").addEventListener("click", () => {
    scrollToPanelId("ai-cli-panel");
  });

  document.getElementById("start-checkup").addEventListener("click", () => {
    const scoreEl = document.getElementById("ai-score");
    const subEl = document.getElementById("ai-score-sub");
    scoreEl.textContent = "检测中…";
    subEl.textContent = "正在采集系统指标";
    setTimeout(() => {
      const score = 70 + Math.floor(Math.random() * 25);
      scoreEl.textContent = String(score);
      subEl.textContent = score >= 85 ? "系统运行稳定 · 风险较低" : "存在优化空间 · 查看建议";
    }, 600);
  });

  document.querySelectorAll('[data-view-all="process"]').forEach((btn) => {
    btn.addEventListener("click", () => scrollToPanelId("panel-process"));
  });

  document.getElementById("manage-scripts").addEventListener("click", () => {
    scrollToPanelId("panel-toolbox");
    updateToolLog("脚本管理入口待实现：当前支持预置命令卡片。");
  });

  const openAiScriptPreview = (recipe) => {
    modalTitle.textContent = `AI 脚本：${recipe.name}`;
    modalBody.innerHTML = "";
    const desc = document.createElement("p");
    desc.textContent = recipe.description;
    modalBody.appendChild(desc);

    const riskLabel = { low: "低", medium: "中", high: "高" }[recipe.risk] || recipe.risk;
    const riskLine = document.createElement("div");
    riskLine.className = `ai-script-risk risk-${recipe.risk}`;
    riskLine.textContent = `风险等级：${riskLabel}${recipe.requires_admin ? " · 需要管理员权限" : ""}`;
    modalBody.appendChild(riskLine);

    const stepsWrap = document.createElement("div");
    stepsWrap.className = "ai-script-steps";
    recipe.steps.forEach((step, idx) => {
      const stepEl = document.createElement("div");
      stepEl.className = "ai-script-step";
      const title = document.createElement("div");
      title.className = "ai-script-step-title";
      title.textContent = `步骤 ${idx + 1}：${step.rationale}`;
      const cmd = document.createElement("pre");
      cmd.className = "ai-script-step-cmd";
      cmd.textContent = `[${step.shell.toUpperCase()}] ${step.command}`;
      stepEl.appendChild(title);
      stepEl.appendChild(cmd);
      stepsWrap.appendChild(stepEl);
    });
    modalBody.appendChild(stepsWrap);

    modalConfirm.textContent = "执行脚本";
    modal.classList.remove("hidden");

    const restore = () => {
      modalConfirm.textContent = "确认";
      modalBody.innerHTML = "";
    };
    const runIt = async () => {
      closeModal();
      restore();
      updateToolLog(`正在执行 AI 脚本：${recipe.name}…`);
      if (!tauriInvoke) {
        updateToolLog(`【模拟执行】${recipe.name}\n${recipe.steps.map((s) => s.command).join("\n")}`);
        return;
      }
      const result = await tauriInvoke("run_ai_script", { id: recipe.id });
      updateToolLog(result.message);
    };
    modalConfirm.addEventListener("click", runIt, { once: true });
    modalCancel.addEventListener("click", restore, { once: true });
  };

  document.getElementById("generate-optimize-script").addEventListener("click", async () => {
    scrollToPanelId("panel-toolbox");
    if (!tauriInvoke) {
      openAiScriptPreview({
        id: "daily-healthcheck",
        name: "日常健康检查（模拟）",
        description: "当前为静态模式，展示脚本预览但不会执行真实命令。",
        risk: "low",
        requires_admin: false,
        steps: [
          { rationale: "刷新 DNS 缓存", command: "ipconfig /flushdns", shell: "cmd" },
          { rationale: "查看监听端口数", command: "netstat -ano", shell: "cmd" }
        ]
      });
      return;
    }
    updateToolLog("AI：正在根据当前系统状态生成脚本…");
    const recipe = await tauriInvoke("generate_ai_script");
    openAiScriptPreview(recipe);
  });

  document.getElementById("open-toolbox").addEventListener("click", () => {
    scrollToPanelId("panel-toolbox");
  });

  const globalSearch = document.getElementById("global-search");
  globalSearch.addEventListener("input", () => {
    const q = globalSearch.value.trim().toLowerCase();
    Array.from(processList.children).forEach((row) => {
      const name = row.querySelector(".list-name")?.textContent?.toLowerCase() ?? "";
      row.style.display = !q || name.includes(q) ? "" : "none";
    });
    Array.from(portList.children).forEach((row) => {
      const text = row.textContent?.toLowerCase() ?? "";
      row.style.display = !q || text.includes(q) ? "" : "none";
    });
  });
}

const banner = document.createElement("div");
banner.className = "dev-banner";
banner.textContent = "Win-Top UI Skeleton (Tauri MVP)";
document.body.appendChild(banner);
