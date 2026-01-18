window.dashboardData = {
  metrics: {
    cpu: {
      value: "28%",
      summary: "峰值 42% · 过去 15 分钟"
    },
    memory: {
      value: "62%",
      summary: "12.4 GB / 20 GB"
    },
    network: {
      value: "8.4 MB/s",
      summary: "上行 1.2 · 下行 7.2"
    }
  },
  aiCheckup: {
    score: "86",
    summary: "系统运行稳定 · 风险较低",
    issues: [
      { label: "内存占用偏高", tag: "建议优化" },
      { label: "磁盘健康良好", tag: "正常" },
      { label: "可疑端口连接", tag: "需关注" }
    ]
  },
  monitorOverview: [
    { label: "CPU 负载", value: 0.28, display: "28%" },
    { label: "内存压力", value: 0.62, display: "62%" },
    { label: "磁盘活跃度", value: 0.44, display: "44%" },
    { label: "网络占用", value: 0.35, display: "35%" }
  ],
  processOverview: [
    { pid: 2316, name: "Chrome", cpu: 14, memory: "1.6 GB" },
    { pid: 412, name: "Visual Studio Code", cpu: 9, memory: "980 MB" },
    { pid: 904, name: "Docker Desktop", cpu: 5, memory: "1.2 GB" }
  ],
  processDetail: {
    pid: 2316,
    name: "Chrome",
    cpu: "14%",
    memory: "1.6 GB",
    path: "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe"
  },
  processActions: {
    priority: {
      current: "普通",
      options: ["低", "普通", "高", "实时"]
    }
  },
  portOverview: [
    { port: 3000, protocol: "TCP", process: "Node", pid: 2316 },
    { port: 5432, protocol: "TCP", process: "PostgreSQL", pid: 412 },
    { port: 6379, protocol: "TCP", process: "Redis", pid: 902 }
  ],
  toolbox: [
    {
      id: "net-diagnose",
      name: "网络诊断",
      description: "执行基础网络诊断与修复命令。",
      command: "ipconfig /flushdns",
      requiresAdmin: true,
      shell: "cmd"
    },
    {
      id: "disk-clean",
      name: "磁盘清理",
      description: "清理临时文件并释放空间。",
      command: "cleanmgr",
      requiresAdmin: false,
      shell: "cmd"
    },
    {
      id: "system-repair",
      name: "系统修复",
      description: "扫描并修复系统文件。",
      command: "sfc /scannow",
      requiresAdmin: true,
      shell: "cmd"
    },
    {
      id: "free-port",
      name: "释放端口",
      description: "查找并释放占用端口的进程。",
      command: "netstat -ano",
      requiresAdmin: false,
      shell: "powershell"
    }
  ]
};
