import { writable } from "svelte/store";

/// 全局唯一的右键菜单状态：{ x, y, items }。组件树只保留一个 ContextMenu 实例。
export const contextMenu = writable(null);

export function openContextMenu(event, items = []) {
  event.preventDefault();
  event.stopPropagation();
  const usable = items.filter(Boolean);
  if (!usable.length) return;
  // 键盘触发（Shift+F10）时 clientX/Y 为 0，退回到目标元素的位置。
  const rect = event.currentTarget?.getBoundingClientRect?.();
  const x = event.clientX || (rect ? rect.left + 16 : 16);
  const y = event.clientY || (rect ? rect.top + 16 : 16);
  contextMenu.set({ x, y, items: usable });
}

export function closeContextMenu() {
  contextMenu.set(null);
}
