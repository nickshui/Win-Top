<script>
  import { onMount } from "svelte";
  import UpdateModal from "./UpdateModal.svelte";
  import {
    currentVersion,
    updateInfo,
    updateChecking,
    ensureVersion,
    checkForUpdate,
  } from "../update.js";

  let modalOpen = false;

  onMount(() => {
    ensureVersion();
    // 启动后静默检查一次：失败不弹提示，只在弹窗里展示原因
    const timer = setTimeout(() => checkForUpdate({ silent: true }), 2500);
    return () => clearTimeout(timer);
  });

  $: info = $updateInfo;
  $: hasUpdate = !!info?.has_update;
  $: hint = $updateChecking ? "检查中…" : hasUpdate ? `可更新 v${info.latest_version}` : "检查更新";
</script>

<button
  class="version-row"
  class:has-update={hasUpdate}
  on:click={() => (modalOpen = true)}
  title="版本与更新"
  aria-label={`当前版本 v${$currentVersion || "未知"}，点击检查更新`}
>
  <span class="dot" class:on={hasUpdate}></span>
  <span class="ver">v{$currentVersion || "—"}</span>
  <span class="hint">{hint}</span>
</button>

<UpdateModal bind:open={modalOpen} />

<style>
  .version-row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px var(--sp-2);
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-muted);
    font-family: inherit;
    font-size: 11.5px;
    line-height: 1.4;
    cursor: pointer;
    text-align: left;
    transition: background 0.15s ease, border-color 0.15s ease, color 0.15s ease;
  }
  .version-row:hover {
    background: var(--surface-2);
    border-color: var(--border);
    color: var(--text);
  }
  .version-row:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: var(--border);
    flex-shrink: 0;
    transition: background 0.15s ease, box-shadow 0.15s ease;
  }
  .dot.on {
    background: var(--accent);
    box-shadow: 0 0 8px rgba(99, 102, 241, 0.8);
  }
  .ver {
    font-family: var(--font-mono);
    font-size: 11px;
    letter-spacing: 0.2px;
    color: var(--text-muted);
  }
  .has-update .ver {
    color: var(--text);
  }
  .hint {
    margin-left: auto;
    font-size: 10.5px;
    white-space: nowrap;
    opacity: 0;
    transition: opacity 0.15s ease;
  }
  .version-row:hover .hint,
  .version-row:focus-visible .hint,
  .has-update .hint {
    opacity: 1;
  }
  .has-update .hint {
    color: #a5b4fc;
  }
</style>
