import test from "node:test";
import assert from "node:assert/strict";
import { get } from "svelte/store";
import { contextMenu, openContextMenu, closeContextMenu } from "./contextMenu.js";

const fakeEvent = (overrides = {}) => ({
  clientX: 10,
  clientY: 20,
  currentTarget: null,
  preventDefault() { this.prevented = true; },
  stopPropagation() { this.stopped = true; },
  ...overrides,
});

test("右键菜单记录坐标并过滤空项", () => {
  const event = fakeEvent();
  openContextMenu(event, [null, { label: "查看详情", action() {} }, false]);
  const menu = get(contextMenu);
  assert.equal(menu.x, 10);
  assert.equal(menu.y, 20);
  assert.deepEqual(menu.items.map((item) => item.label), ["查看详情"]);
  assert.equal(event.prevented, true);
  assert.equal(event.stopped, true);
  closeContextMenu();
  assert.equal(get(contextMenu), null);
});

test("键盘触发时退回到目标元素位置", () => {
  const event = fakeEvent({
    clientX: 0,
    clientY: 0,
    currentTarget: {
      getBoundingClientRect: () => ({ left: 100, top: 50 }),
    },
  });
  openContextMenu(event, [{ label: "复制", action() {} }]);
  const menu = get(contextMenu);
  assert.equal(menu.x, 116);
  assert.equal(menu.y, 66);
  closeContextMenu();
});

test("没有可用菜单项时不打开", () => {
  closeContextMenu();
  openContextMenu(fakeEvent(), [null, false]);
  assert.equal(get(contextMenu), null);
});
