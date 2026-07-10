// 轻量 Markdown 渲染（AI 助手消息气泡用）。
// 安全策略：先整体转义 HTML（防 XSS），再解析块级与行内语法，仅生成白名单标签。
// 注意：因为先转义，块级语法里的 ">" 在文本中已是 "&gt;"，引用块必须按 "&gt;" 匹配。

export function escapeHtml(s) {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

// 用户消息：转义 + 换行转 <br/>
export function renderUserText(text) {
  return escapeHtml(text).replace(/\n/g, "<br />");
}

// 行内：`code`、**bold**、*italic*、[text](url)
// 占位哨兵用 NUL 字符（不可能出现在用户文本里）
const NUL = String.fromCharCode(0);
const CODE_BACK_RE = new RegExp(NUL + "CODE(\\d+)" + NUL, "g");

function renderInline(s) {
  let t = s;
  // 行内代码先占位，避免其中内容被其它规则误伤
  const codes = [];
  t = t.replace(/`([^`]+)`/g, (_, c) => {
    codes.push(c);
    return NUL + "CODE" + (codes.length - 1) + NUL;
  });
  // 链接 [text](http...)，仅允许 http/https
  t = t.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, (_, txt, url) => {
    return `<a href="${url}" target="_blank" rel="noopener noreferrer">${txt}</a>`;
  });
  t = t.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  t = t.replace(/(^|[^*])\*([^*\n]+)\*/g, "$1<em>$2</em>");
  // 还原行内代码
  t = t.replace(CODE_BACK_RE, (_, i) => `<code>${codes[+i]}</code>`);
  return t;
}

// 引用行前缀：转义后的 ">" 是 "&gt;"
const QUOTE_RE = /^\s*&gt;\s?/;

export function renderMarkdown(src) {
  if (!src) return "";
  const escaped = escapeHtml(src);
  const lines = escaped.split("\n");
  let html = "";
  let i = 0;

  const flushList = (buf, ordered) => {
    if (!buf.length) return "";
    const tag = ordered ? "ol" : "ul";
    return `<${tag}>` + buf.map((it) => `<li>${renderInline(it)}</li>`).join("") + `</${tag}>`;
  };

  while (i < lines.length) {
    let line = lines[i];

    // 代码块 ```（流式时允许暂未闭合：吃到结尾即可）
    const fence = line.match(/^\s*```(\w*)\s*$/);
    if (fence) {
      const body = [];
      i++;
      while (i < lines.length && !/^\s*```\s*$/.test(lines[i])) {
        body.push(lines[i]);
        i++;
      }
      i++; // 跳过结束 ```
      html += `<pre><code>${body.join("\n")}</code></pre>`;
      continue;
    }

    // 表格：当前行含 | 且下一行是分隔行 |---|
    if (line.includes("|") && i + 1 < lines.length && /^\s*\|?[\s:|-]+\|[\s:|-]*$/.test(lines[i + 1]) && lines[i + 1].includes("-")) {
      const parseRow = (r) => r.trim().replace(/^\||\|$/g, "").split("|").map((c) => c.trim());
      const headers = parseRow(line);
      i += 2; // 跳过表头与分隔行
      const bodyRows = [];
      while (i < lines.length && lines[i].includes("|") && lines[i].trim() !== "") {
        bodyRows.push(parseRow(lines[i]));
        i++;
      }
      let t = "<table><thead><tr>";
      t += headers.map((h) => `<th>${renderInline(h)}</th>`).join("");
      t += "</tr></thead><tbody>";
      for (const row of bodyRows) {
        t += "<tr>" + headers.map((_, ci) => `<td>${renderInline(row[ci] ?? "")}</td>`).join("") + "</tr>";
      }
      t += "</tbody></table>";
      html += t;
      continue;
    }

    // 标题 # .. ######
    const heading = line.match(/^(#{1,6})\s+(.*)$/);
    if (heading) {
      const level = heading[1].length;
      html += `<h${level}>${renderInline(heading[2])}</h${level}>`;
      i++;
      continue;
    }

    // 分隔线
    if (/^\s*([-*_])\1{2,}\s*$/.test(line)) {
      html += "<hr />";
      i++;
      continue;
    }

    // 引用 >（已转义为 &gt;）
    if (QUOTE_RE.test(line)) {
      const quote = [];
      while (i < lines.length && QUOTE_RE.test(lines[i])) {
        quote.push(lines[i].replace(QUOTE_RE, ""));
        i++;
      }
      html += `<blockquote>${renderInline(quote.join(" "))}</blockquote>`;
      continue;
    }

    // 无序列表
    if (/^\s*[-*+]\s+/.test(line)) {
      const buf = [];
      while (i < lines.length && /^\s*[-*+]\s+/.test(lines[i])) {
        buf.push(lines[i].replace(/^\s*[-*+]\s+/, ""));
        i++;
      }
      html += flushList(buf, false);
      continue;
    }

    // 有序列表
    if (/^\s*\d+\.\s+/.test(line)) {
      const buf = [];
      while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i])) {
        buf.push(lines[i].replace(/^\s*\d+\.\s+/, ""));
        i++;
      }
      html += flushList(buf, true);
      continue;
    }

    // 空行
    if (line.trim() === "") {
      i++;
      continue;
    }

    // 普通段落：合并连续非空、非块级起始行
    const para = [line];
    i++;
    while (
      i < lines.length &&
      lines[i].trim() !== "" &&
      !/^\s*```/.test(lines[i]) &&
      !/^(#{1,6})\s+/.test(lines[i]) &&
      !/^\s*[-*+]\s+/.test(lines[i]) &&
      !/^\s*\d+\.\s+/.test(lines[i]) &&
      !QUOTE_RE.test(lines[i]) &&
      !(lines[i].includes("|") && lines[i].includes("-"))
    ) {
      para.push(lines[i]);
      i++;
    }
    html += `<p>${renderInline(para.join("<br />"))}</p>`;
  }
  return html;
}
