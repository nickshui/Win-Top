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
  const processDetailName = document.getElementById("process-detail-name");
  const processDetailPid = document.getElementById("process-detail-pid");
  const processDetailCpu = document.getElementById("process-detail-cpu");
  const processDetailMemory = document.getElementById("process-detail-memory");
  const processDetailStatus = document.getElementById("process-detail-status");
  const processDetailExe = document.getElementById("process-detail-exe");
  const tauriInvoke = window?.__TAURI__?.invoke;

  const renderProcessDetail = (detail) => {
    if (!detail) {
      processDetailName.textContent = "--";
      processDetailPid.textContent = "--";
      processDetailCpu.textContent = "--";
      processDetailMemory.textContent = "--";
      processDetailStatus.textContent = "--";
      processDetailExe.textContent = "--";
      return;
    }

    processDetailName.textContent = detail.name;
    processDetailPid.textContent = detail.pid;
    processDetailCpu.textContent = detail.cpu;
    processDetailMemory.textContent = detail.memory;
    processDetailStatus.textContent = detail.status;
    processDetailExe.textContent = detail.exe || "--";
  };

  const renderProcessRows = (items) => {
    processList.innerHTML = "";
    processCount.textContent = items.length.toString();

    items.forEach((item) => {
      const row = document.createElement("li");
      row.className = "process-row";
      row.dataset.pid = item.pid;

      const name = document.createElement("span");
      name.className = "list-name";
      name.textContent = item.name;

      const metric = document.createElement("span");
      metric.className = "list-metric";
      metric.textContent = `CPU ${item.cpu}% · 内存 ${item.memory}`;

      row.appendChild(name);
      row.appendChild(metric);
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

  const fetchProcessDetail = async (pid) => {
    if (!pid) {
      renderProcessDetail(null);
      return;
    }

    if (!tauriInvoke) {
      renderProcessDetail(data.processDetails[pid]);
      return;
    }

    const detail = await tauriInvoke("get_process_detail", { pid: Number(pid) });
    renderProcessDetail(detail);
  };

  const fetchProcessOverview = async () => {
    if (!tauriInvoke) {
      renderProcessRows(data.processOverview);
      fetchProcessDetail(data.processOverview[0]?.pid);
      return;
    }

    const processes = await tauriInvoke("get_process_overview");
    renderProcessRows(processes);
    fetchProcessDetail(processes[0]?.pid);
  };

  fetchProcessOverview();

  processList.addEventListener("click", (event) => {
    const target = event.target.closest(".process-row");
    if (!target) {
      return;
    }

    fetchProcessDetail(target.dataset.pid);
  });
}

const banner = document.createElement("div");
banner.className = "dev-banner";
banner.textContent = "Win-Top UI Skeleton (Tauri MVP)";
document.body.appendChild(banner);
