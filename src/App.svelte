<script>
  import { onMount, onDestroy } from "svelte";
  import { startMetrics, startEvents, startNetTraffic, startCleanupNotifications, startDiskIo } from "./lib/stores.js";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import TopBar from "./lib/components/TopBar.svelte";
  import Overview from "./lib/views/Overview.svelte";
  import Processes from "./lib/views/Processes.svelte";
  import Network from "./lib/views/Network.svelte";
  import Disk from "./lib/views/Disk.svelte";
  import Events from "./lib/views/Events.svelte";
  import About from "./lib/views/About.svelte";
  import Placeholder from "./lib/views/Placeholder.svelte";
  import Optimize from "./lib/views/Optimize.svelte";
  import Tools from "./lib/views/Tools.svelte";
  import Toast from "./lib/components/Toast.svelte";

  let current = "overview";
  let stopMetrics;
  let stopEvents;
  let stopNetTraffic;
  let stopCleanupNotif;
  let stopDiskIo;

  onMount(() => {
    stopMetrics = startMetrics();
    stopEvents = startEvents();
    stopNetTraffic = startNetTraffic();
    stopCleanupNotif = startCleanupNotifications();
    stopDiskIo = startDiskIo();
  });
  onDestroy(() => {
    if (stopMetrics) stopMetrics();
    if (stopEvents) stopEvents();
    if (stopNetTraffic) stopNetTraffic();
    if (stopCleanupNotif) stopCleanupNotif();
    if (stopDiskIo) stopDiskIo();
  });

  const meta = {
    overview: { title: "概览", icon: "gauge" },
    process: {
      title: "进程管理",
      icon: "cpu",
      plan: "NtQuerySystemInformation 全量进程 + 虚拟滚动表格，按 CPU/内存排序，结束/优先级用 WinAPI 直调。",
    },
    events: { title: "实时事件", icon: "activity" },
    network: {
      title: "网络与端口",
      icon: "network",
      plan: "GetExtendedTcpTable 结构化端口→PID→进程，连接状态分类，ETW 实时连接事件。",
    },
    disk: {
      title: "磁盘管理",
      icon: "harddrive",
      plan: "GetDiskFreeSpaceEx 容量 + WMI SMART 健康/温度/磨损，分区使用率可视化。",
    },
    toolbox: {
      title: "优化加速",
      icon: "terminal",
      plan: "命令卡片参数化，长任务异步流式输出，需管理员的操作走按需提权。",
    },
    tools: { title: "系统工具", icon: "wrench" },
    ai: {
      title: "AI 助手",
      icon: "sparkles",
      plan: "基于真实系统快照的状态解释与脚本生成，执行前可视化确认与风险提示。",
    },
    about: { title: "关于", icon: "info" },
  };
</script>

<div class="shell">
  <Sidebar {current} onSelect={(id) => (current = id)} />
  <div class="main">
    <TopBar title={meta[current].title} />
    <div class="view">
      {#if current === "overview"}
        <Overview />
      {:else if current === "process"}
        <Processes />
      {:else if current === "events"}
        <Events />
      {:else if current === "network"}
        <Network />
      {:else if current === "disk"}
        <Disk />
      {:else if current === "toolbox"}
        <Optimize navigate={(id) => (current = id)} />
      {:else if current === "tools"}
        <Tools />
      {:else if current === "about"}
        <About />
      {:else}
        <Placeholder
          icon={meta[current].icon}
          title={meta[current].title}
          plan={meta[current].plan}
        />
      {/if}
    </div>
  </div>
</div>

<Toast />

<style>
  .shell {
    display: flex;
    height: 100vh;
    overflow: hidden;
  }
  .main {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .view {
    flex: 1;
    overflow-y: auto;
    padding: var(--sp-6);
  }
</style>
