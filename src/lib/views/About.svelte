<script>
  import { open } from "@tauri-apps/api/shell";
  import Icon from "../components/Icon.svelte";
  import { currentVersion } from "../update.js";

  const PROJECT_URL = "https://github.com/nickshui/Win-Top";

  const recommendations = [
    {
      name: "JitWord",
      tagline: "多人实时协同在线 AI Word",
      url: "https://next.jitword.com",
      icon: "📝",
    },
    {
      name: "AiKnow",
      tagline: "可自定义配置 AI 智能体的知识库",
      url: "https://aiknow.jitword.com/",
      icon: "📚",
    },
  ];

  async function openUrl(url) {
    try {
      await open(url);
    } catch (e) {
      window.open(url, "_blank");
    }
  }

  async function openProject() {
    try {
      await open(PROJECT_URL);
    } catch (e) {
      window.open(PROJECT_URL, "_blank");
    }
  }
</script>

<div class="about">
  <div class="hero">
    <span class="logo"></span>
    <div>
      <div class="title">Win-Top <span class="ver">v{$currentVersion || "—"}</span></div>
      <div class="subtitle">Windows 优化加速与系统监控工作台</div>
    </div>
  </div>

  <p class="desc">
    一款开源、原生的 Windows 优化加速与系统监控工具：内存自动加速、开机启动项治理、
    C 盘清理与磁盘空间分析、进程 / 端口 / GPU 监控与网络体检，帮助 Windows 长期保持流畅。
    后端全部采用原生 Windows API（PDH 性能计数器、ETW 实时事件、NtQuerySystemInformation、
    IP Helper、WMI），无 shell 调用、低开销、高响应。
  </p>

  <div class="pills" aria-label="核心能力">
    <span>内存自动加速</span>
    <span>开机启动管理</span>
    <span>C 盘清理</span>
    <span>磁盘空间分析</span>
    <span>进程 / GPU 监控</span>
    <span>端口占用排查</span>
  </div>

  <p class="usecase">
    适合：Windows 越用越卡、开机变慢、C 盘空间不足、内存占用偏高、端口被占用，
    或想看清任务管理器之外的进程 / GPU / 磁盘细节。
  </p>

  <div class="info-grid">
    <div class="info-row">
      <span class="k">作者</span>
      <span class="v">nickshui</span>
    </div>
    <div class="info-row">
      <span class="k">技术栈</span>
      <span class="v">Tauri · Rust · Svelte</span>
    </div>
    <div class="info-row">
      <span class="k">原生能力</span>
      <span class="v">PDH · ETW · NtQuerySystemInformation · IP Helper · WMI</span>
    </div>
    <div class="info-row">
      <span class="k">项目地址</span>
      <button class="link" on:click={openProject}>
        <Icon name="github" size={15} />
        github.com/nickshui/Win-Top
      </button>
    </div>
  </div>

  <button class="primary" on:click={openProject}>
    <Icon name="github" size={16} />
    访问项目主页
  </button>

  <section class="recommend">
    <h3 class="rec-title">推荐产品</h3>
    <div class="rec-grid">
      {#each recommendations as r}
        <button class="rec-card" on:click={() => openUrl(r.url)}>
          <span class="rec-icon">{r.icon}</span>
          <span class="rec-info">
            <span class="rec-name">{r.name}</span>
            <span class="rec-tagline">{r.tagline}</span>
            <span class="rec-url">{r.url}</span>
          </span>
        </button>
      {/each}
    </div>
  </section>

  <p class="copyright">© 2026 nickshui · Win-Top</p>
</div>

<style>
  .about {
    max-width: 720px;
    display: flex;
    flex-direction: column;
    gap: var(--sp-6);
  }
  .hero {
    display: flex;
    align-items: center;
    gap: var(--sp-4);
  }
  .logo {
    width: 48px;
    height: 48px;
    border-radius: 16px;
    background: linear-gradient(135deg, var(--accent), #7c3aed);
    box-shadow: 0 0 24px rgba(99, 102, 241, 0.5);
    flex-shrink: 0;
  }
  .title {
    font-size: 26px;
    font-weight: 700;
  }
  .ver {
    font-size: 14px;
    font-weight: 400;
    color: var(--text-muted);
    font-family: var(--font-mono);
    margin-left: 6px;
  }
  .subtitle {
    color: var(--text-muted);
    font-size: 14px;
    margin-top: 2px;
  }
  .desc {
    margin: 0;
    line-height: 1.8;
    font-size: 14px;
    color: var(--text-muted);
  }
  .pills {
    display: flex;
    flex-wrap: wrap;
    gap: 7px;
  }
  .pills span {
    padding: 4px 10px;
    border: 1px solid rgba(99, 102, 241, 0.35);
    border-radius: 999px;
    background: rgba(99, 102, 241, 0.1);
    color: #c7d2fe;
    font-size: 11px;
  }
  .usecase {
    margin: 0;
    padding: 10px 12px;
    border-left: 2px solid rgba(34, 197, 94, 0.55);
    border-radius: 0 8px 8px 0;
    background: rgba(34, 197, 94, 0.06);
    color: var(--text-muted);
    font-size: 12px;
    line-height: 1.7;
  }
  .info-grid {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: var(--sp-4) var(--sp-6);
  }
  .info-row {
    display: flex;
    align-items: center;
    gap: var(--sp-4);
    padding: 6px 0;
    font-size: 14px;
  }
  .info-row + .info-row {
    border-top: 1px solid var(--border);
  }
  .k {
    width: 88px;
    flex-shrink: 0;
    color: var(--text-muted);
  }
  .v {
    color: var(--text);
  }
  .link {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: none;
    border: none;
    padding: 0;
    color: #93c5fd;
    font-family: var(--font-mono);
    font-size: 14px;
    cursor: pointer;
  }
  .link:hover {
    text-decoration: underline;
  }
  .primary {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    border: none;
    background: linear-gradient(135deg, var(--accent), #7c3aed);
    color: #fff;
    font-family: inherit;
    font-size: 14px;
    padding: 10px 18px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .primary:hover {
    filter: brightness(1.08);
  }
  .recommend {
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
  .rec-title {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
  }
  .rec-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
    gap: var(--sp-3);
  }
  .rec-card {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    text-align: left;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: var(--sp-4);
    cursor: pointer;
    font-family: inherit;
    transition: border-color 0.15s ease, background 0.15s ease, transform 0.15s ease;
  }
  .rec-card:hover {
    border-color: var(--accent);
    background: rgba(99, 102, 241, 0.08);
    transform: translateY(-1px);
  }
  .rec-icon {
    font-size: 26px;
    line-height: 1;
    flex-shrink: 0;
  }
  .rec-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .rec-name {
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
  }
  .rec-tagline {
    font-size: 13px;
    color: var(--text-muted);
  }
  .rec-url {
    font-size: 11px;
    color: #93c5fd;
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .copyright {
    margin: 0;
    font-size: 12px;
    color: var(--text-muted);
  }
</style>
