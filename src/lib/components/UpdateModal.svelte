<script>
  import Modal from "./Modal.svelte";
  import Icon from "./Icon.svelte";
  import { renderMarkdown } from "../markdown.js";
  import {
    currentVersion,
    updateInfo,
    updateChecking,
    updateError,
    updateProgress,
    installPhase,
    installError,
    checkForUpdate,
    installUpdate,
    editionAsset,
    installmentHint,
    openExternal,
  } from "../update.js";

  export let open = false;

  const RELEASES_URL = "https://github.com/nickshui/Win-Top/releases";
  const KIND_LABEL = { nsis: "安装包", msi: "MSI", portable: "便携版", other: "文件" };

  let kicked = false;

  // 第一次打开时按需检查；启动时已静默检查过则直接展示缓存结果。
  $: if (open && !kicked) {
    kicked = true;
    if (!$updateInfo || $updateError) checkForUpdate({ force: !$updateInfo });
  }

  $: info = $updateInfo;
  $: hasUpdate = !!info?.has_update;
  $: notes = (info?.notes || "").trim();
  $: notesHtml = notes ? renderMarkdown(notes) : "";
  $: targetAsset = editionAsset(info);
  $: installSupported = !!info?.can_self_install && !!targetAsset;
  $: editionHint = installmentHint(info);
  $: busy = ["preparing", "downloading", "verifying", "installing"].includes($installPhase);
  $: progressPct = Math.min(100, Math.max(0, Number($updateProgress.pct) || 0));
  $: phaseText =
    $installPhase === "preparing"
      ? "正在准备下载…"
      : $installPhase === "downloading"
        ? `正在下载安装包 ${Math.round(progressPct)}%`
        : $installPhase === "verifying"
          ? "正在校验安装包完整性…"
          : $installPhase === "installing"
            ? "正在安装并重启，请稍候…"
            : "";

  function fmtSize(bytes) {
    if (!bytes || bytes < 0) return "";
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(0)} KB`;
    return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  }

  function fmtDate(iso) {
    if (!iso) return "";
    const d = new Date(iso);
    return Number.isNaN(d.getTime()) ? "" : d.toLocaleString();
  }

  async function download(asset) {
    if (asset?.url) await openExternal(asset.url);
  }

  async function openRelease() {
    await openExternal(info?.html_url || RELEASES_URL);
  }

  async function startInstall() {
    if (!targetAsset) return;
    await installUpdate(targetAsset);
  }
</script>

<Modal bind:open title="软件更新" width={470} on:close={() => (open = false)}>
  <div class="wrap">
    <div class="head">
      <div class="versions">
        <div class="line">
          <span class="label">当前版本</span>
          <span class="ver">v{$currentVersion || "—"}</span>
        </div>
        {#if info}
          <div class="line">
            <span class="label">最新版本</span>
            <span class="ver" class:accent={hasUpdate}>v{info.latest_version}</span>
            {#if info.published_at}
              <span class="dim">· {fmtDate(info.published_at)}</span>
            {/if}
          </div>
        {/if}
      </div>
      {#if $updateChecking}
        <span class="pill"><span class="spinner"></span>检查中</span>
      {:else if hasUpdate}
        <span class="pill new">可更新</span>
      {:else if info}
        <span class="pill ok">已是最新</span>
      {/if}
    </div>

    {#if $updateError}
      <div class="error">检查失败：{$updateError}</div>
      <div class="actions">
        <button class="secondary" on:click={() => checkForUpdate({ force: true })} disabled={$updateChecking}>
          <Icon name="refresh" size={14} /> 重试
        </button>
        <button class="ghost" on:click={openRelease}>
          <Icon name="external" size={14} /> 打开 Releases 页面
        </button>
        <button class="ghost" on:click={() => (open = false)}>关闭</button>
      </div>
    {:else if info && hasUpdate}
      <div class="section">
        <div class="section-title">{info.title}</div>
        {#if notesHtml}
          <div class="notes md">{@html notesHtml}</div>
        {:else}
          <div class="notes empty">该版本未提供更新说明。</div>
        {/if}
      </div>

      <div class="edition-hint">
        <span class="edition-badge">{info.edition_label}</span>
        <span>{editionHint}</span>
      </div>

      {#if $installError}
        <div class="error">自动更新失败：{$installError}</div>
      {/if}

      {#if busy}
        <div class="progress-block">
          <div class="progress-head">
            <span>{phaseText}</span>
            <span class="progress-num">
              {fmtSize($updateProgress.downloaded)}{#if $updateProgress.total || targetAsset?.size}
                / {fmtSize($updateProgress.total || targetAsset?.size)}
              {/if}
            </span>
          </div>
          <div class="progress-track">
            <div class="progress-fill" style="width: {progressPct}%"></div>
          </div>
        </div>
      {/if}

      <div class="section">
        <div class="section-title">手动下载（备用）</div>
        <div class="assets">
          {#each info.assets as asset (asset.name)}
            <div class="asset">
              <span class="kind">{KIND_LABEL[asset.kind] ?? KIND_LABEL.other}</span>
              <span class="name" title={asset.name}>{asset.name}</span>
              <span class="size">{fmtSize(asset.size)}</span>
              <button class="secondary small" on:click={() => download(asset)} disabled={busy}>
                <Icon name="download" size={13} /> 下载
              </button>
            </div>
          {:else}
            <div class="notes empty">该版本没有可下载的安装包，请前往 Releases 页面。</div>
          {/each}
        </div>
        <p class="tips">
          自动更新失败时可在此手动下载；历史数据在
          <code>%LOCALAPPDATA%\WinTop\history</code>，升级不会清除。
        </p>
      </div>

      <div class="actions">
        {#if installSupported}
          <button class="primary" on:click={startInstall} disabled={busy}>
            <Icon name="download" size={15} /> {busy ? "正在更新…" : "立即更新并重启"}
          </button>
        {:else}
          <button class="primary" disabled>
            <Icon name="download" size={15} /> 无法自动更新
          </button>
        {/if}
        <button class="secondary" on:click={openRelease} disabled={busy}>
          <Icon name="external" size={14} /> Release 页面
        </button>
        <button class="ghost" on:click={() => (open = false)} disabled={busy}>关闭</button>
      </div>
    {:else if info}
      <div class="up-to-date">
        <Icon name="check" size={16} />
        <span>已是最新版本 v{info.latest_version}</span>
      </div>
      <div class="actions">
        <button class="secondary" on:click={() => checkForUpdate({ force: true })} disabled={$updateChecking}>
          <Icon name="refresh" size={14} /> 重新检查
        </button>
        <button class="ghost" on:click={openRelease}>
          <Icon name="external" size={14} /> 查看 Release
        </button>
        <button class="ghost" on:click={() => (open = false)}>关闭</button>
      </div>
    {:else if $updateChecking}
      <div class="loading">
        <span class="spinner big"></span>
        正在向 GitHub 查询最新版本…
      </div>
    {:else}
      <div class="loading">尚未检查更新。</div>
      <div class="actions">
        <button class="secondary" on:click={() => checkForUpdate({ force: true })}>
          <Icon name="refresh" size={14} /> 立即检查
        </button>
        <button class="ghost" on:click={() => (open = false)}>关闭</button>
      </div>
    {/if}

    <p class="foot-note">来自 GitHub Releases · 下载后校验 SHA-256 完整性；只访问公开版本信息，不上传任何本机数据</p>
  </div>
</Modal>

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    color: var(--text-muted);
  }

  /* ── 顶部版本信息 ── */
  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--sp-3);
  }
  .versions {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .line {
    display: flex;
    align-items: baseline;
    gap: 6px;
    font-size: 12.5px;
  }
  .label {
    color: var(--text-muted);
    font-size: 11.5px;
    width: 52px;
    flex-shrink: 0;
  }
  .ver {
    font-family: var(--font-mono);
    color: var(--text);
    font-size: 14px;
    letter-spacing: 0.2px;
  }
  .ver.accent {
    color: #a5b4fc;
  }
  .dim {
    color: var(--text-muted);
    font-size: 11px;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex-shrink: 0;
    font-size: 11px;
    padding: 3px 10px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text-muted);
  }
  .pill.new {
    color: #c7d2fe;
    background: rgba(99, 102, 241, 0.14);
    border-color: rgba(99, 102, 241, 0.45);
  }
  .pill.ok {
    color: var(--ok);
    background: rgba(34, 197, 94, 0.1);
    border-color: rgba(34, 197, 94, 0.38);
  }
  .spinner {
    width: 10px;
    height: 10px;
    border-radius: 999px;
    border: 2px solid rgba(148, 163, 184, 0.35);
    border-top-color: var(--text);
    animation: spin 0.8s linear infinite;
    flex-shrink: 0;
  }
  .spinner.big {
    width: 16px;
    height: 16px;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  /* ── 分区 ── */
  .section {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .section-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--text);
    letter-spacing: 0.3px;
  }

  /* ── 更新说明（Markdown） ── */
  .notes {
    max-height: 150px;
    overflow-y: auto;
    padding: 9px 12px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-size: 12.5px;
    line-height: 1.65;
    color: var(--text-muted);
    overflow-wrap: anywhere;
  }
  .notes::-webkit-scrollbar {
    width: 7px;
  }
  .notes::-webkit-scrollbar-thumb {
    background: var(--surface-2);
    border-radius: 999px;
  }
  .notes::-webkit-scrollbar-track {
    background: transparent;
  }
  .notes.empty {
    color: var(--text-muted);
    font-style: italic;
  }
  .notes.md :global(> :first-child) {
    margin-top: 0;
  }
  .notes.md :global(> :last-child) {
    margin-bottom: 0;
  }
  .notes.md :global(h1),
  .notes.md :global(h2),
  .notes.md :global(h3),
  .notes.md :global(h4) {
    margin: 12px 0 4px;
    font-size: 13px;
    color: var(--text);
  }
  .notes.md :global(h1) {
    font-size: 14px;
  }
  .notes.md :global(p) {
    margin: 4px 0;
  }
  .notes.md :global(ul),
  .notes.md :global(ol) {
    margin: 4px 0;
    padding-left: 18px;
  }
  .notes.md :global(li) {
    margin: 2px 0;
  }
  .notes.md :global(strong) {
    color: var(--text);
    font-weight: 600;
  }
  .notes.md :global(a) {
    color: #93c5fd;
    text-decoration: none;
  }
  .notes.md :global(a:hover) {
    text-decoration: underline;
  }
  .notes.md :global(code) {
    font-family: var(--font-mono);
    font-size: 11.5px;
    padding: 1px 4px;
    border-radius: 4px;
    background: var(--surface-2);
    color: var(--text);
  }
  .notes.md :global(blockquote) {
    margin: 6px 0;
    padding-left: 10px;
    border-left: 2px solid var(--border);
  }
  .notes.md :global(hr) {
    border: none;
    border-top: 1px solid var(--border);
    margin: 10px 0;
  }

  /* ── 下载列表 ── */
  .assets {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .asset {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 7px 10px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    transition: border-color 0.15s ease;
  }
  .asset:hover {
    border-color: var(--surface-2);
  }
  .kind {
    flex-shrink: 0;
    font-size: 10.5px;
    padding: 2px 7px;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--text-muted);
  }
  .name {
    flex: 1;
    min-width: 0;
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .size {
    flex-shrink: 0;
    font-size: 11px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .tips {
    margin: 2px 0 0;
    font-size: 11px;
    line-height: 1.5;
    color: var(--text-muted);
  }
  .tips code {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text);
  }

  /* ── 安装方式提示与下载进度 ── */
  .edition-hint {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    background: rgba(99, 102, 241, 0.08);
    border: 1px solid rgba(99, 102, 241, 0.28);
    font-size: 11.5px;
    line-height: 1.55;
    color: #c7d2fe;
  }
  .edition-badge {
    flex-shrink: 0;
    font-size: 10.5px;
    padding: 2px 7px;
    border-radius: 999px;
    background: rgba(99, 102, 241, 0.18);
    border: 1px solid rgba(99, 102, 241, 0.35);
    color: #c7d2fe;
  }
  .progress-block {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 9px 12px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .progress-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    font-size: 11.5px;
    color: var(--text);
  }
  .progress-num {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .progress-track {
    height: 6px;
    border-radius: 999px;
    background: var(--surface-2);
    overflow: hidden;
  }
  .progress-fill {
    height: 100%;
    border-radius: 999px;
    background: linear-gradient(90deg, var(--accent), #8b5cf6);
    transition: width 0.15s linear;
  }

  /* ── 状态块 ── */
  .error {
    padding: 9px 12px;
    border-radius: var(--radius-sm);
    font-size: 12.5px;
    line-height: 1.6;
    color: #fca5a5;
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.35);
    overflow-wrap: anywhere;
  }
  .loading {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 14px 0;
    color: var(--text-muted);
    font-size: 12.5px;
  }
  .up-to-date {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    color: var(--ok);
    background: rgba(34, 197, 94, 0.08);
    border: 1px solid rgba(34, 197, 94, 0.28);
    font-size: 12.5px;
  }

  /* ── 按钮 ── */
  .actions {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    flex-wrap: wrap;
  }
  button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    font-family: inherit;
    font-size: 12.5px;
    padding: 8px 14px;
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: filter 0.15s ease, background 0.15s ease, border-color 0.15s ease;
  }
  button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .primary {
    border: none;
    color: #fff;
    background: linear-gradient(135deg, var(--accent), #7c3aed);
  }
  .primary:hover:not(:disabled) {
    filter: brightness(1.08);
  }
  .secondary {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text);
  }
  .secondary:hover:not(:disabled) {
    background: var(--surface-2);
  }
  .ghost {
    border: 1px solid transparent;
    background: transparent;
    color: var(--text-muted);
  }
  .ghost:hover:not(:disabled) {
    color: var(--text);
    background: var(--surface-2);
  }
  .small {
    padding: 5px 10px;
    font-size: 11.5px;
  }

  .foot-note {
    margin: 0;
    padding-top: var(--sp-3);
    border-top: 1px solid var(--border);
    font-size: 11px;
    line-height: 1.5;
    color: var(--text-muted);
    opacity: 0.85;
  }
</style>
