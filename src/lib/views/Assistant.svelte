<script>
  import { onMount, tick } from "svelte";
  import { pushToast, collectSystemSnapshot } from "../stores.js";

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
  let messages = [];    // { id, role: 'user'|'assistant', html }
  let msgSeq = 0;
  let streamingRaw = ""; // 当前正在流式接收的原始文本(不放入 messages,避免模板分支切换)
  let input = "";
  let sending = false;
  let scroller;

  // 把一条原始文本转成可渲染的 HTML；assistant→markdown, user→纯文本
  function toHtml(text, role) {
    if (role === "assistant") return renderMarkdown(text);
    // user: 转义 + \n→<br/> (已转义的 &#10; 做换行)
    return escapeHtml(text).replace(/\n/g, "<br />");
  }

  // 滚动到底部。关键：必须通过局部别名 el 赋值 scrollTop，绝不能写
  // `scroller.scrollTop = ...`——对组件级变量 scroller 的成员赋值会被 Svelte
  // 编译成 $$invalidate(scroller)。一旦有响应式语句依赖 scroller，就会形成
  // 「设置滚动→标记 scroller 脏→响应式重跑→再设置滚动」的无限自触发，冻死主线程。
  function scrollToBottom() {
    // Svelte 在 microtask 后才更新 DOM，用 tick 等渲染完成
    tick().then(() => { const el = scroller; if (el) el.scrollTop = el.scrollHeight; });
  }

  // 每次 messages 变更自动滚到底（本响应式块只依赖 messages，不触碰 scroller）
  $: if (messages.length) scrollToBottom();

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

  let xhr = null;

  function send(text) {
    const content = (text ?? input).trim();
    if (!content || sending) return;
    if (!configured) { showConfig = true; pushToast("请先配置 API", "warn"); return; }

    input = "";
    messages = [...messages, { id: ++msgSeq, role: "user", html: toHtml(content, "user") }];
    sending = true;
    streamingRaw = "";

    // 快照为纯内存读取(读 store,零 invoke),瞬时完成
    let snapshot = "";
    try { snapshot = collectSystemSnapshot(); } catch {}

    const payloadMessages = [
      { role: "system", content: SYSTEM_PROMPT },
      { role: "system", content: "当前系统快照：\n" + snapshot },
      ...messages.map((m) => ({ role: m.role, content: m.html.replace(/<[^>]+>/g, "") })),
    ];

    // 用 XMLHttpRequest 做流式：onprogress 完全异步回调,绝不阻塞 UI 主线程。
    xhr = new XMLHttpRequest();
    xhr.open("POST", cfg.baseUrl.replace(/\/$/, "") + "/chat/completions");
    xhr.setRequestHeader("Content-Type", "application/json");
    xhr.setRequestHeader("Authorization", "Bearer " + cfg.apiKey);
    xhr.timeout = 60000; // 60s 硬超时

    let raw = "";
    let processed = 0; // 已解析到的 responseText 长度
    let lastRender = 0;

    const parseChunk = () => {
      const full = xhr.responseText;
      const chunk = full.slice(processed);
      processed = full.length;
      const lines = chunk.split("\n");
      for (const line of lines) {
        const t = line.trim();
        if (!t.startsWith("data:")) continue;
        const data = t.slice(5).trim();
        if (data === "[DONE]") continue;
        try {
          const json = JSON.parse(data);
          raw += json.choices?.[0]?.delta?.content ?? "";
        } catch {}
      }
      const now = Date.now();
      if (now - lastRender >= 80) {
        streamingRaw = raw;
        lastRender = now;
        scrollToBottom();
      }
    };

    xhr.onprogress = parseChunk;

    xhr.onload = () => {
      parseChunk();
      if (xhr.status >= 200 && xhr.status < 300) {
        const finalHtml = toHtml(raw || "(未返回内容)", "assistant");
        messages = [...messages, { id: ++msgSeq, role: "assistant", html: finalHtml }];
      } else {
        // 非 2xx：responseText 可能是错误 JSON
        let msg = `HTTP ${xhr.status}`;
        try { const j = JSON.parse(xhr.responseText); msg += " " + (j.error?.message || xhr.responseText.slice(0, 200)); }
        catch { msg += " " + xhr.responseText.slice(0, 200); }
        messages = [...messages, { id: ++msgSeq, role: "assistant", html: toHtml("请求失败：" + msg, "assistant") }];
        pushToast("AI 请求失败：" + msg, "error");
      }
      finishSend();
    };

    xhr.onerror = () => {
      messages = [...messages, { id: ++msgSeq, role: "assistant", html: toHtml("网络错误：无法连接到 " + cfg.baseUrl + "，请检查网络/代理与 API 地址。", "assistant") }];
      pushToast("网络错误,无法连接 API", "error");
      finishSend();
    };

    xhr.ontimeout = () => {
      messages = [...messages, { id: ++msgSeq, role: "assistant", html: toHtml("请求超时(60s)：该 API 地址无响应,请检查网络/代理设置。", "assistant") }];
      pushToast("请求超时", "error");
      finishSend();
    };

    xhr.onabort = () => {
      const stopped = raw + "\n\n[已停止]";
      messages = [...messages, { id: ++msgSeq, role: "assistant", html: toHtml(stopped, "assistant") }];
      finishSend();
    };

    // 让气泡先渲染,再发起请求(纯异步,不阻塞)
    tick().then(() => {
      try {
        xhr.send(JSON.stringify({ model: cfg.model, messages: payloadMessages, stream: true }));
      } catch (e) {
        pushToast("发送失败：" + e.message, "error");
        finishSend();
      }
    });
  }

  function finishSend() {
    sending = false;
    streamingRaw = "";
    xhr = null;
  }

  function stop() {
    if (xhr) xhr.abort();
  }

  function clearChat() {
    messages = [];
  }

  function onKeydown(e) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }

  // ===== 轻量 Markdown 渲染 =====
  // 先整体转义 HTML（防 XSS），再解析块级与行内语法，仅生成白名单标签。
  function escapeHtml(s) {
    return s
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }

  // 行内：`code`、**bold**、*italic*、[text](url)
  function renderInline(s) {
    let t = s;
    // 行内代码先占位，避免其中内容被其它规则误伤
    const codes = [];
    t = t.replace(/`([^`]+)`/g, (_, c) => {
      codes.push(c);
      return `\u0000CODE${codes.length - 1}\u0000`;
    });
    // 链接 [text](http...)，仅允许 http/https
    t = t.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, (_, txt, url) => {
      return `<a href="${url}" target="_blank" rel="noopener noreferrer">${txt}</a>`;
    });
    t = t.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
    t = t.replace(/(^|[^*])\*([^*\n]+)\*/g, "$1<em>$2</em>");
    // 还原行内代码
    t = t.replace(/\u0000CODE(\d+)\u0000/g, (_, i) => `<code>${codes[+i]}</code>`);
    return t;
  }

  function renderMarkdown(src) {
    if (!src) return "";
    const escaped = escapeHtml(src);
    const lines = escaped.split("\n");
    let html = "";
    let i = 0;

    const flushList = (buf, ordered) => {
      if (!buf.length) return "";
      const tag = ordered ? "ol" : "ul";
      return `<${tag}>` + buf.map((it) => `<li>${renderInline(it)}</li>`).join("") + `</${tag}>`;
    };

    while (i < lines.length) {
      let line = lines[i];

      // 代码块 ```
      const fence = line.match(/^\s*```(\w*)\s*$/);
      if (fence) {
        const body = [];
        i++;
        while (i < lines.length && !/^\s*```\s*$/.test(lines[i])) {
          body.push(lines[i]);
          i++;
        }
        i++; // 跳过结束 ```
        html += `<pre><code>${body.join("\n")}</code></pre>`;
        continue;
      }

      // 表格：当前行含 | 且下一行是分隔行 |---|
      if (line.includes("|") && i + 1 < lines.length && /^\s*\|?[\s:|-]+\|[\s:|-]*$/.test(lines[i + 1]) && lines[i + 1].includes("-")) {
        const parseRow = (r) => r.trim().replace(/^\||\|$/g, "").split("|").map((c) => c.trim());
        const headers = parseRow(line);
        i += 2; // 跳过表头与分隔行
        const bodyRows = [];
        while (i < lines.length && lines[i].includes("|") && lines[i].trim() !== "") {
          bodyRows.push(parseRow(lines[i]));
          i++;
        }
        let t = "<table><thead><tr>";
        t += headers.map((h) => `<th>${renderInline(h)}</th>`).join("");
        t += "</tr></thead><tbody>";
        for (const row of bodyRows) {
          t += "<tr>" + headers.map((_, ci) => `<td>${renderInline(row[ci] ?? "")}</td>`).join("") + "</tr>";
        }
        t += "</tbody></table>";
        html += t;
        continue;
      }

      // 标题 # .. ######
      const heading = line.match(/^(#{1,6})\s+(.*)$/);
      if (heading) {
        const level = heading[1].length;
        html += `<h${level}>${renderInline(heading[2])}</h${level}>`;
        i++;
        continue;
      }

      // 分隔线
      if (/^\s*([-*_])\1{2,}\s*$/.test(line)) {
        html += "<hr />";
        i++;
        continue;
      }

      // 引用 >
      if (/^\s*>\s?/.test(line)) {
        const quote = [];
        while (i < lines.length && /^\s*>\s?/.test(lines[i])) {
          quote.push(lines[i].replace(/^\s*>\s?/, ""));
          i++;
        }
        html += `<blockquote>${renderInline(quote.join(" "))}</blockquote>`;
        continue;
      }

      // 无序列表
      if (/^\s*[-*+]\s+/.test(line)) {
        const buf = [];
        while (i < lines.length && /^\s*[-*+]\s+/.test(lines[i])) {
          buf.push(lines[i].replace(/^\s*[-*+]\s+/, ""));
          i++;
        }
        html += flushList(buf, false);
        continue;
      }

      // 有序列表
      if (/^\s*\d+\.\s+/.test(line)) {
        const buf = [];
        while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i])) {
          buf.push(lines[i].replace(/^\s*\d+\.\s+/, ""));
          i++;
        }
        html += flushList(buf, true);
        continue;
      }

      // 空行
      if (line.trim() === "") {
        i++;
        continue;
      }

      // 普通段落：合并连续非空、非块级起始行
      const para = [line];
      i++;
      while (
        i < lines.length &&
        lines[i].trim() !== "" &&
        !/^\s*```/.test(lines[i]) &&
        !/^(#{1,6})\s+/.test(lines[i]) &&
        !/^\s*[-*+]\s+/.test(lines[i]) &&
        !/^\s*\d+\.\s+/.test(lines[i]) &&
        !/^\s*>\s?/.test(lines[i]) &&
        !(lines[i].includes("|") && lines[i].includes("-"))
      ) {
        para.push(lines[i]);
        i++;
      }
      html += `<p>${renderInline(para.join("<br />"))}</p>`;
    }
    return html;
  }

  onMount(loadConfig);
</script>

<div class="assistant">
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
  <div class="chat-scroll" bind:this={scroller}>
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
        <div class="msg {m.role}">
          <div class="msg-role">{m.role === "user" ? "我" : "AI"}</div>
          <div class="msg-body">
            <div class="msg-text {m.role === 'assistant' ? 'markdown' : ''}">{@html m.html}</div>
          </div>
        </div>
      {/each}
      <!-- 发送中 + 尚无流式内容：显示加载态 -->
      {#if sending && !streamingRaw}
        <div class="msg assistant">
          <div class="msg-role">AI</div>
          <div class="msg-body">
            <span class="typing"><span></span><span></span><span></span></span>
          </div>
        </div>
      {/if}
      <!-- 流式进行中的临时块（纯文本，不存在 messages 里）-->
      {#if streamingRaw}
        <div class="msg assistant">
          <div class="msg-role">AI</div>
          <div class="msg-body">
            <div class="msg-text">{streamingRaw}</div>
          </div>
        </div>
      {/if}
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
  .assistant {
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

  .msg { display: flex; gap: var(--sp-3); margin-bottom: var(--sp-4); align-items: flex-start; }
  .msg.user { flex-direction: row-reverse; }
  .msg-role {
    flex-shrink: 0;
    width: 30px;
    height: 30px;
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 12px;
    font-weight: 600;
  }
  .msg.user .msg-role { background: rgba(99, 102, 241, 0.18); color: var(--accent); }
  .msg.assistant .msg-role { background: rgba(34, 197, 94, 0.14); color: var(--ok); }
  .msg-body {
    max-width: min(760px, 82%);
    width: fit-content;
    min-width: 0;
    padding: 10px 14px;
    border-radius: 12px;
    background: var(--surface-2);
    overflow: visible;
  }
  .msg.assistant .msg-body {
    background: var(--surface-2);
    border-top-left-radius: 4px;
  }
  .msg.user .msg-body {
    background: rgba(99, 102, 241, 0.14);
    border-top-right-radius: 4px;
  }
  .msg-text {
    font-size: 14px;
    line-height: 1.7;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .msg.user .msg-text { color: var(--text); }

  /* Markdown 渲染样式 */
  .msg-text.markdown { white-space: normal; }
  .markdown :global(> :first-child) { margin-top: 0; }
  .markdown :global(> :last-child) { margin-bottom: 0; }
  .markdown :global(h1),
  .markdown :global(h2),
  .markdown :global(h3),
  .markdown :global(h4) {
    margin: 0.9em 0 0.4em;
    font-weight: 600;
    line-height: 1.35;
  }
  .markdown :global(h1) { font-size: 18px; }
  .markdown :global(h2) { font-size: 16px; }
  .markdown :global(h3) { font-size: 15px; }
  .markdown :global(h4) { font-size: 14px; color: var(--text-muted); }
  .markdown :global(p) { margin: 0.5em 0; }
  .markdown :global(ul),
  .markdown :global(ol) { margin: 0.5em 0; padding-left: 1.4em; }
  .markdown :global(li) { margin: 0.25em 0; }
  .markdown :global(strong) { font-weight: 600; color: var(--text); }
  .markdown :global(em) { font-style: italic; }
  .markdown :global(a) { color: var(--accent); text-decoration: underline; }
  .markdown :global(hr) {
    border: none;
    border-top: 1px solid var(--border);
    margin: 1em 0;
  }
  .markdown :global(blockquote) {
    margin: 0.6em 0;
    padding: 4px 12px;
    border-left: 3px solid var(--accent);
    color: var(--text-muted);
    background: var(--surface-2);
    border-radius: 0 6px 6px 0;
  }
  .markdown :global(code) {
    font-family: var(--font-mono);
    font-size: 12.5px;
    background: var(--surface-2);
    padding: 1px 5px;
    border-radius: 4px;
  }
  .markdown :global(pre) {
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 10px 12px;
    overflow-x: auto;
    margin: 0.6em 0;
  }
  .markdown :global(pre code) {
    background: transparent;
    padding: 0;
    font-size: 12.5px;
    line-height: 1.6;
    white-space: pre;
  }
  .markdown :global(table) {
    border-collapse: collapse;
    width: 100%;
    margin: 0.7em 0;
    font-size: 13px;
    display: block;
    overflow-x: auto;
  }
  .markdown :global(th),
  .markdown :global(td) {
    border: 1px solid var(--border);
    padding: 6px 10px;
    text-align: left;
    vertical-align: top;
  }
  .markdown :global(th) {
    background: var(--surface-2);
    font-weight: 600;
    color: var(--text);
  }
  .markdown :global(tr:nth-child(even) td) {
    background: rgba(255, 255, 255, 0.02);
  }

  .typing { display: inline-flex; gap: 4px; padding: 6px 0; }
  .typing span {
    width: 6px; height: 6px; border-radius: 999px; background: var(--text-muted);
    animation: blink 1.2s infinite ease-in-out;
  }
  .typing span:nth-child(2) { animation-delay: 0.2s; }
  .typing span:nth-child(3) { animation-delay: 0.4s; }
  @keyframes blink { 0%, 80%, 100% { opacity: 0.25; } 40% { opacity: 1; } }

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
