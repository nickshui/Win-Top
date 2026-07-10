<script>
  import { open } from "@tauri-apps/api/shell";
  import Icon from "../components/Icon.svelte";

  const PROJECT_URL = "https://github.com/nickshui/Win-Top";
  const VERSION = "0.1.0";

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
      <div class="title">Win-Top <span class="ver">v{VERSION}</span></div>
      <div class="subtitle">Windows 智能资源管理工作台</div>
    </div>
  </div>

  <p class="desc">
    一款现代、原生的 Windows 资源管理与诊断工具：统一监控 CPU / 内存 / 进程 / 网络端口 / 磁盘，
    集成实时事件流与一键网络体检。后端全部采用原生 Windows API（PDH 性能计数器、ETW 实时事件、
    NtQuerySystemInformation、IP Helper、WMI），无 shell 调用、低开销、高响应。
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
