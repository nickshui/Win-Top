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
  data.monitorOverview.forEach((item) => {
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
}

const banner = document.createElement("div");
banner.className = "dev-banner";
banner.textContent = "Win-Top UI Skeleton (Tauri MVP)";
document.body.appendChild(banner);
