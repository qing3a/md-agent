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
    let cjk = q
      .replace(/[A-Za-z0-9_-]+/g, ' ')
      // 剥离全角/CJK 标点（，。！？：；（）【】「」『』…·——等），防止标点混入中文短语产生噪声关键词
      .replace(/[\uFF00-\uFF0F\uFF1A-\uFF20\uFF3B-\uFF40\uFF5B-\uFF65\u3000-\u303F\u2018\u2019\u201C\u201D\u2026\u00B7\u2014]/g, ' ');
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

  // ---------- 联网检索触发（web_search 服务端通道） ----------
  // 命中触发词 → ask() 首轮带 web:true（Responses API）；供 ask() 与测试共用
  const WEB_TRIGGERS = ['搜一下', '搜索一下', '联网', '网上', '查一下', '最新', 'websearch', '网上查', '实时'];
  Core.webTrigger = function (q) {
    if (!q) return false;
    const lq = q.toLowerCase();
    return WEB_TRIGGERS.some((w) => lq.includes(w));
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
  // 兼容平铺参数（{"tool":"x","q":..}）与 args 包裹（{"tool":"x","args":{..}}）；toolApi 为工具名→处理器的映射；
  // 工具名容错：DeepSeek 偶发把函数名写成调用式（risk.check() → risk.check）
  Core.normalizeToolName = function (name) {
    return String(name || '').replace(/\(\)\s*$/, '').trim();
  };
  Core.tryParseTool = function (text, toolApi) {
    const t = String(text || '').trim();
    if (!t.startsWith('{')) return null;
    for (const raw of Core.extractJsonObjects(t)) {
      try {
        const j = JSON.parse(raw);
        const name = Core.normalizeToolName(j && j.tool);
        if (name && toolApi && toolApi[name]) {
          const args = (j.args && typeof j.args === 'object' && !Array.isArray(j.args)) ? j.args : j;
          return { tool: name, args };
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
        const name = Core.normalizeToolName(j && j.tool);
        if (name && toolApi && toolApi[name]) {
          const args = (j.args && typeof j.args === 'object' && !Array.isArray(j.args)) ? j.args : j;
          return { tool: name, args };
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

  // ---------- Context Engineering 组装器 v1（模块化 builder，稳定前缀在前） ----------
  // 稳定前缀（会话内不变 → DeepSeek 自动前缀缓存命中）：身份 + L1 规范/记忆 + 工具清单 + 回答规则
  // 易变内容（命中技能）不进 system：由调用方放 userMsg 尾部（tail attachment 语义），避免命中即炸前缀缓存

  // ---------- CE 分节注册表（方向 3，cc-haha systemPromptSection 思路） ----------
  // 系统提示词按 name 分节：普通节会话内 memo（/clear 或配置翻转才重算）；cacheBreak 节每轮重算（必须给 reason）。
  // 保证稳定前缀字节级不变——新增内容默认走 memo 节；易变内容必须显式声明理由，否则静默失效缓存。
  const sectionCache = new Map();
  Core.resetSectionCache = function () { sectionCache.clear(); };
  Core.defineSection = function (name, compute, opts) {
    return { name, compute, opts: opts || {} };
  };
  // 按序解析：memo 节命中缓存直接复用；cacheBreak 节每轮重算（today 等）。返回与 sections 等长的数组（含 null）。
  Core.resolveSections = function (sections) {
    return sections.map((s) => {
      if (!s.opts.cacheBreak && sectionCache.has(s.name)) return sectionCache.get(s.name);
      const v = s.compute();
      if (!s.opts.cacheBreak) sectionCache.set(s.name, v);
      return v;
    });
  };

  Core.buildGuidePrefix = function (parts, opts) {
    // C 半步：llmConfigured 工具化取用时 L1 全文移出前缀（规范/记忆由 read_l1 按需取），引导语随之条件化，避免「声明有 L1 却无内容」的悬空引导
    const hasL1 = !!(parts.guideText || parts.memoryText);
    const fresh = !!(opts && opts.fresh); // fresh-window 降级：绕过缓存（显式丢记忆的最小上下文）
    const sections = [
      Core.defineSection('identity', () =>
        '你是本地双层 MD 知识库的检索问答助手。' + (hasL1 ? '\n以下是知识库 L1 规范/记忆/索引层（权威约定，需遵循）：' : '')),
      Core.defineSection('guide', () => (parts.guideText ? '【规范层（KB/FRAMEWORK/RULES）】\n' + parts.guideText : null)),
      Core.defineSection('memory', () => (parts.memoryText ? '【记忆/索引层（MEMORY/INDEX）】\n' + parts.memoryText : null)),
      Core.defineSection('tools', () =>
        '工具调用（需要更多知识库/网页/文件信息时主动使用）：\n' +
        '调用工具时**第一行就输出**：{"tool":"<工具名>","args":{...}}（不要先输出解释或其它文字，不要代码块标记）；\n' +
        '需要多个信息时可连续输出多行工具调用。\n可用工具：\n' + (parts.toolsTxt || '(工具清单加载失败)') +
        '\n调用后你会收到「工具返回」，基于它继续回答；不需要工具时直接回答。'),
      Core.defineSection(
        'frc',
        () =>
          // 方向 4（cc-haha SUMMARIZE_TOOL_RESULTS 思路）：工具结果注入是「截断摘要」而非全文——
          // 模型被告知可重查，截断不影响取用，同时控制工具轮 miss token 体积
          '工具返回可能是截断摘要（完整内容未全部注入）。使用工具结果时把关键信息写进你的回答；' +
          '之后如需完整内容，重新调用该工具即可。',
        { reason: '静态指令，memo' }
      ),
      Core.defineSection(
        'rules',
        () =>
          '回答规则：\n' +
          '1. 检索与记忆取用规则：回答涉及知识库内容时，**必须先调用工具取用**——规范/历史决策/既有记忆查 read_l1，L2 正文查 search/memory_search；基于工具返回回答，引用标注来源；片段确实不足时如实说明，不得仅凭模型自身知识陈述规范；\n' +
          '2. 优先依据用户消息中给出的检索片段回答，引用格式 [文件:行号]；\n' +
          '3. 片段不足时如实说明，不要编造；\n' +
          '4. 用中文简洁回答；\n' +
          '5. 多轮对话中注意保持与上文一致（引用只需标注本轮片段来源）；\n' +
          '6. 今天是 ' + (parts.today || '') + '。若本次问答产生了值得沉淀的知识（新事实、已定决策、用户纠正、新规范），在回答末尾单独附写回块（不要放进代码块）：\n' +
          '   <!-- md-agent-save -->\n' +
          '   {"path":"相对KB根路径","mode":"append|new","content":"markdown正文"}\n' +
          '   - 新知识：path 指向 notes/ 下的 L2 文件，mode=new，正文含 # 标题；\n' +
          '   - 追加/决策/纠正：path=MEMORY.md，mode=append；\n' +
          '   - 没有可沉淀内容时不要输出该块。',
        { cacheBreak: true, reason: 'today 跨天变化（每天一次 miss，可接受）' }
      ),
    ];
    const rs = fresh ? sections.map((s) => s.compute()) : Core.resolveSections(sections);
    // 结构组装（紧凑，与原始 buildGuidePrefix 输出一致）：identity/guide/memory 连排 → tools → frc → rules
    return rs.filter(Boolean).join('\n');
  };
  Core.buildSkillTail = function (skillTxt) {
    return skillTxt ? '相关技能（命中触发词，按技能步骤执行）：' + skillTxt : '';
  };

  // ---------- 会话管理（A3）：解析 kb/sessions/<id>.md → [{q, a}] ----------
  // 格式：frontmatter + `## Q: <q>\nA: <a>`（条目间 \n\n 分隔）；兼容缺 frontmatter 的旧文件
  Core.parseSessionFile = function (content) {
    const txt = String(content || '');
    const parts = txt.split(/^## Q: /m);
    const out = [];
    for (let i = 1; i < parts.length; i++) {
      const lines = parts[i].split('\n');
      const q = (lines.shift() || '').trim();
      let a = lines.join('\n').trim();
      if (a.startsWith('A: ')) a = a.slice(3).trim();
      if (q) out.push({ q, a });
    }
    return out;
  };

  // CE 双模式：上下文超限错误识别（fresh-window 降级重试触发条件）
  Core.isOverflowError = function (msg) {
    const m = msg || '';
    return /context|token|length|长度|超限|上限|too long|maximum/i.test(m) && !/未配置|api_key|apikey|401|403|404/i.test(m);
  };

  // ---------- 应用市场权限映射（阶段 1）：API 路径 → 粗粒度权限 ----------
  // app 只被放行 app.json 声明的权限；未映射的管理端点（config/heartbeat/health 等）对 app 默认拒绝
  Core.permForPath = function (path) {
    const p = path || '';
    if (/^\/api\/llm/.test(p)) return 'llm';
    if (/^\/api\/(search|l1)/.test(p)) return 'search';
    if (/^\/api\/graph\//.test(p)) return 'graph';
    if (/^\/api\/file/.test(p)) return 'file';
    if (/^\/api\/tasks/.test(p)) return 'tasks';
    if (/^\/api\/fetch/.test(p)) return 'fetch';
    if (/^\/api\/page/.test(p)) return 'page';
    if (/^\/api\/(audit|consolidate)/.test(p)) return 'audit';
    if (/^\/api\/apps\/[^/]+\/data/.test(p)) return 'storage'; // App 状态持久化（localStorage 代理落盘）
    if (/^\/api\/kb\/(sync|pending\/approve|pending\/reject|pending\/preview)/.test(p) || /^\/api\/link/.test(p)) return 'write';
    return null;
  };
  Core.appCan = function (path, method, perms) {
    const perm = Core.permForPath(path);
    if (!perm) return false;
    if (perm === 'file') return (perms || []).includes(method === 'POST' ? 'write' : 'read');
    return (perms || []).includes(perm);
  };

  // ---------- 引用匹配（回答/工具结果中的 [文件:行号] → 图谱可点击） ----------
  // 兼容小节格式 [xx.md:14 小节:...]；路径组排除 [ ] 与空白——[[xx.md:14]] 双链包裹时
  // 匹配内层而非把前导 [ 吞进路径（旧 regex 曾把 path 误抓成 [xx.md）。
  Core.matchRefs = function (text) {
    const RE = /\[([^\[\]\s]+?\.md):(\d+)[^\[\]]*\]/g;
    const out = [];
    let m;
    while ((m = RE.exec(String(text || '')))) {
      out.push({ full: m[0], path: m[1], line: Number(m[2]), start: m.index });
    }
    return out;
  };

  // ---------- 应用结构化结果提取（Phase A：agent:ask 结果回推） ----------
  // 约定标记：回答末尾 `<!-- md-agent-app-data -->{json}<!-- / -->`（与 md-agent-save 块同模式）；
  // 应用侧提示模型输出结构化 JSON，宿主提取后随完成信号回推（app 直接渲染卡片，不必解析文本）
  Core.extractAppData = function (text) {
    const s = String(text || '');
    const m = s.match(/<!--\s*md-agent-app-data\s*-->([\s\S]*?)<!--\s*\/\s*-->/);
    if (!m) return { text: s, data: null };
    let data = null;
    try { data = JSON.parse(m[1].trim()); } catch (e) { data = null; }
    return { text: s.replace(m[0], '').trim(), data };
  };

  root.Core = Core;
  if (typeof module !== 'undefined' && module.exports) module.exports = Core;
})(typeof window !== 'undefined' ? window : globalThis);
