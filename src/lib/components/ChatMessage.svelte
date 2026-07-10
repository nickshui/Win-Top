<script>
  import { renderMarkdown, renderUserText } from "../markdown.js";

  // role: 'user' | 'assistant'；text: 原始文本（assistant 为 Markdown 源码）
  // streaming: 该条消息是否仍在流式生成中（空文本时显示打字指示器）
  export let role = "assistant";
  export let text = "";
  export let streaming = false;

  $: isUser = role === "user";
  // assistant 全程按 Markdown 实时渲染（半成品语法会随后续增量自动闭合），
  // 消息节点从占位到完成始终是同一个组件实例，不做「纯文本块→最终消息」的大块交换。
  $: html = isUser ? renderUserText(text) : renderMarkdown(text);
</script>

<!-- 关键教训：消息行的 class 绝不拼接 role 单词（曾因 class="msg assistant"
     被视图根容器 .assistant 的布局规则命中，导致每条 AI 消息行高被钉死在
     calc(100vh - 140px) 且 flex 变 column，内容溢出叠到后续消息上。） -->
<div class="msg" class:mine={isUser}>
  <div class="avatar">{isUser ? "我" : "AI"}</div>
  <div class="bubble">
    {#if !isUser && streaming && !text}
      <span class="typing"><span></span><span></span><span></span></span>
    {:else}
      <div class="content" class:md={!isUser}>{@html html}</div>
    {/if}
  </div>
</div>

<style>
  .msg {
    display: flex;
    gap: var(--sp-3);
    margin-bottom: var(--sp-4);
    align-items: flex-start;
  }
  .msg.mine { flex-direction: row-reverse; }

  .avatar {
    flex-shrink: 0;
    width: 30px;
    height: 30px;
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 12px;
    font-weight: 600;
    background: rgba(34, 197, 94, 0.14);
    color: var(--ok);
  }
  .msg.mine .avatar {
    background: rgba(99, 102, 241, 0.18);
    color: var(--accent);
  }

  .bubble {
    max-width: min(760px, 82%);
    width: fit-content;
    min-width: 0;
    padding: 10px 14px;
    border-radius: 12px;
    border-top-left-radius: 4px;
    background: var(--surface-2);
    overflow: visible;
  }
  .msg.mine .bubble {
    background: rgba(99, 102, 241, 0.14);
    border-top-left-radius: 12px;
    border-top-right-radius: 4px;
  }

  .content {
    font-size: 14px;
    line-height: 1.7;
    word-break: break-word;
  }

  /* Markdown 渲染样式（白名单标签） */
  .content.md :global(> :first-child) { margin-top: 0; }
  .content.md :global(> :last-child) { margin-bottom: 0; }
  .content.md :global(h1),
  .content.md :global(h2),
  .content.md :global(h3),
  .content.md :global(h4) {
    margin: 0.9em 0 0.4em;
    font-weight: 600;
    line-height: 1.35;
  }
  .content.md :global(h1) { font-size: 18px; }
  .content.md :global(h2) { font-size: 16px; }
  .content.md :global(h3) { font-size: 15px; }
  .content.md :global(h4) { font-size: 14px; color: var(--text-muted); }
  .content.md :global(p) { margin: 0.5em 0; }
  .content.md :global(ul),
  .content.md :global(ol) { margin: 0.5em 0; padding-left: 1.4em; }
  .content.md :global(li) { margin: 0.25em 0; }
  .content.md :global(strong) { font-weight: 600; color: var(--text); }
  .content.md :global(em) { font-style: italic; }
  .content.md :global(a) { color: var(--accent); text-decoration: underline; }
  .content.md :global(hr) {
    border: none;
    border-top: 1px solid var(--border);
    margin: 1em 0;
  }
  .content.md :global(blockquote) {
    margin: 0.6em 0;
    padding: 4px 12px;
    border-left: 3px solid var(--accent);
    color: var(--text-muted);
    background: var(--surface-2);
    border-radius: 0 6px 6px 0;
  }
  .content.md :global(code) {
    font-family: var(--font-mono);
    font-size: 12.5px;
    background: var(--surface-2);
    padding: 1px 5px;
    border-radius: 4px;
  }
  .content.md :global(pre) {
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 10px 12px;
    overflow-x: auto;
    margin: 0.6em 0;
  }
  .content.md :global(pre code) {
    background: transparent;
    padding: 0;
    font-size: 12.5px;
    line-height: 1.6;
    white-space: pre;
  }
  .content.md :global(table) {
    border-collapse: collapse;
    width: 100%;
    margin: 0.7em 0;
    font-size: 13px;
    display: block;
    overflow-x: auto;
  }
  .content.md :global(th),
  .content.md :global(td) {
    border: 1px solid var(--border);
    padding: 6px 10px;
    text-align: left;
    vertical-align: top;
  }
  .content.md :global(th) {
    background: var(--surface-2);
    font-weight: 600;
    color: var(--text);
  }
  .content.md :global(tr:nth-child(even) td) {
    background: rgba(255, 255, 255, 0.02);
  }

  .typing { display: inline-flex; gap: 4px; padding: 6px 0; }
  .typing span {
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: var(--text-muted);
    animation: blink 1.2s infinite ease-in-out;
  }
  .typing span:nth-child(2) { animation-delay: 0.2s; }
  .typing span:nth-child(3) { animation-delay: 0.4s; }
  @keyframes blink {
    0%, 80%, 100% { opacity: 0.25; }
    40% { opacity: 1; }
  }
</style>
