<script>
  import { toasts } from "../stores.js";
</script>

<div class="toast-container">
  {#each $toasts as t (t.id)}
    <div class="toast {t.kind}">{t.message}</div>
  {/each}
</div>

<style>
  .toast-container {
    position: fixed;
    bottom: var(--sp-6);
    right: var(--sp-6);
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    z-index: 60;
    max-width: 380px;
  }
  .toast {
    padding: 10px 14px;
    border-radius: var(--radius-sm);
    font-size: 13px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--text);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
    animation: slide-in 0.2s ease;
  }
  .toast.ok {
    border-left: 3px solid var(--ok);
  }
  .toast.error {
    border-left: 3px solid var(--danger);
  }
  @keyframes slide-in {
    from {
      transform: translateY(8px);
      opacity: 0;
    }
    to {
      transform: translateY(0);
      opacity: 1;
    }
  }
</style>
