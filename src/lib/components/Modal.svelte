<script>
  import { createEventDispatcher } from "svelte";

  export let open = false;
  export let title = "";

  const dispatch = createEventDispatcher();
  const close = () => dispatch("close");

  function onKey(e) {
    if (open && e.key === "Escape") close();
  }
</script>

<svelte:window on:keydown={onKey} />

{#if open}
  <div class="backdrop">
    <button class="backdrop-btn" aria-label="关闭" on:click={close}></button>
    <div class="modal" role="dialog" aria-modal="true" aria-label={title}>
      <h3>{title}</h3>
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
  }
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
