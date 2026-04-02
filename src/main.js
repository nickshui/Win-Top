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

      const name = document.createElement("span");
      name.className = "list-name";
      name.textContent = item.name;

      const metric = document.createElement("span");
      metric.className = "list-metric";
      metric.textContent = `CPU ${item.cpu}% · 内存 ${item.memory}`;

      const button = document.createElement("button");
      button.className = "ghost";
      button.textContent = "查看详情";
      button.addEventListener("click", () => {
        fetchProcessDetail(item.pid);
      });

      row.appendChild(name);
      row.appendChild(metric);
      row.appendChild(button);
      processList.appendChild(row);
    });
  };

  const renderPortRows = (items) => {
    portList.innerHTML = "";
    portCount.textContent = items.length.toString();

    items.forEach((item) => {
      const row = document.createElement("div");
      row.className = "port-item";

      const info = document.createElement("div");
      const port = document.createElement("div");
      port.className = "port";
      port.textContent = `:${item.port}`;

      const meta = document.createElement("div");
      meta.className = "port-sub";
      meta.textContent = `${item.process} · PID ${item.pid} · ${item.protocol}`;

      info.appendChild(port);
      info.appendChild(meta);

      const button = document.createElement("button");
      button.className = "ghost";
      button.textContent = "查看进程";

      row.appendChild(info);
      row.appendChild(button);
      portList.appendChild(row);
    });
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

    const ports = await tauriInvoke("get_port_overview");
    renderPortRows(ports);
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

  const setPriority = async () => {
    const pid = Number(detailPid.textContent);
    const current = data.processActions.priority.current;
    const next = data.processActions.priority.options.find((option) => option !== current);
    if (!tauriInvoke) {
      detailStatus.textContent = `已模拟设置优先级为 ${next}，PID: ${pid}`;
      return;
    }

    const result = await tauriInvoke("set_process_priority", { pid, level: next });
    detailStatus.textContent = result.message;
  };

  fetchProcessOverview();
  fetchProcessDetail(data.processDetail.pid);
  fetchPortOverview();
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
    openModal({
      title: "设置优先级确认",
      body: `将调整进程 ${detailName.textContent}（PID ${detailPid.textContent}）的优先级。`,
      onConfirm: setPriority
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

  viewActionLogsButton.addEventListener("click", refreshActionLogs);
  exportActionLogsButton.addEventListener("click", exportActionLogs);
}

const banner = document.createElement("div");
banner.className = "dev-banner";
banner.textContent = "Win-Top UI Skeleton (Tauri MVP)";
document.body.appendChild(banner);
