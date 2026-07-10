<script>
  import { onMount, onDestroy, tick } from "svelte";
  import { pushToast, collectSystemSnapshot } from "../stores.js";
  import { streamChat } from "../aiClient.js";
  import ChatMessage from "../components/ChatMessage.svelte";

  // ===== 内置厂商与模型预设 =====
  // 各厂商的 OpenAI 兼容接口地址与主流最新模型。可随时在下拉里选，也可自定义。
  const PROVIDERS = [
    {
      id: "openai",
      label: "OpenAI",
      baseUrl: "https://api.openai.com/v1",
      keyHint: "sk-...",
      models: ["gpt-5.6", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna", "gpt-5", "gpt-5-mini", "gpt-4.1", "gpt-4.1-mini", "o4-mini"],
    },
    {
      id: "deepseek",
      label: "DeepSeek",
      baseUrl: "https://api.deepseek.com/v1",
      keyHint: "sk-...",
      models: ["deepseek-v4-flash", "deepseek-v4-pro", "deepseek-chat", "deepseek-reasoner"],
    },
    {
      id: "moonshot",
      label: "月之暗面 Kimi",
      baseUrl: "https://api.moonshot.cn/v1",
      keyHint: "sk-...",
      models: ["kimi-k2.7-code", "kimi-k2.7-code-highspeed", "kimi-k2.6", "kimi-k2.5", "moonshot-v1-128k", "moonshot-v1-32k", "moonshot-v1-8k"],
    },
    {
      id: "dashscope",
      label: "阿里通义千问",
      baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
      keyHint: "sk-...",
      models: ["qwen3-max", "qwen-max", "qwen-plus", "qwen-flash", "qwen-turbo", "qwen2.5-72b-instruct"],
    },
    {
      id: "zhipu",
      label: "智谱 GLM",
      baseUrl: "https://open.bigmodel.cn/api/paas/v4",
      keyHint: "xxx.xxx",
      models: ["glm-4.6", "glm-4.5", "glm-4.5-air", "glm-4-plus", "glm-4-flash"],
    },
    {
      id: "siliconflow",
      label: "硅基流动",
      baseUrl: "https://api.siliconflow.cn/v1",
      keyHint: "sk-...",
      models: ["deepseek-ai/DeepSeek-V3.1", "deepseek-ai/DeepSeek-R1", "Qwen/Qwen3-235B-A22B", "moonshotai/Kimi-K2-Instruct"],
    },
    {
      id: "openrouter",
      label: "OpenRouter",
      baseUrl: "https://openrouter.ai/api/v1",
      keyHint: "sk-or-...",
      models: ["openai/gpt-5.6", "anthropic/claude-sonnet-4.5", "google/gemini-2.5-flash", "deepseek/deepseek-chat-v3.1", "x-ai/grok-4"],
    },
    {
      id: "ollama",
      label: "本地 Ollama",
      baseUrl: "http://localhost:11434/v1",
      keyHint: "任意值（本地可留 ollama）",
      models: ["llama3.3", "qwen3", "deepseek-r1", "gemma3"],
    },
    {
      id: "custom",
      label: "自定义厂商",
      baseUrl: "",
      keyHint: "按厂商要求填写",
      models: [],
    },
  ];

  // ===== 配置（持久化到 localStorage）=====
  const LS_KEY = "wintop.ai.config";
  let cfg = {
    providerId: "openai",
    baseUrl: "https://api.openai.com/v1",
    apiKey: "",
    model: "gpt-5.6",
  };
  let showConfig = false;
  // 模型下拉里的“自定义模型”开关
  let customModel = false;

  $: activeProvider = PROVIDERS.find((p) => p.id === cfg.providerId) ?? PROVIDERS[0];
  // 当前厂商预设模型是否包含已选模型；不包含则视为自定义模型
  $: if (activeProvider && activeProvider.id !== "custom") {
    if (!customModel && activeProvider.models.length && !activeProvider.models.includes(cfg.model)) {
      customModel = true;
    }
  }

  // 切换厂商：自动填充其 baseUrl 与首个模型（自定义厂商则清空由用户填）
  function selectProvider(id) {
    const p = PROVIDERS.find((x) => x.id === id);
    if (!p) return;
    cfg.providerId = id;
    if (p.id === "custom") {
      customModel = true;
    } else {
      cfg.baseUrl = p.baseUrl;
      customModel = false;
      if (p.models.length && !p.models.includes(cfg.model)) cfg.model = p.models[0];
    }
    cfg = cfg;
  }

  function onModelSelect(e) {
    const v = e.target.value;
    if (v === "__custom__") {
      customModel = true;
      cfg.model = "";
    } else {
      customModel = false;
      cfg.model = v;
    }
    cfg = cfg;
  }

  function loadConfig() {
    try {
      const raw = localStorage.getItem(LS_KEY);
      if (raw) cfg = { ...cfg, ...JSON.parse(raw) };
    } catch {}
  }
  function saveConfig() {
    if (!cfg.baseUrl || !cfg.apiKey || !cfg.model) return;
    try {
      localStorage.setItem(LS_KEY, JSON.stringify(cfg));
      pushToast("配置已保存", "ok");
      showConfig = false;
    } catch (e) {
      pushToast("保存失败：" + e, "error");
    }
  }

  $: configured = !!cfg.apiKey && !!cfg.baseUrl && !!cfg.model;

  // ===== 对话状态 =====
  // 消息存原始文本（assistant 为 Markdown 源码），渲染交给 ChatMessage；
  // 发送历史给 API 时直接用 text，不做 HTML 剥离的有损转换。
  let messages = []; // { id, role: 'user'|'assistant', text }
  let msgSeq = 0;
  let input = "";
  let sending = false;
  let streamId = null; // 正在流式生成的 assistant 消息 id
  let client = null;   // streamChat 句柄
  let scroller;

  // ===== 滚动：贴底跟随 =====
  // 用户停留在底部附近时才自动跟随新内容；上翻阅读时不打扰。
  let autoFollow = true;
  function onScroll() {
    const el = scroller;
    if (el) autoFollow = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
  }
  // 关键：必须通过局部别名 el 赋值 scrollTop，绝不能写 `scroller.scrollTop = ...`——
  // 对组件级变量 scroller 的成员赋值会被 Svelte 编译成 $$invalidate(scroller)，
  // 一旦有响应式语句依赖 scroller 就会无限自触发冻死主线程（踩过的坑）。
  // 因此这里也不用任何 `$:` 响应式块做滚动，全部显式调用。
  function scrollToBottom(force = false) {
    tick().then(() => {
      const el = scroller;
      if (el && (force || autoFollow)) el.scrollTop = el.scrollHeight;
    });
  }

  const quickPrompts = [
    "分析当前系统整体健康状况，指出瓶颈",
    "哪些进程占用资源异常？建议如何处理",
    "我的磁盘空间该如何清理？",
    "内存占用偏高，有什么优化建议？",
  ];

  const SYSTEM_PROMPT =
    "你是 Win-Top 系统诊断助手，内嵌运行在名为「Win-Top」的 Windows 系统监控与优化软件中。" +
    "你会收到该电脑的真实系统快照（CPU/内存/磁盘/进程等）。请基于这些真实数据给出准确、可操作的中文建议。\n\n" +
    "【核心原则】优先引导用户使用 Win-Top 软件内已有的图形化功能来解决问题，而不是让用户去命令行敲 netstat/tasklist/taskkill 等命令。" +
    "只有当软件确实没有对应能力时，才退而给出命令行方案。引导时请明确指出：进入哪个左侧菜单 → 用哪个功能 → 怎么操作。\n\n" +
    "【Win-Top 已内置的功能清单（请据此引导）】\n" +
    "- 「网络与端口」→ 端口连接：在搜索框输入端口号（如 5050）即可筛出占用该端口的进程，点该行右侧「结束」按钮即可一键关闭对应进程（会自动分析同端口 v4/v6 绑定，避免误杀）。该页还有：一键网络体检、下行测速、输入「域名/IP:端口」检测连通性。\n" +
    "- 「进程管理」→ 支持列表/树形视图、可调刷新间隔(1s/2s/5s/暂停)、按 CPU/内存/磁盘排序、结束进程、设置优先级、查看进程详情（命令行/路径/句柄等）。\n" +
    "- 「磁盘管理」→ 分区使用率、物理磁盘 SMART 健康/温度；「空间分析」标签可对整盘做 MFT 极速扫描，列出最大目录/文件，支持目录逐级下钻、在资源管理器中定位。\n" +
    "- 「优化加速」→ 一键加速（垃圾清理+内存释放）、单独的垃圾清理、内存释放、启动项管理（禁用开机自启）、系统体检评分。\n" +
    "- 「系统工具」→ 防火墙规则开关、创建系统还原点、文件解锁（查找并结束占用某文件的进程）、导出系统快照(JSON/CSV)。\n" +
    "- 「概览」→ 实时 CPU/内存/磁盘/网络指标与可交互历史趋势图；「实时事件」→ ETW 进程启动/退出事件流。\n\n" +
    "【回答要求】\n" +
    "1. 针对快照中的异常项，先说明问题，再给出「在 Win-Top 中怎么做」的具体操作路径。\n" +
    "2. 涉及结束进程、清理磁盘、改防火墙等有风险操作时，务必提示风险（如 dwm.exe/svchost.exe 等系统关键进程不可随意结束）。\n" +
    "3. 不要编造快照中没有的数据。回答用 Markdown，简洁、条理清晰。";

  // 按 id 更新一条消息的文本（不可变更新，keyed each 下组件实例保持稳定）
  function setMessageText(id, text) {
    messages = messages.map((m) => (m.id === id ? { ...m, text } : m));
  }

  // ===== 流式增量的 rAF 节流 =====
  // 网络回调随时到达；DOM 更新对齐渲染帧，每帧至多一次。
  let pendingText = null;
  let rafId = 0;
  function queueStreamText(id, text) {
    pendingText = text;
    if (rafId) return;
    rafId = requestAnimationFrame(() => {
      rafId = 0;
      if (pendingText !== null && streamId === id) {
        setMessageText(id, pendingText);
        pendingText = null;
        scrollToBottom();
      }
    });
  }
  function cancelQueued() {
    if (rafId) cancelAnimationFrame(rafId);
    rafId = 0;
    pendingText = null;
  }

  function finalize(id, text) {
    cancelQueued();
    setMessageText(id, text);
    sending = false;
    streamId = null;
    client = null;
    scrollToBottom();
  }

  function send(text) {
    const content = (text ?? input).trim();
    if (!content || sending) return;
    if (!configured) { showConfig = true; pushToast("请先配置 API", "warn"); return; }

    input = "";
    messages = [...messages, { id: ++msgSeq, role: "user", text: content }];

    // 快照为纯内存读取(读 store,零 invoke),瞬时完成
    let snapshot = "";
    try { snapshot = collectSystemSnapshot(); } catch {}

    // 历史在占位消息加入之前构建（原始文本，无损）
    const payloadMessages = [
      { role: "system", content: SYSTEM_PROMPT },
      { role: "system", content: "当前系统快照：\n" + snapshot },
      ...messages.map((m) => ({ role: m.role, content: m.text })),
    ];

    // 占位 assistant 消息：从空文本到完成始终是同一个消息节点
    const aiId = ++msgSeq;
    messages = [...messages, { id: aiId, role: "assistant", text: "" }];
    sending = true;
    streamId = aiId;
    autoFollow = true;
    scrollToBottom(true);

    client = streamChat(
      { baseUrl: cfg.baseUrl, apiKey: cfg.apiKey, model: cfg.model, messages: payloadMessages },
      {
        onDelta: (full) => queueStreamText(aiId, full),
        onDone: (full) => finalize(aiId, full || "(未返回内容)"),
        onFail: (msg) => { finalize(aiId, msg); pushToast("AI 请求失败", "error"); },
        onAbort: (full) => finalize(aiId, full ? full + "\n\n[已停止]" : "[已停止]"),
      }
    );
  }

  function stop() {
    if (client) client.abort();
  }

  function clearChat() {
    if (client) client.abort();
    messages = [];
  }

  function onKeydown(e) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }

  onMount(loadConfig);
  onDestroy(() => {
    cancelQueued();
    if (client) client.abort();
  });
