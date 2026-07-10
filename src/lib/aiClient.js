// OpenAI 兼容 chat/completions 流式客户端。
// 用 XMLHttpRequest 做流式：onprogress 完全异步回调，绝不阻塞 UI 主线程
// （此前用 fetch+reader 的版本曾参与主线程冻死问题，XHR 方案已真机验证稳定）。
//
// streamChat 返回 { abort }；回调约定：
//   onDelta(fullText)  —— 每批增量后回调（已节流交给调用方），参数是累计全文
//   onDone(fullText)   —— 正常结束（HTTP 2xx）
//   onFail(message)    —— HTTP 错误 / 网络错误 / 超时（同一入口，方便统一 UI）
//   onAbort(fullText)  —— 调用方主动中止

export function streamChat({ baseUrl, apiKey, model, messages, timeoutMs = 60000 }, { onDelta, onDone, onFail, onAbort }) {
  const xhr = new XMLHttpRequest();
  xhr.open("POST", baseUrl.replace(/\/$/, "") + "/chat/completions");
  xhr.setRequestHeader("Content-Type", "application/json");
  xhr.setRequestHeader("Authorization", "Bearer " + apiKey);
  xhr.timeout = timeoutMs;

  let raw = "";       // 累计出的全文
  let processed = 0;  // responseText 已解析长度
  let settled = false;

  const parseChunk = () => {
    const full = xhr.responseText;
    const chunk = full.slice(processed);
    processed = full.length;
    let got = false;
    for (const line of chunk.split("\n")) {
      const t = line.trim();
      if (!t.startsWith("data:")) continue;
      const data = t.slice(5).trim();
      if (data === "[DONE]") continue;
      try {
        const json = JSON.parse(data);
        const delta = json.choices?.[0]?.delta?.content ?? "";
        if (delta) { raw += delta; got = true; }
      } catch {}
    }
    if (got) onDelta(raw);
  };

  xhr.onprogress = parseChunk;

  xhr.onload = () => {
    if (settled) return;
    settled = true;
    parseChunk();
    if (xhr.status >= 200 && xhr.status < 300) {
      onDone(raw);
    } else {
      let msg = `HTTP ${xhr.status}`;
      try {
        const j = JSON.parse(xhr.responseText);
        msg += " " + (j.error?.message || xhr.responseText.slice(0, 200));
      } catch {
        msg += " " + xhr.responseText.slice(0, 200);
      }
      onFail("请求失败：" + msg);
    }
  };

  xhr.onerror = () => {
    if (settled) return;
    settled = true;
    onFail("网络错误：无法连接到 " + baseUrl + "，请检查网络/代理与 API 地址。");
  };

  xhr.ontimeout = () => {
    if (settled) return;
    settled = true;
    onFail(`请求超时(${Math.round(timeoutMs / 1000)}s)：该 API 地址无响应，请检查网络/代理设置。`);
  };

  xhr.onabort = () => {
    if (settled) return;
    settled = true;
    onAbort(raw);
  };

  try {
    xhr.send(JSON.stringify({ model, messages, stream: true }));
  } catch (e) {
    settled = true;
    // send 同步抛错（极少见），走统一失败通道
    queueMicrotask(() => onFail("发送失败：" + (e?.message || e)));
  }

  return {
    abort() { if (!settled) xhr.abort(); },
  };
}
