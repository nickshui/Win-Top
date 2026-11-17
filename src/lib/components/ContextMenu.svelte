<script>
  import { onDestroy, onMount, tick } from "svelte";
  import { contextMenu, closeContextMenu } from "../contextMenu.js";

  let menuEl;
  let pos = { x: 0, y: 0 };
  let ready = false;

  $: menu = $contextMenu;
  $: if (menu) {
    ready = false;
    positionMenu(menu);
  } else {
    ready = false;
  }

  async function positionMenu(value) {
    await tick();
    if (!menuEl) return;
    const pad = 8;
    const rect = menuEl.getBoundingClientRect();
    const x = Math.min(value.x, window.innerWidth - rect.width - pad);
    const y = Math.min(value.y, window.innerHeight - rect.height - pad);
    pos = { x: Math.max(pad, x), y: Math.max(pad, y) };
    ready = true;
    menuEl.querySelector("button:not(:disabled)")?.focus?.();
  }

  function run(item) {
    if (item.disabled || item.separator) return;
    closeContextMenu();
    item.action?.();
  }

  const enabledItems = () => [...(menuEl?.querySelectorAll("button:not(:disabled)") || [])];

  function onKeydown(event) {
    if (!menu) return;
    if (event.key === "Escape") {
      event.preventDefault();
      closeContextMenu();
      return;
    }
    const items = enabledItems();
    if (!items.length) return;
    const index = items.indexOf(document.activeElement);
    if (event.key === "ArrowDown") {
      event.preventDefault();
      items[(index + 1) % items.length].focus();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      items[(index - 1 + items.length) % items.length].focus();
    } else if (event.key === "Home") {
      event.preventDefault();
      items[0].focus();
    } else if (event.key === "End") {
      event.preventDefault();
      items[items.length - 1].focus();
    } else if (event.key === "Tab") {
      event.preventDefault();
    }
  }

  function onWindowPointer(event) {
    if (menu && menuEl && !menuEl.contains(event.target)) closeContextMenu();
  }
  function onWindowBlur() {
    if (menu) closeContextMenu();
  }
  function onWindowScroll() {
    if (menu) closeContextMenu();
  }

  onMount(() => {
    // 捕获阶段监听滚动：内部滚动容器也能让菜单及时收起。
    window.addEventListener("scroll", onWindowScroll, true);
    return () => window.removeEventListener("scroll", onWindowScroll, true);
  });
  onDestroy(closeContextMenu);
</script>

<svelte:window
  on:pointerdown={onWindowPointer}
  on:blur={onWindowBlur}
  on:resize={onWindowBlur}
/>

{#if menu}
  <div
    bind:this={menuEl}
    class="ctx"
    class:ready
    style="left:{pos.x}px; top:{pos.y}px"
    role="menu"
    tabindex="-1"
    on:keydown={onKeydown}
    on:contextmenu|preventDefault
  >
    {#each menu.items as item, index (index)}
      {#if item.separator}
        <div class="sep" role="separator"></div>
      {:else}
        <button
          class="item"
          class:danger={item.danger}
          disabled={item.disabled}
          role="menuitem"
          on:click={() => run(item)}
        >
          <span class="label">{item.label}</span>
          {#if item.hint}<span class="hint">{item.hint}</span>{/if}
        </button>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .ctx {
    position: fixed;
    z-index: 90;
    min-width: 178px;
    max-width: 320px;
    padding: 4px;
    border: 1px solid var(--border);
    border-radius: 9px;
    background: rgba(30, 41, 59, 0.98);
    box-shadow: 0 14px 38px rgba(2, 6, 23, 0.55);
    backdrop-filter: blur(10px);
    visibility: hidden;
    user-select: none;
  }
  .ctx.ready { visibility: visible; }
  .item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 18px;
    width: 100%;
    padding: 7px 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text);
    font-family: inherit;
    font-size: 12px;
    text-align: left;
    cursor: pointer;
  }
  .item:hover, .item:focus-visible {
    outline: none;
    background: rgba(99, 102, 241, 0.18);
  }
  .item:disabled { color: var(--text-muted); opacity: 0.55; cursor: default; }
  .item.danger { color: #fca5a5; }
  .item.danger:hover, .item.danger:focus-visible { background: rgba(239, 68, 68, 0.16); }
  .hint { color: var(--text-muted); font: 11px var(--font-mono); }
  .sep { height: 1px; margin: 4px 6px; background: var(--border); }
</style>