</script>

<!-- 根容器类名 ai-view：绝不可叫 "assistant"——消息 role 同名，
     历史上曾因此让根布局规则命中每条 AI 消息行，造成整屏重叠。 -->
<div class="ai-view">
  <!-- 顶部栏 -->
  <div class="ai-head">
    <div class="ai-title">
      <span class="ai-badge" class:on={configured}></span>
      <span>AI 系统诊断助手</span>
      <span class="ai-model mono">{configured ? `${activeProvider.label} · ${cfg.model}` : "未配置"}</span>
    </div>
    <div class="ai-head-actions">
      {#if messages.length > 0}
        <button class="ghost-sm" on:click={clearChat}>清空对话</button>
      {/if}
      <button class="ghost-sm" on:click={() => (showConfig = !showConfig)}>
        {showConfig ? "收起配置" : "配置"}
      </button>
    </div>
  </div>

  <!-- 配置面板 -->
  {#if showConfig || !configured}
    <div class="config-panel">
      <p class="config-hint">
        接入 OpenAI 兼容接口。选择厂商后自动填好接口地址与最新模型，也可自定义厂商/模型。密钥仅保存在本机 localStorage，不会上传到本软件服务器。
      </p>

      <!-- 厂商选择 -->
      <div class="provider-row">
        {#each PROVIDERS as p}
          <button
            class="provider-chip"
            class:active={cfg.providerId === p.id}
            on:click={() => selectProvider(p.id)}
          >{p.label}</button>
        {/each}
      </div>

      <div class="config-grid">
        <label>
          <span>API 地址</span>
          <input class="cfg-input mono" bind:value={cfg.baseUrl} placeholder="https://api.openai.com/v1" />
        </label>
        <label>
          <span>API Key</span>
          <input class="cfg-input mono" type="password" bind:value={cfg.apiKey} placeholder={activeProvider.keyHint || "sk-..."} />
        </label>
        <label>
          <span>模型</span>
          {#if activeProvider.id !== "custom" && activeProvider.models.length && !customModel}
            <select class="cfg-input mono" value={cfg.model} on:change={onModelSelect}>
              {#each activeProvider.models as m}
                <option value={m}>{m}</option>
              {/each}
              <option value="__custom__">自定义模型…</option>
            </select>
          {:else}
            <div class="model-custom">
              <input class="cfg-input mono" bind:value={cfg.model} placeholder="输入模型名，如 gpt-4o" />
              {#if activeProvider.id !== "custom" && activeProvider.models.length}
                <button class="link-btn" on:click={() => { customModel = false; if (!activeProvider.models.includes(cfg.model)) cfg.model = activeProvider.models[0]; cfg = cfg; }}>选预设</button>
              {/if}
            </div>
          {/if}
        </label>
      </div>
      <div class="config-actions">
        <button class="primary" on:click={saveConfig} disabled={!cfg.baseUrl || !cfg.apiKey || !cfg.model}>保存配置</button>
      </div>
    </div>
  {/if}

  <!-- 对话区 -->
  <div class="chat-scroll" bind:this={scroller} on:scroll={onScroll}>
    {#if messages.length === 0}
      <div class="empty-state">
        <div class="empty-title">向 AI 询问系统状况</div>
        <p class="empty-sub">每次提问会附带当前 CPU / 内存 / 磁盘 / 进程的真实快照，AI 基于实时数据给出建议。</p>
        <div class="quick-grid">
          {#each quickPrompts as p}
            <button class="quick-card" on:click={() => send(p)} disabled={sending}>{p}</button>
          {/each}
        </div>
      </div>
    {:else}
      {#each messages as m (m.id)}
        <ChatMessage role={m.role} text={m.text} streaming={sending && m.id === streamId} />
      {/each}
    {/if}
  </div>

  <!-- 输入区 -->
  <div class="input-bar">
    <textarea
      class="ai-input"
      bind:value={input}
      on:keydown={onKeydown}
      placeholder={configured ? "描述你的问题，Enter 发送，Shift+Enter 换行" : "请先在上方配置 API"}
      rows="1"
      disabled={sending}
    ></textarea>
    {#if sending}
      <button class="stop-btn" on:click={stop}>停止</button>
    {:else}
      <button class="send-btn" on:click={() => send()} disabled={!input.trim()}>发送</button>
    {/if}
  </div>
</div>

<style>
  .ai-view {
    display: flex;
    flex-direction: column;
    height: calc(100vh - 140px);
    min-height: 400px;
  }
  .mono { font-family: var(--font-mono); font-variant-numeric: tabular-nums; }

  .ai-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: var(--sp-3);
  }
  .ai-title {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: 15px;
    font-weight: 600;
  }
  .ai-badge {
    width: 8px;
    height: 8px;
    border-radius: 999px;
    background: var(--text-muted);
  }
  .ai-badge.on {
    background: var(--ok);
    box-shadow: 0 0 8px rgba(34, 197, 94, 0.6);
  }
  .ai-model {
    font-size: 11px;
    color: var(--text-muted);
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--surface-2);
  }
  .ai-head-actions { display: flex; gap: var(--sp-2); }
  .ghost-sm {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-muted);
    font-family: inherit;
    font-size: 12px;
    padding: 5px 12px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .ghost-sm:hover { background: var(--surface-2); color: var(--text); }

  .config-panel {
    border: 1px solid var(--border);
    background: var(--surface);
    border-radius: var(--radius);
    padding: var(--sp-4);
    margin-bottom: var(--sp-3);
  }
  .config-hint { margin: 0 0 var(--sp-3); font-size: 12px; color: var(--text-muted); line-height: 1.6; }
  .provider-row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: var(--sp-3);
  }
  .provider-chip {
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text-muted);
    font-family: inherit;
    font-size: 12px;
    padding: 5px 12px;
    border-radius: 999px;
    cursor: pointer;
    transition: all 0.15s;
  }
  .provider-chip:hover { color: var(--text); border-color: var(--accent); }
  .provider-chip.active {
    color: #fff;
    background: var(--accent);
    border-color: var(--accent);
  }
  .config-grid {
    display: grid;
    grid-template-columns: 1fr 1fr 1fr;
    gap: var(--sp-3);
  }
  .config-grid label { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--text-muted); }
  .cfg-input {
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--bg, var(--surface-2));
    color: var(--text);
    font-size: 12px;
  }
  select.cfg-input { cursor: pointer; appearance: none; }
  .cfg-input:focus-visible { outline: 2px solid var(--accent); outline-offset: 1px; }
  .model-custom { display: flex; gap: 6px; align-items: stretch; }
  .model-custom .cfg-input { flex: 1; min-width: 0; }
  .link-btn {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-muted);
    font-family: inherit;
    font-size: 11px;
    padding: 0 10px;
    border-radius: var(--radius-sm);
    cursor: pointer;
    white-space: nowrap;
  }
  .link-btn:hover { color: var(--accent); border-color: var(--accent); }
  .config-actions { margin-top: var(--sp-3); }

  .chat-scroll {
    flex: 1;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    padding: var(--sp-4);
  }

  .empty-state { max-width: 640px; margin: var(--sp-6) auto; text-align: center; }
  .empty-title { font-size: 18px; font-weight: 600; margin-bottom: var(--sp-2); }
  .empty-sub { font-size: 13px; color: var(--text-muted); line-height: 1.6; margin: 0 0 var(--sp-6); }
  .quick-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--sp-3);
  }
  .quick-card {
    text-align: left;
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
    padding: 12px 14px;
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: border-color 0.15s, background 0.15s;
  }
  .quick-card:hover:not(:disabled) { border-color: var(--accent); background: rgba(99, 102, 241, 0.08); }
  .quick-card:disabled { opacity: 0.5; cursor: default; }

  .input-bar {
    display: flex;
    align-items: flex-end;
    gap: var(--sp-2);
    margin-top: var(--sp-3);
  }
  .ai-input {
    flex: 1;
    resize: none;
    max-height: 140px;
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text);
    font-family: inherit;
    font-size: 14px;
    line-height: 1.5;
  }
  .ai-input:focus-visible { outline: 2px solid var(--accent); outline-offset: 1px; }
  .send-btn, .stop-btn {
    flex-shrink: 0;
    border: none;
    font-family: inherit;
    font-size: 14px;
    padding: 10px 20px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .send-btn { background: linear-gradient(135deg, var(--accent), #7c3aed); color: #fff; }
  .send-btn:disabled { opacity: 0.5; cursor: default; }
  .stop-btn { background: var(--surface-2); color: var(--danger); border: 1px solid rgba(239, 68, 68, 0.4); }
  .primary {
    border: none;
    background: linear-gradient(135deg, var(--accent), #7c3aed);
    color: #fff;
    font-family: inherit;
    font-size: 13px;
    padding: 8px 16px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .primary:disabled { opacity: 0.6; cursor: default; }

  @media (max-width: 720px) {
    .config-grid { grid-template-columns: 1fr; }
    .quick-grid { grid-template-columns: 1fr; }
  }
</style>
