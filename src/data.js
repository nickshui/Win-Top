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
  ]
};
