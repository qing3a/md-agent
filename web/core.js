/* md-agent 前端纯逻辑核心（无 DOM 依赖，Node 可测；浏览器挂 window.Core）
 * 关键词提取 / 工具调用 JSON 解析——从 app.js 提取，保持与 DOM 层分离 */
(function (root) {
  const Core = {};

  // ---------- 关键词提取 ----------
  // ASCII 单词直接提取；中文按多字功能词 + 语气助词切分，
  // 切出的 CJK 片段(≥2)取「整段 + 滑动双字」，避免单字停用字切断实体词（如 向量/配置）。
  const FW = [
    '为什么','怎么样','怎么能','是不是','会不会','有没有','能不能','需要','帮忙',
    '什么','怎么','如何','请问','怎样','为啥','咋','一下','这个','那个','这些','那些',
    '因为','所以','如果','但是','然后','可以','就是','不是','一个','我们','你们','他们',
  ].sort((a, b) => b.length - a.length); // 长的先切，如 为什么 先于 什么
  const PARTICLES = new Set('吗呢啊呀吧么的得地了着是吗哦嘛嗯');

  Core.extractKeywords = function (q) {
    const kw = new Set();
    for (const m of q.matchAll(/[A-Za-z0-9][A-Za-z0-9_-]*/g)) kw.add(m[0].toLowerCase());
    let cjk = q.replace(/[A-Za-z0-9_-]+/g, ' ');
    for (const w of FW) cjk = cjk.split(w).join(' ');
    let buf = '';
    const flush = () => {
      const t = buf.trim();
      if (t.length >= 2) {
        kw.add(t);
        if (t.length >= 3) {
          for (let i = 0; i + 2 <= t.length; i++) kw.add(t.slice(i, i + 2));
        }
      }
      buf = '';
    };
    for (const ch of cjk) {
      if (/\s/.test(ch)) { flush(); continue; }
      if (PARTICLES.has(ch)) { flush(); continue; }
      buf += ch;
    }
    flush();
    return [...kw];
  };

  // ---------- 工具调用 JSON 解析（Phase 3-C Step 1） ----------

  // 栈扫描提取顶层 JSON 对象（正确处理嵌套 args 与字符串内大括号）
  Core.extractJsonObjects = function (text) {
    const out = [];
    let i = 0;
    while (i < text.length) {
      if (text[i] !== '{') { i++; continue; }
      let depth = 0, j = i, inStr = false;
      for (; j < text.length; j++) {
        const ch = text[j];
        if (inStr) {
          if (ch === '\\') j++;
          else if (ch === '"') inStr = false;
          continue;
        }
        if (ch === '"') inStr = true;
        else if (ch === '{') depth++;
        else if (ch === '}') { depth--; if (depth === 0) break; }
      }
      if (depth !== 0) break; // 未闭合，放弃
      out.push(text.slice(i, j + 1));
      i = j + 1;
    }
    return out;
  };

  // 解析工具调用 JSON：必须以 { 开头；支持一次输出多个工具 JSON（DeepSeek 会用空格分隔），取第一个合法；
  // 兼容平铺参数（{"tool":"x","q":..}）与 args 包裹（{"tool":"x","args":{..}}）；toolApi 为工具名→处理器的映射
  Core.tryParseTool = function (text, toolApi) {
    const t = String(text || '').trim();
    if (!t.startsWith('{')) return null;
    for (const raw of Core.extractJsonObjects(t)) {
      try {
        const j = JSON.parse(raw);
        if (j && typeof j.tool === 'string' && toolApi && toolApi[j.tool]) {
          const args = (j.args && typeof j.args === 'object' && !Array.isArray(j.args)) ? j.args : j;
          return { tool: j.tool, args };
        }
      } catch (e) { /* 跳过非工具 JSON 对象 */ }
    }
    return null;
  };

  // 流结束后全文检测工具调用：DeepSeek 可能先输出简短说明再输出工具 JSON（前缀 <200 字视为工具意图）
  Core.detectToolInFull = function (text, toolApi) {
    const t = String(text || '').trim();
    if (t.startsWith('{')) return Core.tryParseTool(t, toolApi);
    const ti = t.indexOf('{"tool"');
    if (ti === -1) return null;
    const lead = t.slice(0, ti).trim();
    if (lead.length > 200) return null; // 前缀是长正文，非工具调用
    for (const raw of Core.extractJsonObjects(t.slice(ti))) {
      try {
        const j = JSON.parse(raw);
        if (j && typeof j.tool === 'string' && toolApi && toolApi[j.tool]) {
          const args = (j.args && typeof j.args === 'object' && !Array.isArray(j.args)) ? j.args : j;
          return { tool: j.tool, args };
        }
      } catch (e) { /* 跳过 */ }
    }
    return null;
  };

  // LCS 行级 diff（O(n*m)，小文档够用；大文档退化逐行对比）
  Core.diffLines = function (a, b) {
    const n = a.length, m = b.length;
    if (n * m > 4_000_000) {
      const out = [];
      const max = Math.max(n, m);
      for (let i = 0; i < max; i++) {
        if (a[i] !== b[i]) {
          if (i < n) out.push({ t: '-', line: a[i] });
          if (i < m) out.push({ t: '+', line: b[i] });
        }
      }
      return out;
    }
    const dp = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
    for (let i = n - 1; i >= 0; i--) {
      for (let j = m - 1; j >= 0; j--) {
        dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
      }
    }
    const out = [];
    let i = 0, j = 0;
    while (i < n && j < m) {
      if (a[i] === b[j]) { out.push({ t: ' ', line: a[i] }); i++; j++; }
      else if (dp[i + 1][j] >= dp[i][j + 1]) { out.push({ t: '-', line: a[i] }); i++; }
      else { out.push({ t: '+', line: b[j] }); j++; }
    }
    while (i < n) { out.push({ t: '-', line: a[i] }); i++; }
    while (j < m) { out.push({ t: '+', line: b[j] }); j++; }
    return out;
  };

  root.Core = Core;
  if (typeof module !== 'undefined' && module.exports) module.exports = Core;
})(typeof window !== 'undefined' ? window : globalThis);
