// Win-Top 前端纯函数测试。
// 运行：node --test src/lib/markdown.test.js
//
// 重点覆盖 AI 助手消息渲染：模型输出是不可信输入（endpoint 由用户配置，
// 内容还可能被注入的系统快照影响），而它经 { @html } 直接进 DOM。
import { test } from "node:test";
import assert from "node:assert/strict";

import { renderMarkdown, escapeHtml, escapeAttr } from "./markdown.js";

test("escapeHtml 覆盖文本节点的三个危险字符", () => {
  assert.equal(escapeHtml("<b>&</b>"), "&lt;b&gt;&amp;&lt;/b&gt;");
});

test("escapeAttr 转义引号，escapeHtml 不转义引号", () => {
  assert.equal(escapeAttr('a"b'), "a&quot;b");
  assert.equal(escapeAttr("a'b"), "a&#39;b");
  // escapeHtml 是文本节点用，不处理引号是刻意的
  assert.equal(escapeHtml('a"b'), 'a"b');
});

test("恶意链接不得注入 HTML 属性", () => {
  // 真实攻击载荷：URL 里的 `"` 会闭合 href 属性并挂上新的事件处理器。
  // 括号只写一层，保证能匹配上链接语法——否则测的只是"正则没匹配"。
  const evil = '[click](https://x/"onmouseover="alert(1))';
  const html = renderMarkdown(evil);

  // 修复后的实际行为：带引号的 URL 被链接字符类拒绝，整段作为**惰性文本**输出。
  // 因此不能断言"载荷字符串不存在"——它以文本形式保留是正确且安全的。
  // 要断言的是：没有生成 <a>，也没有出现任何新的事件属性。
  assert.ok(!html.includes("<a "), `不应为该载荷生成链接：${html}`);
  assert.ok(!/\son\w+\s*=/.test(html), `出现了新的事件属性：${html}`);
});

test("单引号同样不得逃出属性", () => {
  const evil = "[click](https://x/'onmouseover='alert(1))";
  const html = renderMarkdown(evil);
  assert.ok(!/\son\w+\s*=/.test(html), `单引号逃逸未被阻止：${html}`);
});

test("正常链接仍然渲染成可点击的 a 标签，且查询串不被二次转义", () => {
  const html = renderMarkdown("[文档](https://example.com/a?b=1&c=2)");
  assert.ok(html.includes("<a "), `应生成链接：${html}`);
  assert.ok(html.includes('href="https://example.com/a?b=1&amp;c=2"'), html);
  assert.ok(!html.includes("&amp;amp;"), `查询串被二次转义，链接会失效：${html}`);
});

test("非 http/https 协议不生成链接", () => {
  const html = renderMarkdown("[x](javascript:alert(1))");
  assert.ok(!html.includes("<a "), `不应生成链接：${html}`);
});

test("模型输出里的原始 HTML 不会成为标签", () => {
  const html = renderMarkdown('<img src=x onerror="alert(1)">');
  assert.ok(!html.includes("<img"), html);
});

test("行内代码块内容不被二次解析", () => {
  const html = renderMarkdown("`<b>not bold</b>`");
  assert.ok(!html.includes("<b>"), html);
});
