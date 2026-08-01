<script>
  import { createEventDispatcher, tick } from "svelte";

  export let open = false;
  export let title = "";

  const dispatch = createEventDispatcher();
  let dialog;
  let restoreTarget;
  let wasOpen = false;

  $: {
    if (open && !wasOpen) activate();
    if (!open && wasOpen) restoreFocus();
    wasOpen = open;
  }

  async function activate() {
    restoreTarget = document.activeElement;
    await tick();
    const [first] = focusableControls();
    (first || dialog)?.focus();
  }

  async function restoreFocus() {
    await tick();
    restoreTarget?.focus?.();
    restoreTarget = null;
  }

  const close = () => dispatch("close");

  function focusableControls() {
    return dialog
      ? [...dialog.querySelectorAll('button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex="-1"])')]
      : [];
  }

  function onKey(e) {
    if (!open) return;
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      return;
    }
    if (e.key !== "Tab" || !dialog) return;
    const controls = focusableControls();
    if (!controls.length) {
      e.preventDefault();
      return;
    }
    const first = controls[0];
    const last = controls[controls.length - 1];
    if (!dialog.contains(document.activeElement)) {
      e.preventDefault();
      (e.shiftKey ? last : first).focus();
    } else if (e.shiftKey && (document.activeElement === first || document.activeElement === dialog)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  }
</script>

<svelte:window on:keydown={onKey} />

{#if open}
  <div class="backdrop">
    <button class="backdrop-btn" aria-label="关闭" on:click={close}></button>
    <div bind:this={dialog} class="modal" role="dialog" aria-modal="true" aria-labelledby="modal-title" tabindex="-1">
      <h3 id="modal-title">{title}</h3>
      <div class="body"><slot /></div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
  }
  .backdrop-btn {
    position: absolute;
    inset: 0;
    border: none;
    padding: 0;
    margin: 0;
    background: rgba(15, 23, 42, 0.7);
    backdrop-filter: blur(2px);
    cursor: pointer;
  }
  .modal {
    position: relative;
    width: 380px;
    max-width: calc(100vw - 32px);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: var(--sp-6);
    display: flex;
    flex-direction: column;
    gap: var(--sp-4);
    outline: none;
  }
  .modal:focus-visible { box-shadow: 0 0 0 2px rgba(99, 102, 241, 0.5); }
  h3 {
    margin: 0;
    font-size: 16px;
  }
  .body {
    font-size: 13px;
    color: var(--text-muted);
    line-height: 1.6;
  }
</style>
