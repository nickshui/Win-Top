<script>
  import Icon from "../components/Icon.svelte";

  export let icon = "cpu";
  export let title = "";
  export let plan = "";
</script>

<div class="placeholder">
  <div class="icon"><Icon name={icon} size={36} /></div>
  <h2>{title}</h2>
  <p class="hint">该模块即将到来</p>
  {#if plan}
    <p class="plan">{plan}</p>
  {/if}
</div>

<style>
  .placeholder {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    gap: var(--sp-3);
    min-height: 60vh;
    color: var(--text-muted);
  }
  .icon {
    width: 72px;
    height: 72px;
    display: grid;
    place-items: center;
    border-radius: 20px;
    background: var(--surface);
    border: 1px solid var(--border);
    color: var(--accent);
  }
  h2 {
    margin: 0;
    font-size: 20px;
    color: var(--text);
  }
  .hint {
    margin: 0;
    font-size: 14px;
  }
  .plan {
    margin: 0;
    max-width: 460px;
    font-size: 13px;
    line-height: 1.7;
  }
</style>
