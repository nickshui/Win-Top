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
  const processList = document.getElementById("process-list");
  const processCount = document.getElementById("process-count");
  const detailName = document.getElementById("detail-name");
  const detailPid = document.getElementById("detail-pid");
  const detailCpu = document.getElementById("detail-cpu");
  const detailMemory = document.getElementById("detail-memory");
  const detailPath = document.getElementById("detail-path");
  const terminateButton = document.getElementById("terminate-process");
  const priorityButton = document.getElementById("set-priority");
  const modal = document.getElementById("confirm-modal");
  const modalTitle = document.getElementById("modal-title");
  const modalBody = document.getElementById("modal-body");
  const modalCancel = document.getElementById("modal-cancel");
  const modalConfirm = document.getElementById("modal-confirm");
  const tauriInvoke = window?.__TAURI__?.invoke;

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
      renderMonitorRows(data.monitorOverview, formatNow());
      return;
    }

    const snapshot = await tauriInvoke("get_monitor_snapshot");
    renderMonitorRows(snapshot.overview, snapshot.updated_at);
  };

  fetchMonitorSnapshot();
  setInterval(fetchMonitorSnapshot, 3000);

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

  const terminateProcess = async () => {
    const pid = Number(detailPid.textContent);
    if (!tauriInvoke) {
      alert(`已触发结束进程（模拟），PID: ${pid}`);
      return;
    }

    const result = await tauriInvoke("terminate_process", { pid });
    alert(result ? "进程已结束" : "进程结束失败");
  };

  const setPriority = async () => {
    const pid = Number(detailPid.textContent);
    const current = data.processActions.priority.current;
    const next = data.processActions.priority.options.find((option) => option !== current);
    if (!tauriInvoke) {
      alert(`已模拟设置优先级为 ${next}，PID: ${pid}`);
      return;
    }

    const result = await tauriInvoke("set_process_priority", { pid, level: next });
    alert(result ? `优先级已设置为 ${next}` : "设置优先级失败");
  };

  fetchProcessOverview();
  fetchProcessDetail(data.processDetail.pid);

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
}

const banner = document.createElement("div");
banner.className = "dev-banner";
banner.textContent = "Win-Top UI Skeleton (Tauri MVP)";
document.body.appendChild(banner);
