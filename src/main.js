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
  const tauriInvoke = window?.__TAURI__?.invoke;

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

  fetchProcessOverview();
  fetchProcessDetail(data.processDetail.pid);
}

const banner = document.createElement("div");
banner.className = "dev-banner";
banner.textContent = "Win-Top UI Skeleton (Tauri MVP)";
document.body.appendChild(banner);
