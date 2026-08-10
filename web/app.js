/* md-agent 前端：DeepSeek 式网页版（左侧会话边栏 + 顶栏 + 气泡消息流 + 底部输入条）操作双层知识库 + Agent 问答回路
 * 回路：启动注入 L1（规范/记忆/索引）→ 用户提问 → 提取关键词 → 检索 L2 → 拼 Prompt → /api/llm 代理 → 回答
 */
(function () {
  if (typeof StreamTerm === 'undefined') {
    document.body.innerHTML =
      '<pre style="color:#f38ba8;padding:16px">stream.js 未加载（缺失或损坏）。请确认 web/ 完整后刷新。</pre>';
    return;
  }
  // 终端壳重做（demo 结构）：DOM 消息流替代 xterm（stream.js），接口保持 writeln/write/onData 等
  const term = new StreamTerm();
  initCardDelegation(); // 交互卡片按钮事件委托（#stream 上绑定一次，重放卡片无需补绑）
  // 会话相对时间：只显示距现在的 小时/天（+ 月/年兜底）；mtime 优先，date 字符串本地解析
  function relTimeOf(s) {
    let ts = s.mtime ? s.mtime * 1000 : NaN;
    if (!ts || isNaN(ts)) {
      if (!s.date) return '';
      const t = new Date(s.date.replace(/-/g, '/') + ' 00:00:00').getTime(); // 本地时区解析 YYYY-MM-DD
      ts = isNaN(t) ? Date.now() : t;
    }
    const diff = Date.now() - ts;
    const min = Math.floor(diff / 60000);
    if (min < 1) return '刚刚';
    if (min < 60) return min + ' 分钟前';
    const h = Math.floor(min / 60);
    if (h < 24) return h + ' 小时前';
    const d = Math.floor(h / 24);
    if (d < 30) return d + ' 天前';
    const mo = Math.floor(d / 30);
    if (mo < 12) return mo + ' 个月前';
    return Math.floor(mo / 12) + ' 年前';
  }

  // 多行串安全写屏：行级渲染，统一换行（stream.js 已按 \n 分行，这里只做兼容规范化）
  // DeepSeek 式原生 textarea：输入事件同步 line（无字符流事件）；草稿防抖落盘；打字计入活动（7 天归档判定）
  term._input.addEventListener('input', () => {
    line = term._input.value;
    touchActivity();
    saveDraft();
  });
  // 联网搜索开关（DeepSeek 式）：开启后本轮首请求带 web:true（服务端 web_search，非流式）
  let webToggle = false;
  document.getElementById('web-toggle').addEventListener('change', (e) => {
    webToggle = e.target.checked;
  });

  // 深度思考 / 联网搜索 toggle 按钮样式切换
  document.getElementById('think-toggle')?.addEventListener('click', function() {
    this.classList.toggle('active');
  });
  document.getElementById('search-toggle')?.addEventListener('click', function() {
    this.classList.toggle('active');
    const wt = document.getElementById('web-toggle');
    if (wt) { wt.checked = this.classList.contains('active'); webToggle = wt.checked; }
  });

  // ---------- 主题（豆包式 data-theme：localStorage 优先 → 跟随系统；切换即时生效 + 广播面板） ----------
  const themeBtn = document.getElementById('theme-btn');
  let currentTheme = document.documentElement.dataset.theme || 'dark';
  function applyTheme(t, persist) {
    currentTheme = t;
    document.documentElement.dataset.theme = t;
    if (persist) { try { localStorage.setItem('md-agent-theme', t); } catch (e) { /* 忽略 */ } }
    if (themeBtn) themeBtn.textContent = t === 'dark' ? '☀️' : '🌙'; // 显示"切换去向"
    // 广播所有面板 iframe（BRIDGE 监听 theme 消息同步）
    for (const f of document.querySelectorAll('#view-panes iframe')) {
      if (f.contentWindow) f.contentWindow.postMessage({ type: 'theme', theme: t }, '*');
    }
  }
  if (themeBtn) themeBtn.addEventListener('click', () => applyTheme(currentTheme === 'dark' ? 'light' : 'dark', true));
  // 面板主题变量（theme.css 同步取一次，注入面板 iframe；主壳已 <link> 引用）
  let themeCss = '';
  try {
    const xhr = new XMLHttpRequest();
    xhr.open('GET', 'theme.css', false);
    xhr.send();
    if (xhr.status === 200) themeCss = xhr.responseText;
  } catch (e) { /* 忽略 */ }
  // 菜单激活高亮：当前打开的内置视图（/view）对应菜单项 .active（豆包式子页面感）
  function updateMenuActive(spec) {
    const arg = spec && spec.arg;
    for (const b of quickBtns) {
      const on = !!arg && b.dataset.view === arg;
      b.classList.toggle('active', on);
    }
  }

  const PROMPT = 'md-agent'; // 保留（历史引用点可能用到；气泡 UI 不显示）
  let L1_TEXT = ''; // 启动时注入的 L1 层全文
  let GUIDE_TEXT = ''; // L1 规范层（KB/FRAMEWORK/RULES）——稳定前缀
  let MEMORY_TEXT = ''; // L1 记忆/索引层（MEMORY/INDEX）——易变
  let llmConfigured = false; // 上下文组装 v2：LLM 配置（endpoint 非空）时知识取用走「工具化」，否则降级「注入 + 启发式预检索」
  // 配置翻转（endpoint 增删，8s 轮询感知）→ 系统提示词分节缓存失效：前缀语义变化，必须重算
  function applyLlmConfigured(v) {
    if (v !== llmConfigured) Core.resetSectionCache();
    llmConfigured = v;
  }
  let history = loadHistory(); // 多轮对话记忆（localStorage 持久化，刷新不丢）
  const MAX_HISTORY = 8; // 最近 4 轮

  // 项目制（多项目硬隔离）：当前项目 id（null = 个人空间默认库）+ 项目列表。
  // 当前项目仅存前端（localStorage），后端按请求头 X-Project 解析隔离根——多窗口各管各的，无共享可变状态
  const PROJECT_KEY = 'md-agent-current-project';
  let currentProject = null;   // null|'default' = 个人空间；否则项目 id
  let currentProjectName = '个人空间';
  let projectList = [];        // [{id,name,template,created}]

  // L0 会话快照（轻量，步骤①）：本页会话的「问题+回答」对，空闲防抖写入 kb/sessions/<时间>.md。
  // 流水非知识——后端已排除 sessions/ 于图谱/检索/心跳指纹；为未来提炼流水线（task.rs 蒸馏 → pending 人审）存原料。
  const MAX_SESSION_LOG = 50;
  let sessionLog = [];      // [{q, a, ts}]
  let sessionFile = null;   // 本会话固定文件名（首次落盘时定）
  let sessionTaskId = null; // 任务驱动会话：绑定任务 id（新任务+对话=新会话；frontmatter 记 task 字段）
  let l0Timer = null;
  function sessionStamp() {
    const d = new Date();
    const p = (n) => String(n).padStart(2, '0');
    return d.getFullYear() + '-' + p(d.getMonth() + 1) + '-' + p(d.getDate()) + '-' + p(d.getHours()) + p(d.getMinutes()) + p(d.getSeconds());
  }
  // A4 会话收尾归档：writeSessionFile(archived) 写快照（archived=true → status=archived）；
  // writeL0Snapshot 保持 fire-and-forget（20s 防抖 / beforeunload 尽力写）
  async function writeSessionFile(archived) {
    if (!sessionLog.length || sessionLog.length === 0) return;
    if (!sessionFile) sessionFile = 'sessions/' + sessionStamp() + '.md';
    const body = sessionLog.map((s) =>
      '## Q: ' + String(s.q || '').slice(0, 300) + '\nA: ' + String(s.a || '(无回答/中断)').slice(0, 3000)
    ).join('\n\n');
    // A1 会话实体化：frontmatter 元数据（title=首问截断 30 字 / status / count / task=任务驱动会话）——/api/sessions lite 枚举数据源
    const title = String((sessionLog[0] && sessionLog[0].q) || '').slice(0, 30);
    const content = '---\ntype: session\ndate: ' + localToday() + '\ntitle: ' + title + '\nstatus: ' + (archived ? 'archived' : 'active') + '\ncount: ' + sessionLog.length + (sessionTaskId ? '\ntask: ' + sessionTaskId : '') + '\n---\n\n# 会话记录\n\n' + body + '\n';
    return api('/api/file', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: sessionFile, content }),
    });
  }
  function writeL0Snapshot() {
    writeSessionFile(false).catch(() => { /* L0 落盘失败不打扰用户 */ });
  }
  function scheduleL0Snapshot() {
    clearTimeout(l0Timer);
    l0Timer = setTimeout(writeL0Snapshot, 20000); // 20s 无新问答 → 落盘
  }
  // 后台执行完成（切走会话）：问答对直接追加到 exe 所属会话文件（归属写入，不动当前全局状态）
  async function appendSessionQA(sessionPath, q, a) {
    try {
      const f = await api('/api/file?path=' + encodeURIComponent(sessionPath)).catch(() => null);
      const head = (f && f.content) || '';
      const qa = '\n\n## Q: ' + String(q || '').slice(0, 300) + '\nA: ' + String(a || '(无回答/中断)').slice(0, 3000);
      const next = head.replace(/^(count:\s*)\d+$/m, (m) => 'count: ' + ((parseInt(m.replace(/^count:\s*/, ''), 10) || 0) + 1)) + qa;
      await api('/api/file', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ path: sessionPath, content: next }),
      });
      sbFingerprint = '';
      paintSidebar();
    } catch (e) { /* 后台写入失败静默（回答仍在缓冲/历史界面可见） */ }
  }
  // 会话关闭前尽力写一次（best-effort，不 await；标记 archived，摘要/未决检测走 idle 或 /clear 路径）
  window.addEventListener('beforeunload', () => { writeSessionFile(true).catch(() => {}); });
  window.addEventListener('pagehide', () => { writeSessionFile(true).catch(() => {}); });

  // ---------- 会话管理（A2 列表 / A3 恢复） ----------
  async function sessionsCmd() {
    const r = await api('/api/sessions').catch(() => null);
    if (!r || !r.sessions || !r.sessions.length) {
      term.writeln('\x1b[33m暂无历史会话（问答防抖落盘 kb/sessions/，/clear 或关闭页面时归档）\x1b[0m');
      return;
    }
    term.writeln('\x1b[1;36m──── 历史会话（' + r.total + '）────\x1b[0m');
    for (const s of r.sessions) {
      const st = s.status === 'active' ? '\x1b[32m●\x1b[0m' : '\x1b[90m○\x1b[0m';
      const d = s.date || (s.mtime ? new Date(s.mtime * 1000).toISOString().slice(0, 10) : '-');
      term.writeln(' ' + st + ' ' + s.id + '  ' + (s.title || '(无标题)') + '\x1b[90m  [' + s.count + ' 轮 · ' + d + ']\x1b[0m');
    }
    term.writeln('\x1b[90m恢复: /resume <id 或标题关键词> · 面板: /view sessions\x1b[0m');
  }

  // 恢复会话 = 切换到该对话页：清屏并把历史 Q/A 渲染为消息流（用户气泡 + 助手气泡），之后可继续对话。
  // 历史内容为纯文本（可能含历史 ANSI），term.write 走 ansiToHtml（已 HTML 转义），安全。
  function renderHistory(parsed) {
    term.clear();
    for (const p of parsed) {
      if (String(p.q || '').trim()) {
        term.beginMsg('user');
        term.appendCard(escHtml(p.q));
        term.endMsg();
      }
      const a = String(p.a || '').trim();
      if (a && a !== '(无回答/中断)') {
        term.beginMsg('assistant');
        term.write(a);
        term.endMsg(true);
      }
    }
    if (!parsed.length) term.writeln('（空会话，无内容可显示）');
  }

  async function resumeCmd(arg) {
    if (!arg) {
      term.writeln('\x1b[33m用法: /resume <会话 id 或标题关键词>\x1b[0m（先 /sessions 查看列表）');
      return;
    }
    // 跨项目查找：会话归属项目，恢复时自动驱动项目上下文（使用哪个项目的对话就操作哪个项目）
    const r = await api('/api/sessions?all=1').catch(() => null);
    const groups = (r && r.projects) || [];
    let hit = null, projId = null;
    for (const g of groups) {
      const s = (g.sessions || []).find((x) => x.id === arg) ||
        (g.sessions || []).find((x) => x.title && x.title.includes(arg));
      if (s) { hit = s; projId = g.id; break; }
    }
    if (!hit) {
      term.writeln('\x1b[33m未找到会话: ' + arg + '\x1b[0m（/sessions 查看列表）');
      return;
    }
    // 跨项目会话：先切到所属项目（switchProject 自带重置）；同项目幂等跳过
    if ((projId || null) !== currentProject) {
      const g = groups.find((x) => (x.id || null) === (projId || null));
      await switchProject(projId, g ? g.name : '个人空间');
    }
    const f = await api('/api/file?path=sessions/' + encodeURIComponent(hit.id + '.md'));
    const parsed = Core.parseSessionFile((f && f.content) || '');
    if (!parsed.length) {
      term.writeln('\x1b[33m会话文件无有效问答对: ' + hit.id + '\x1b[0m');
      return;
    }
    closeView(); // 切换对话页语义：先关掉详情页（自动化/图谱等），否则视图盖着聊天区 = 点了切不过去
    // A3 恢复=新会话语义：载入 history（工作窗口内）+ 重置分节缓存（方向 3 memo 不沿用旧值）
    history = [];
    for (const p of parsed) {
      history.push({ role: 'user', content: p.q });
      if (p.a) history.push({ role: 'assistant', content: p.a });
    }
    history = history.slice(-MAX_HISTORY);
    Core.resetSectionCache();
    saveHistory();
    logActivity('session', '恢复会话 ' + hit.id + '（' + parsed.length + ' 轮）', { id: hit.id });
    // 真会话绑定：本会话文件 = 被恢复会话；sessionLog 载入历史 Q/A（后续快照续写同文件，不丢旧内容）
    sessionFile = 'sessions/' + hit.id + '.md';
    sessionLog = parsed.map((p) => ({ q: p.q, a: p.a || '(无回答/中断)', ts: Date.now() })).slice(-MAX_SESSION_LOG);
    sessionArchived = false;
    sessionTaskId = hit.task ? String(hit.task) : null; // 任务驱动会话：恢复时还原任务绑定
    // 切换对话页语义：清屏 + 渲染历史 Q/A 到消息流（像打开了那个对话页），继续提问即续写
    renderHistory(parsed);
    term.writeln('\x1b[32m✓ 已切换至历史对话：' + (hit.title || hit.id) + '（' + parsed.length + ' 轮）\x1b[0m' + (sessionTaskId ? ' · 任务 #' + sessionTaskId : ''));
    term.writeln('\x1b[90m继续提问即续写此对话；/clear 开始新对话\x1b[0m');
    if (topbarTitle) topbarTitle.textContent = String(hit.title || hit.id).slice(0, 30);
    // 会话后台执行挂接：切到的会话若有未完成执行 → 重放缓冲继续渲染；原会话执行转后台
    suspendExe();
    const exe = exes[sessionFile];
    if (exe && !exe.done) activateExe(exe);
    busy = currentExeBusy();
    setBusyUI();
  }

  // ---------- 会话收尾归档（A4 自动归档 + B3 未决决策，合并落地） ----------
  // 触发器：7 天无活动 / /clear（beforeunload 只标记 archived，不生成摘要）
  // 调研结论：30min 是 Web 连接级空闲超时标准值，对本地知识库无意义；会话是持久线程，
  // 归档阈值取 7 天（业界无自动收尾惯例，快照常驻、标记 + 摘要可逆可续）
  const SESSION_IDLE_MS = 7 * 24 * 60 * 60 * 1000;
  let lastActivityAt = Date.now();
  let sessionArchived = false;
  function touchActivity() { lastActivityAt = Date.now(); }
  setInterval(() => {
    if (!sessionArchived && sessionLog.length && Date.now() - lastActivityAt > SESSION_IDLE_MS) {
      term.writeln('\x1b[90m(7 天无操作 → 会话收尾归档)\x1b[0m');
      archiveSession();
    }
  }, 60000);

  // 未决规则粗筛（零 LLM，B3 降级）：回答含建议/方案类措辞 → 候选未决
  function ruleDetectUndecided(log) {
    const KW = /建议|方案|可选|推荐|可以考虑/;
    const out = [];
    for (const s of log) {
      if (KW.test(s.a || '')) out.push({ topic: String(s.q || '').slice(0, 60), advice: String(s.a || '').slice(0, 200) });
    }
    return out;
  }

  // 收尾归档：1) 快照 status=archived  2) 摘要落 notes/会话归档/（进可检索层）  3) 未决提案进 pending/
  async function archiveSession() {
    if (sessionArchived || !sessionLog.length) return;
    sessionArchived = true;
    const id = (sessionFile || ('sessions/' + sessionStamp() + '.md')).replace('sessions/', '').replace(/\.md$/, '');
    const title = String((sessionLog[0] && sessionLog[0].q) || '').slice(0, 30);
    try {
      await writeSessionFile(true); // status=archived
      const qa = sessionLog.map((s) => 'Q: ' + String(s.q).slice(0, 200) + '\nA: ' + String(s.a || '(无回答)').slice(0, 1500)).join('\n\n').slice(0, 8000);
      // LLM 一次收尾调用（摘要 + 未决检测）；失败/未配置 → 规则降级
      let summary = null;
      let undecided = [];
      if (llmConfigured) {
        try {
          const r = await fetch('/api/llm', {
            method: 'POST', headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              messages: [{
                role: 'user',
                content: '请对以下会话做收尾归档，只输出 JSON（不要其它文字）：\n' +
                  '{"summary":"会话要点（3-5 条，含关键决策与已沉淀记忆的 [[双链]]）","undecided":[{"topic":"议题","advice":"上次给出的方案/建议"}]}\n' +
                  'undecided 为空数组表示无未决议题；只有「给了建议但用户未拍板」的才列入。\n\n' + qa,
              }],
              stream: false,
            }),
            signal: AbortSignal.timeout(120000),
          });
          const body = await r.json();
          const full = (body.choices && body.choices[0] && body.choices[0].message && body.choices[0].message.content) || '';
          const j = Core.extractJsonObjects(full)[0];
          if (j) { summary = j.summary ? String(j.summary) : null; undecided = Array.isArray(j.undecided) ? j.undecided : []; }
        } catch (e) { /* LLM 失败 → 规则降级 */ }
      }
      if (!summary) summary = sessionLog.map((s) => '- ' + String(s.q).slice(0, 40)).join('\n');
      if (!undecided.length && !llmConfigured) undecided = ruleDetectUndecided(sessionLog);
      // 摘要落 notes/会话归档/<date>-<id>.md（派生产物自动落盘，进可检索层，无人审）
      const ar = 'notes/会话归档/' + localToday() + '-' + id + '.md';
      await api('/api/file', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ path: ar, content: '---\ntype: session-archive\ndate: ' + localToday() + '\nsource: sessions/' + id + '.md\n---\n\n# 会话归档：' + title + '\n\n' + summary + '\n' }),
      });
      // 未决提案进 pending/DECISION.<id>-<n>.md（待审人审；审批路由见后端 DECISION. 分支）
      let n = 0;
      for (const u of undecided.slice(0, 3)) {
        const topic = String(u.topic || '').trim();
        if (!topic) continue;
        n++;
        const content = '---\ntype: decision\nsource: sessions/' + id + '.md\ndate: ' + localToday() + '\n---\n\n## 议题：' + topic + '\n上次方案：' + String(u.advice || '').slice(0, 300) + '\n';
        await api('/api/file', {
          method: 'POST', headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ path: 'pending/DECISION.' + id + '-' + n + '.md', content }),
        }).catch(() => {});
      }
      term.writeln('\x1b[90m(已归档 → ' + ar + (n ? ' · 未决提案 ' + n + ' 条进待审' : '') + ')\x1b[0m');
    } catch (e) {
      term.writeln('\x1b[33m归档失败: ' + ((e && e.message) || e) + '\x1b[0m');
    }
    sessionLog = [];
  }

  // ---------- C3 代码提案应用（/dev apply） ----------
  async function devCmd(rest) {
    const [sub, ...args] = rest;
    if (sub === 'apply') {
      if (!args[0]) { term.writeln('\x1b[33m用法: /dev apply <提案路径>（如 pending/code/xxx.md，先 /pending 查看）\x1b[0m'); return; }
      term.writeln('\x1b[90m(应用代码提案 + cargo build 验证，失败自动回滚...)\x1b[0m');
      const r = await api('/api/dev/apply', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ path: args[0] }),
      });
      if (r.ok) term.writeln('\x1b[32m✓ 已应用 ' + (r.applied || []).join(', ') + '，构建验证通过\x1b[0m');
      else term.writeln('\x1b[31m应用失败已回滚: ' + (r.error || r.build || '未知') + '\x1b[0m');
    } else if (sub === 'patch') {
      term.writeln('\x1b[90m/dev patch 由 LLM 工具调用（dev.patch）生成提案；/dev apply <路径> 应用\x1b[0m');
    } else {
      term.writeln('\x1b[90m/dev apply <提案路径>  应用代码提案 + cargo build 验证（失败自动回滚）\x1b[0m');
    }
  }

  function loadHistory() {
    try {
      const h = JSON.parse(localStorage.getItem('md-agent-history') || '[]');
      return Array.isArray(h) ? h.slice(-MAX_HISTORY) : [];
    } catch (e) { return []; }
  }
  function saveHistory() {
    try { localStorage.setItem('md-agent-history', JSON.stringify(history.slice(-MAX_HISTORY))); } catch (e) { /* 忽略 */ }
  }

  // ---- 流内输入框 + 状态行（输入框在内容末尾：上边框/输入行/下边框/状态行 4 行结构）----
  let line = '';                       // 当前输入行内容
  let statusLine = '';                 // 状态栏最新文本（可含 ANSI）
  let atPrompt = false;                // 光标是否停在输入框（决定状态栏能否原地重绘）
  function dispW(s) {
    let w = 0;
    for (const ch of String(s)) {
      w += /[\u1100-\u115F\u2E80-\uA4CF\uAC00-\uD7A3\uF900-\uFAFF\uFE30-\uFE4F\uFF00-\uFF60\uFFE0-\uFFE6]/.test(ch) ? 2 : 1;
    }
    return w;
  }
  function visOnly(s) { return s.replace(/\x1b\[[0-9;]*m/g, ''); }
  function truncateW(s, maxW) {
    const plain = visOnly(s);
    if (dispW(plain) <= maxW) return s;
    let out = ''; let w = 0;
    for (const ch of Array.from(plain)) {
      const cw = dispW(ch);
      if (w + cw > maxW - 1) break;
      out += ch; w += cw;
    }
    return out + '\u2026';
  }
  // 重绘状态行（DOM 输入块：直接写状态行元素）
  function drawStatusRow() {
    if (atPrompt && statusLine) term.setStatus(statusLine.replace(/\x1b\[[0-9;]*m/g, ''));
  }
  // 输入框（输入中）：DOM 输入条常驻底部（DeepSeek 式），输入值/状态直接落 DOM
  // 空状态切换：新对话时输入框居中偏下；首条消息后回底部
  const mainEl = document.getElementById('main');
  function setEmptyState(empty) {
    if (empty) mainEl.classList.add('empty-state');
    else mainEl.classList.remove('empty-state');
  }
  function showPrompt() {
    atPrompt = true;
    term._input.value = line;
    term.autogrow();
    term.setStatus((statusLine || '').replace(/\x1b\[[0-9;]*m/g, ''));
    term.focus();
  }
// 回车提交：提交内容渲染为用户气泡（.msg.user），输入条清空
  function submitMsg() {
    atPrompt = false;
    setEmptyState(false); // 首条消息 → 输入框回底部
    compClose();
    term.beginMsg('user');
    term.appendCard(escHtml(line));
    term.endMsg();
    line = '';
    term._input.value = '';
    term.autogrow();
    clearDraft();
  }
  // 重绘输入行（退格/历史/补全用）：直接落输入元素
  function redrawInput() { term._input.value = line; term.autogrow(); saveDraft(); }
  // DOM 转义（消息区块渲染用）
  function escHtml(s) {
    return String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  }
  // 输入草稿：未提交输入防抖存 localStorage，刷新恢复（saveDraft 由 redrawInput/追加式输入触发）
  let draftTimer = null;
  function saveDraft() {
    clearTimeout(draftTimer);
    draftTimer = setTimeout(() => { try { localStorage.setItem('md-agent-draft', line); } catch (e) { /* 忽略 */ } }, 500);
  }
  function clearDraft() {
    clearTimeout(draftTimer);
    try { localStorage.removeItem('md-agent-draft'); } catch (e) { /* 忽略 */ }
  }
  const quickBtns = [...document.querySelectorAll('#sb-menu button[data-cmd]')];

  let busy = false;          // 命令/回答进行中（提交/按钮禁用，输入框仍可编辑）
  let cmdHistory = loadCmdHistory(); // 命令行历史（localStorage 持久化；区别于多轮对话 history）
  let histIdx = -1;
  function loadCmdHistory() {
    try {
      const h = JSON.parse(localStorage.getItem('md-agent-cmd-history') || '[]');
      return Array.isArray(h) ? h.slice(-100) : [];
    } catch (e) { return []; }
  }
  function saveCmdHistory() {
    try { localStorage.setItem('md-agent-cmd-history', JSON.stringify(cmdHistory.slice(-100))); } catch (e) { /* 忽略 */ }
  }
  let currentAbort = null;   // 当前 LLM 流的 AbortController（Ctrl+C / Esc 中断回答）

  // 终端实测列宽（消息块铺背景用；DOM 流按容器像素估算）
  function trueCols() {
    return Math.max(40, Math.floor((term.element.clientWidth - 32) / 8));
  }

  // ---------- 输入与提交 ----------

  // busy（回答/动作进行中）期间仍放行的导航类命令（读操作，静默执行、不污染回答流）
  const NAV_CMDS = ['/view', '/side', '/help', 'open'];
  function isNavCmd(t) {
    return NAV_CMDS.some((c) => t === c || t.startsWith(c + ' '));
  }

  // 提交一条命令/问题：流内整行背景块 + 执行；回答期间（busy）只放行导航命令（静默执行）
  async function submitCmd(text) {
    const t = String(text || '').trim();
    if (!t || cfCb) return;
    if (busy && !isNavCmd(t)) return;   // 回答中：非导航命令仍拦截
    if (busy) {                          // 回答中导航：不动 busy/输入框，回答流继续，视图静默打开
      try { await run(t); } catch (e) { term.writeln('\x1b[31m' + ((e && e.message) || e) + '\x1b[0m'); }
      return;
    }
    line = t;
    submitMsg();      // 流内提交块（边框移除 + 背景色 + 清状态行）
    line = '';
    clearDraft();     // 已提交，清草稿
    pushHistory(t);
    // C1 审视层触发：非命令输入含纠错措辞（用户纠正上轮回答 = 强信号，零 token 检测）
    if (!t.startsWith('/') && /不对|错了|不是这样|你错了|不是的/.test(t)) {
      touchExperience('correction', t);
    }
    busy = true;
    setBusyUI();
    try {
      await run(t);
    } catch (e) {
      term.writeln('\x1b[31m' + ((e && e.message) || e) + '\x1b[0m');
    } finally {
      busy = false;
      setBusyUI();
      showPrompt();   // 回答结束后重画输入框（上边框/输入行/下边框/状态行）
      updateTopTitle();
      refreshStatus();
    }
  }

  // 面板 → 宿主命令（市场「运行」/ 功能首页命令卡片）：与键盘提交同一输入条生命周期
  // （submitMsg 移除输入条 → 输出 → showPrompt 重建），避免 atPrompt 悬空导致状态行写进输出中间；
  // 与键盘路径差异：保留用户当前输入行草稿（line 不动）、不记命令历史。
  async function panelCmd(cmd) {
    const t = String(cmd || '').trim();
    if (!t || cfCb) return;
    if (busy && !isNavCmd(t)) return;   // 回答中：与键盘一致，仅放行导航命令
    if (busy) {
      try { await run(t); } catch (e) { term.writeln('\x1b[31m' + ((e && e.message) || e) + '\x1b[0m'); }
      return;
    }
    const savedLine = line;
    line = t;
    submitMsg();       // 输入条移除 + 画「md-agent> <cmd>」提交块（atPrompt=false）
    line = savedLine;  // 恢复草稿（不 pushHistory / clearDraft）
    busy = true;
    setBusyUI();
    try {
      await run(t);
    } catch (e) {
      term.writeln('\x1b[31m' + ((e && e.message) || e) + '\x1b[0m');
    } finally {
      busy = false;
      setBusyUI();
      showPrompt();
      updateTopTitle();
      refreshStatus();
    }
  }

  function setBusyUI() {
    // 按钮行已全导航化（读操作）：busy 期间不禁用，点击走 submitCmd 的导航放行分支
    quickBtns.forEach((b) => (b.disabled = false));
  }

  // 按键扩展：↑↓ 历史、Tab 命令补全、Ctrl+C 中断、Esc 关闭（原生 textarea，Enter 由 stream.js 转发；
  // 确认交互已改软弹窗 #cf-overlay，不再有终端 y/n 按键拦截）
  term.attachCustomKeyEventHandler((ev) => {
    const k = ev.key;
    // 补全下拉：↑↓/Tab 移动选择，Enter 选中，Esc 关闭
    if (compOpen && compItems.length) {
      if (k === 'ArrowUp' || k === 'ArrowDown' || k === 'Tab') {
        ev.preventDefault();
        compIdx = (compIdx + (k === 'ArrowUp' ? -1 : 1) + compItems.length) % compItems.length;
        compPaint();
        return false;
      }
      if (k === 'Enter') { ev.preventDefault(); compPick(compIdx); return false; }
      if (k === 'Escape') { ev.preventDefault(); compClose(); return false; }
    }
    // ↑↓ 历史：仅输入为空时（原生 textarea 多行时光标移动优先）
    if ((k === 'ArrowUp' || k === 'ArrowDown') && !line.trim()) {
      ev.preventDefault();
      navHistory(k === 'ArrowUp' ? 1 : -1);
      return false;
    }
    if (k === 'Tab') {
      ev.preventDefault();
      if (!compOpen) domComplete();
      else { compIdx = (compIdx + 1) % compItems.length; compPaint(); }
      return false;
    }
    if (k === 'c' && ev.ctrlKey && currentAbort) {
      ev.preventDefault();
      currentAbort.abort();
      term.writeln('\x1b[90m(已发送中断)\x1b[0m');
      return false;
    }
    // Ctrl+K = 速览侧边栏（若被浏览器全局快捷键占用，可用 /side 或快捷按钮）
    if (k === 'k' && ev.ctrlKey) {
      ev.preventDefault();
      toggleSide();
      return false;
    }
    // Esc = 关闭速览抽屉（优先）或 停止回答（回流内后无 DOM 发送/停止按钮）
    if (k === 'Escape') {
      if (!sideDrawer.classList.contains('hidden')) {
        ev.preventDefault();
        closeSide();
        return false;
      }
      if (currentAbort) {
        ev.preventDefault();
        currentAbort.abort();
        term.writeln('\x1b[90m(已停止)\x1b[0m');
        return false;
      }
    }
    return true;
  });
  quickBtns.forEach((btn) => btn.addEventListener('click', () => submitCmd(btn.dataset.cmd)));

  // ---------- 补全下拉（锚定输入条上方） ----------
  let compOpen = false;
  let compItems = [];
  let compIdx = 0;
  const compEl = document.getElementById('completion');
  function compShow(list) {
    compItems = list;
    compIdx = 0;
    compOpen = true;
    compEl.innerHTML = '';
    list.forEach((it, i) => {
      const d = document.createElement('div');
      d.className = 'comp-item' + (i === compIdx ? ' sel' : '');
      d.textContent = it;
      d.addEventListener('mousedown', (e) => { e.preventDefault(); compPick(i); });
      d.addEventListener('mouseenter', () => { compIdx = i; compPaint(); });
      compEl.appendChild(d);
    });
    // 定位：输入条上方（左侧锚定输入条左缘）
    const ib = document.getElementById('ds-input-wrap') || document.querySelector('#input-bar .i-row');
    const r = ib.getBoundingClientRect();
    compEl.style.bottom = (window.innerHeight - r.top + 8) + 'px';
    compEl.style.left = Math.max(12, r.left) + 'px';
    compEl.classList.add('open');
  }
  function compPaint() {
    compEl.querySelectorAll('.comp-item').forEach((d, i) => d.classList.toggle('sel', i === compIdx));
  }
  function compPick(i) {
    if (compItems[i] !== undefined) {
      line = compItems[i];
      redrawInput();
    }
    compClose();
    term.focus();
  }
  function compClose() {
    compOpen = false;
    compEl.classList.remove('open');
  }
  async function domComplete() {
    const m = line.match(/(^|\s)@([^\s]*)$/);
    if (m) {
      const docs = await loadAtDocs();
      const kw = m[2].toLowerCase();
      const list = docs.filter((p) => p.toLowerCase().includes(kw));
      if (list.length) compShow(list.map((p) => line.slice(0, m.index + m[1].length) + '@' + p));
      return;
    }
    const head = line.match(/^(\/[^\s]*)/);
    let list = [];
    if (head) list = COMMANDS.map(([c]) => c).filter((c) => c.startsWith(head[1]) && c !== head[1]);
    if (list.length) compShow(list);
  }
  document.addEventListener('mousedown', (e) => {
    if (compOpen && !compEl.contains(e.target)) compClose();
  });

  // ---------- 发送/停止按钮（busy 互换，复用 currentAbort；DeepSeek 式） ----------
  const sendBtn = document.getElementById('send-btn');
  function setStopBtn(show) {
    if (!sendBtn) return;
    sendBtn.textContent = show ? '■' : '↑';
    sendBtn.classList.toggle('stop', show);
    sendBtn.title = show ? '停止回答' : '发送 (Enter)';
  }
  if (sendBtn) {
    sendBtn.addEventListener('click', () => {
      if (currentAbort) {
        currentAbort.abort();
        term.writeln('\x1b[90m(已停止)\x1b[0m');
      } else {
        const cmd = line.trim();
        if (cmd) submitCmd(cmd);
      }
    });
  }

  // ---------- 文档摄入（附件按钮）：选文件 → dry-run 预览 → y/n 确认落盘 ----------
  const attachBtn = document.getElementById('attach-btn');
  const attachInput = document.getElementById('attach-file');
  if (attachBtn && attachInput) {
    attachBtn.addEventListener('click', () => attachInput.click());
    attachInput.addEventListener('change', () => {
      const f = attachInput.files && attachInput.files[0];
      attachInput.value = '';
      if (!f) return;
      ingestFile(f);
    });
  }

  async function ingestFile(f) {
    term.writeln('\x1b[90m(正在转换 ' + f.name + '，' + (f.size / 1024).toFixed(0) + ' KB ...)\x1b[0m');
    let b64;
    try {
      const buf = await f.arrayBuffer();
      const bytes = new Uint8Array(buf);
      let bin = '';
      const CH = 0x8000;
      for (let i = 0; i < bytes.length; i += CH) {
        bin += String.fromCharCode.apply(null, bytes.subarray(i, i + CH));
      }
      b64 = btoa(bin);
    } catch (e) {
      term.writeln('\x1b[31m读取文件失败: ' + ((e && e.message) || e) + '\x1b[0m');
      return;
    }
    let preview;
    try {
      preview = await api('/api/ingest', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: f.name, content_base64: b64, dry_run: true }),
      });
    } catch (e) {
      term.writeln('\x1b[31m摄入失败: ' + ((e && e.message) || e) + '\x1b[0m');
      return;
    }
    const md = preview.markdown || '';
    const lines = md.split('\n');
    const shown = lines.slice(0, 40).join('\n');
    term.writeln('\x1b[90m──── 转换预览（' + lines.length + ' 行 · 前 40 行）────\x1b[0m');
    term.writeln(shown || '\x1b[90m(空内容)\x1b[0m');
    const ok = await confirm('摄入到知识库 notes/？');
    if (!ok) return;
    term.writeln('\x1b[90m(正在落盘并重建索引...)\x1b[0m');
    try {
      const r = await api('/api/ingest', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: f.name, content_base64: b64, dry_run: false }),
      });
      term.writeln('\x1b[32m✓ 已摄入 → ' + r.path + '\x1b[0m');
      term.writeln('\x1b[90m可检索: /search <关键词> · 图谱: /view graph\x1b[0m');
    } catch (e) {
      term.writeln('\x1b[31m落盘失败: ' + ((e && e.message) || e) + '\x1b[0m');
    }
  }

  // ---------- 左侧会话边栏（DeepSeek 式：新建/搜索/恢复/归档；E1：标签条版改名 archiveSessionStatus，
  //            不再覆盖 137 行完整归档版 archiveSession——/clear 与 7 天空闲归档随之复活） ----------
  const sbList = document.getElementById('sb-list');
  const sbSearchEl = document.getElementById('sb-search');
  let sbSearchTxt = '';
  let sbSearchTimer = null;
  let sbCache = null;        // /api/sessions lite 列表缓存（8s 轮询指纹对比，变化才重绘——R6）
  let sbFingerprint = '';
  // 单会话归档（侧边栏 ×）：只翻 status 字段（不覆盖完整归档版）；projId 非空时跨项目操作
  async function archiveSessionStatus(id, projId) {
    const hdrs = projId ? { 'X-Project': projId } : {};
    const f = await api('/api/file?path=sessions/' + encodeURIComponent(id + '.md'), { headers: hdrs }).catch(() => null);
    if (!f || !f.content) return;
    const next = f.content.replace(/^(status:\s*)active$/m, '$1archived');
    if (next === f.content) return;
    await api('/api/file', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...hdrs },
      body: JSON.stringify({ path: 'sessions/' + id + '.md', content: next }),
    }).catch(() => {});
    // 检索化：× 轻归档也落规则摘要到 notes/会话归档/（可检索层；LLM 完整版已存在则跳过，不覆盖）
    try {
      const ar = 'notes/会话归档/' + localToday() + '-' + id + '.md';
      const exists = await api('/api/file?path=' + encodeURIComponent(ar), { headers: hdrs }).catch(() => null);
      if (!exists || !exists.content) {
        const qs = [...f.content.matchAll(/## Q: ([^\n]*)/g)].map((m) => '- ' + String(m[1]).slice(0, 40)).join('\n');
        const title = ((f.content.match(/^title: (.+)$/m) || [])[1] || id).trim();
        if (qs) {
          await api('/api/file', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', ...hdrs },
            body: JSON.stringify({
              path: ar,
              content: '---\ntype: session-archive\ndate: ' + localToday() + '\nsource: sessions/' + id + '.md\n---\n\n# 会话归档：' + title + '\n\n' + qs + '\n',
            }),
          }).catch(() => {});
        }
      }
    } catch (e) { /* 摘要落盘失败不影响归档本身 */ }
  }
  // 会话重命名（豆包式 ⋯ 菜单）：改 frontmatter title（纯前端，POST /api/file）；projId 非空时跨项目操作
  async function renameSession(id, oldTitle, projId) {
    const t = prompt('重命名会话（' + id + '）：', String(oldTitle || ''));
    if (t === null || !t.trim() || t.trim() === String(oldTitle || '')) return;
    const hdrs = projId ? { 'X-Project': projId } : {};
    const f = await api('/api/file?path=sessions/' + encodeURIComponent(id + '.md'), { headers: hdrs }).catch(() => null);
    if (!f || !f.content) return;
    const next = f.content.replace(/^(title:\s*).*$/m, '$1' + t.trim().slice(0, 30));
    if (next === f.content) return;
    await api('/api/file', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...hdrs },
      body: JSON.stringify({ path: 'sessions/' + id + '.md', content: next }),
    }).catch(() => {});
    sbFingerprint = '';
    paintSidebar();
    logActivity('session', '重命名会话 ' + id, { id });
  }
  function renderSidebar(r) {
    const all = (r && r.sessions) || [];
    const kw = sbSearchTxt.trim().toLowerCase();
    // 会话归属项目：数据 = 全项目分组（个人空间 = 默认项目）；兼容旧单项目响应
    const groups = (r && r.projects && r.projects.length)
      ? r.projects
      : [{ id: null, name: '个人空间', is_default: true, sessions: all }];
    const curId = (sessionFile || '').replace('sessions/', '').replace(/\.md$/, '');
    sbList.innerHTML = '';
    // 首问会话文件未落盘（列表无对应行）且有执行 → 顶部指示条
    if (exes[null] && !exes[null].done) {
      const runBar = document.createElement('div');
      runBar.className = 'sb-running';
      runBar.innerHTML = '<span class="run-dot"></span> AI 处理中…';
      sbList.appendChild(runBar);
    }
    const isCur = (g) => (g.id || null) === currentProject;
    let anySession = false;
    for (const g of groups) {
      let list = g.sessions || [];
      if (kw) {
        list = list.filter((s) => String(s.title || '').toLowerCase().includes(kw) || s.id.includes(kw));
      }
      const active = list.filter((s) => s.status === 'active');
      const archived = list.filter((s) => s.status !== 'active');
      if (!list.length && !kw && !isCur(g)) continue; // 空项目折叠（当前项目始终显示）
      if (list.length) anySession = true;
      // 项目组头：点击 = 在该项目开新会话（会话驱动项目，无「切换」概念）
      const head = document.createElement('div');
      head.className = 'sb-group sb-proj-head' + (isCur(g) ? ' cur' : '');
      head.title = '在该项目新建对话（点击切换）';
      head.textContent = (isCur(g) ? '▶ ' : '') + (g.name || '个人空间') + ' · ' + list.length;
      head.addEventListener('click', () => startNewInProject(g.id));
      sbList.appendChild(head);
      const group = (items, label) => {
        if (!items.length) return;
        const h = document.createElement('div');
        h.className = 'sb-group';
        h.textContent = label;
        sbList.appendChild(h);
        for (const s of items.slice(0, 60)) {
          const it = document.createElement('div');
          const exeRun = exes['sessions/' + s.id + '.md'] && !exes['sessions/' + s.id + '.md'].done;
          it.className = 'sb-item' + (s.id === curId ? ' current' : '') + (exeRun ? ' running' : '');
          const t = document.createElement('span');
          t.className = 'sb-item-title';
          t.textContent = String(s.title || s.id).slice(0, 24);
          t.title = s.id;
          const d = document.createElement('span');
          d.className = 'sb-item-meta';
          d.textContent = (s.count || 0) + ' 轮 · ' + relTimeOf(s);
          if (exeRun) {
            // 该会话有未完成执行：运行点（思考/工具/回答全过程；后台执行也标）
            const run = document.createElement('span');
            run.className = 'run-dot';
            run.title = 'AI 处理中';
            it.appendChild(run);
          }
          const x = document.createElement('span');
          x.className = 'sb-item-x';
          x.textContent = '×';
          x.title = '归档会话';
          x.addEventListener('click', (ev) => {
            ev.stopPropagation();
            // 归档可逆（进会话列表随时可恢复），免确认直接归档；跨项目操作带目标项目头
            archiveSessionStatus(s.id, g.id).then(() => { sbFingerprint = ''; paintSidebar(); });
          });
          const more = document.createElement('span');
          more.className = 'sb-item-more';
          more.textContent = '⋯';
          more.title = '重命名会话';
          more.addEventListener('click', (ev) => {
            ev.stopPropagation();
            renameSession(s.id, s.title, g.id);
          });
          const del = document.createElement('span');
          del.className = 'sb-item-del';
          del.textContent = '🗑';
          del.title = '删除会话（不可恢复）';
          del.addEventListener('click', async (ev) => {
            ev.stopPropagation();
            // 删除不可恢复 → 软弹窗确认（danger 红色）
            if (!await uiConfirm('删除会话 ' + s.id + '？（不可恢复）', { danger: true })) return;
            await api('/api/file?path=sessions/' + encodeURIComponent(s.id + '.md'),
              { method: 'DELETE', headers: g.id ? { 'X-Project': g.id } : {} }).catch(() => {});
            sbFingerprint = '';
            paintSidebar();
          });
          it.appendChild(t);
          it.appendChild(d);
          it.appendChild(more);
          it.appendChild(del);
          it.appendChild(x);
          it.addEventListener('click', () => {
            // 会话驱动项目：恢复该会话（跨项目自动切换上下文）
            resumeSession(g.id, s.id).then(() => { sbFingerprint = ''; paintSidebar(); });
          });
          sbList.appendChild(it);
        }
      };
      group(active, '进行中');
      group(archived, '会话列表');
    }
    if (!anySession && !kw) {
      const e = document.createElement('div');
      e.className = 'sb-empty';
      e.textContent = '暂无会话 · 提问后自动创建';
      sbList.appendChild(e);
    } else if (kw && !anySession) {
      const e = document.createElement('div');
      e.className = 'sb-empty';
      e.textContent = '无匹配会话';
      sbList.appendChild(e);
    }
    // 底部：新建项目入口
    const np = document.createElement('div');
    np.className = 'sb-item sb-project-new';
    np.title = '新建项目（独立知识空间）';
    np.textContent = '＋ 新建项目';
    np.addEventListener('click', () => openNewProjectWizard());
    sbList.appendChild(np);
  }
  function paintSidebar() {
    if (!sbCache) {
      api('/api/sessions?all=1').catch(() => null).then((r) => { sbCache = r || { projects: [] }; renderSidebar(sbCache); });
      return;
    }
    renderSidebar(sbCache);
  }
  // 8s 轮询：全项目会话指纹变化（新建/归档/计数）才重绘，避免打断 hover（R6）
  async function paintSidebarIfChanged() {
    const r = await api('/api/sessions?all=1').catch(() => null);
    if (!r) return;
    const fp = (r.projects || []).map((g) =>
      (g.id || '') + ':' + (g.sessions || []).map((s) => s.id + ':' + s.status + ':' + s.count).join(',')
    ).join('|');
    if (fp !== sbFingerprint) { sbFingerprint = fp; sbCache = r; renderSidebar(sbCache); }
  }
  // 恢复会话（跨项目）：先切到会话所属项目（switchProject 自带重置），再载入历史
  async function resumeSession(projId, sid) {
    if ((projId || null) !== currentProject) {
      const g = ((sbCache && sbCache.projects) || []).find((x) => (x.id || null) === (projId || null));
      await switchProject(projId, g ? g.name : '个人空间');
    }
    await resumeCmd(sid);
  }
  // 在某项目开新会话（组头/新对话选择/顶栏 chip 共用）：同项目 = 清空；跨项目 = 切项目（自带重置）
  function startNewInProject(id) {
    const g = ((sbCache && sbCache.projects) || []).find((x) => (x.id || null) === (id || null));
    const name = g ? g.name : '个人空间';
    closeProjectMenu();
    closeView(); // 先回聊天区（新对话 = 回到对话主界面）
    if ((id || null) === currentProject) submitCmd('/clear');
    else switchProject(id, name);
    setTimeout(() => { sbFingerprint = ''; paintSidebar(); }, 800);
  }
  if (sbSearchEl) {
    sbSearchEl.addEventListener('input', (e) => {
      sbSearchTxt = e.target.value;
      clearTimeout(sbSearchTimer);
      sbSearchTimer = setTimeout(paintSidebar, 150);
    });
  }
  document.getElementById('session-new').addEventListener('click', (ev) => {
    // 新对话：直接在当前项目开新会话（不弹项目选择子菜单）
    ev.stopPropagation();
    closeProjectMenu();
    closeView();
    if (sessionLog.length && !sessionArchived) archiveSession();
    if (currentAbort) { try { currentAbort.abort(); } catch (e) { /* 忽略 */ } currentAbort = null; }
    for (const k in exes) exes[k].done = true; activeExe = null;
    history = []; saveHistory(); Core.resetSectionCache();
    sessionFile = null; sessionLog = []; sessionArchived = false; sessionTaskId = null;
    if (topbarTitle) topbarTitle.textContent = '新对话';
    term.clear();
    printBanner();
    setEmptyState(true);
    refreshStatus();
    setTimeout(() => { sbFingerprint = ''; paintSidebar(); }, 400);
  });
  paintSidebar();

  // ---------- 顶栏：会话标题 + 模型芯片 ----------
  const topbarTitle = document.getElementById('topbar-title');
  const modelChip = document.getElementById('model-chip');
  function updateTopTitle() {
    if (!topbarTitle) return;
    const q = sessionLog[0] && sessionLog[0].q;
    topbarTitle.textContent = q ? String(q).slice(0, 30) : '新对话';
  }
  function updateModelChip(model, endpoint) {
    if (!modelChip) return;
    modelChip.textContent = model || '未配置 LLM';
    modelChip.classList.toggle('nok', !model || !endpoint);
    modelChip.title = endpoint
      ? (endpoint + ' · 点击打开配置页')
      : '未配置模型服务 · 点击打开配置页';
  }
  if (modelChip) modelChip.addEventListener('click', () => window.open('/config.html', '_blank'));

  // ---------- 输入历史（会话内，空输入时 ArrowUp/Down 翻动） ----------
  function pushHistory(t) {
    if (cmdHistory[cmdHistory.length - 1] === t) return;
    cmdHistory.push(t);
    if (cmdHistory.length > 100) cmdHistory.shift();
    saveCmdHistory();
    histIdx = -1;
  }
  function navHistory(dir) {
    if (!cmdHistory.length) return;
    if (dir > 0) { // 上（更早）
      if (histIdx === -1) histIdx = cmdHistory.length - 1;
      else if (histIdx > 0) histIdx--;
    } else {       // 下（更新）
      if (histIdx === -1) return;
      histIdx++;
      if (histIdx >= cmdHistory.length) { histIdx = -1; line = ''; redrawInput(); return; }
    }
    line = cmdHistory[histIdx] || '';
    redrawInput();
  }

  // ---------- 命令补全面板（界面优先：仅保留常用命令；开发者命令 /l1 /diff /health /open /projects 已移出） ----------
  const COMMANDS = [
    ['/help', '使用指引'], ['/search', '检索双层库'], ['/spaces', '项目空间'],
    ['/sync', '重建 INDEX.md'], ['/digest', '整理新笔记'],
    ['/remember', '手动沉淀到记忆'], ['/graph', '知识图谱/关联簇'], ['/orphans', '孤立文档'],
    ['/tags', '标签统计'], ['/rescan', '重建知识图谱'],
    ['/pending', '查看待审'], ['/preview', '行级预览'], ['/approve', '批准待审'],
    ['/reject', '拒绝待审'], ['/view', '面板渲染层'], ['/audit', '知识库健康审计'], ['/risk', '风控预警（时效/证据/信息）'],
    ['/conflicts', '冲突检查'], ['/link', '补链接'],
    ['/link-all', '批量补链接'], ['/suggest', '补全缺失文档'], ['/fetch', '抓取网页'],
    ['/page', '动态网页读取'], ['/task', '任务引擎'], ['/market', '工作台/应用市场'], ['/clear', '清空多轮记忆'],
    ['/config', '查看配置'], ['/heartbeat', '心跳自动同步'], ['/newproject', '新建项目'],
    ['clear', '清屏'],
  ];
  // ---------- 命令补全（流内 Tab 循环；/ 开头才触发，无下拉浮层） ----------

  // ---------- @ 文件提及（Tab 循环；行尾 @ 触发，候选 = KB 文档路径，提交时注入检索目标） ----------
  let atDocs = null;  // @ 补全候选（/api/graph/graph nodes 路径，惰性加载会话内缓存）
  async function loadAtDocs() {
    if (atDocs) return atDocs;
    try {
      const g = await api('/api/graph/graph');
      atDocs = (g.nodes || []).map((n) => n.path);
    } catch (e) { atDocs = []; }
    return atDocs;
  }

  // ---- 启动欢迎横幅 + 建议 chips（DeepSeek 式；R2：状态行不再启动时拉 5 端点，
  //      改由 refreshStatus 8s 轮询填充——启动只需 1 个 config 请求，输入框不被 banner 阻塞）----
  let bannerDone = Promise.resolve();
  function printBanner() {
    const bannerRow = term.appendCard(
      '<div class="welcome">' +
        '<div class="w-hello">有什么我能帮你的吗？</div>' +
      '</div>'
    );
  }
  printBanner();
  setEmptyState(true); // 启动 = 新对话空状态

  // ---- 项目制（多项目硬隔离）：项目切换器 + 新建项目 ----
  function loadProjectState() {
    try {
      const saved = localStorage.getItem(PROJECT_KEY);
      currentProject = saved && saved !== 'default' ? saved : null;
    } catch (e) { currentProject = null; }
  }
  function templateIcon(tpl) {
    return tpl === 'lawyer' ? '⚖️' : tpl === 'headhunter' ? '🎯' : '📁';
  }
  async function refreshProjects() {
    const r = await api('/api/projects').catch(() => ({ projects: [] }));
    projectList = (r && r.projects) || [];
    if (currentProject && !projectList.some((p) => p.id === currentProject)) {
      currentProject = null; // 项目已删 → 回退个人空间
      try { localStorage.setItem(PROJECT_KEY, 'default'); } catch (e) {}
    }
    const cur = projectList.find((p) => p.id === currentProject);
    currentProjectName = cur ? cur.name : '个人空间';
    renderProjectChip();
    return projectList;
  }
  function renderProjectChip() {
    const chip = document.getElementById('project-chip');
    if (!chip) return;
    const cur = projectList.find((p) => p.id === currentProject);
    chip.textContent = (currentProject ? templateIcon(cur ? cur.template : '') : '🗂️') + ' ' + currentProjectName;
    // 同步更新输入框顶部项目入口
    const entry = document.getElementById('ds-project-entry');
    const info = document.querySelector('.ds-project-info');
    if (entry) {
      const ico = currentProject ? templateIcon(cur ? cur.template : '') : '🗂️';
      entry.querySelector('.ds-pe-ico').textContent = ico;
      entry.querySelector('.ds-pe-name').textContent = currentProjectName;
    }
    if (info) {
      info.style.display = 'none';
    }
  }
  function renderProjectMenu() {
    const list = document.getElementById('pm-list');
    if (!list) return;
    list.innerHTML = '';
    const mk = (id, name, icon, active, tag) => {
      const b = document.createElement('button');
      if (active) b.className = 'active';
      b.innerHTML = '<span class="pm-ico"></span><span class="pm-name"></span>' + (tag ? '<span class="pm-tag"></span>' : '');
      b.querySelector('.pm-ico').textContent = icon;
      b.querySelector('.pm-name').textContent = name;
      if (tag) b.querySelector('.pm-tag').textContent = tag;
      b.addEventListener('click', () => startNewInProject(id));
      list.appendChild(b);
    };
    mk(null, '个人空间', '🗂️', !currentProject, '默认');
    for (const p of projectList) mk(p.id, p.name, templateIcon(p.template), currentProject === p.id, '');
  }
  function toggleProjectMenu(force, anchorEl) {
    const menu = document.getElementById('project-menu');
    if (!menu) return;
    const show = force !== undefined ? force : menu.hidden;
    const entry = document.getElementById('ds-project-entry');
    if (show) {
      renderProjectMenu();
      if (anchorEl && anchorEl.getBoundingClientRect) {
        const r = anchorEl.getBoundingClientRect();
        menu.style.top = (r.bottom + 4) + 'px';
        menu.style.left = (r.left) + 'px';
        menu.style.right = 'auto';
      } else {
        const chip = document.getElementById('project-chip');
        const r = chip ? chip.getBoundingClientRect() : null;
        menu.style.top = ((r ? r.bottom : 48) + 6) + 'px';
        menu.style.right = '16px';
        menu.style.left = 'auto';
      }
      menu.hidden = false;
      if (entry) entry.classList.add('active');
    } else {
      menu.hidden = true;
      if (entry) entry.classList.remove('active');
    }
  }
  function closeProjectMenu() { toggleProjectMenu(false); }
  async function switchProject(id, name) {
    const target = id || null;
    if (target === currentProject) { closeProjectMenu(); return; }
    closeProjectMenu();
    try {
      if (sessionLog.length && !sessionArchived) await archiveSession(); // 旧会话归档进旧项目（await 确保落盘完成）
    } catch (e) { /* 归档失败不阻断切换 */ }
    // 切项目 = 换世界：打断所有会话的后台执行（含首问）
    if (currentAbort) { try { currentAbort.abort(); } catch (e) { /* 忽略 */ } currentAbort = null; }
    for (const k in exes) exes[k].done = true;
    activeExe = null;
    currentProject = target;
    try { localStorage.setItem(PROJECT_KEY, target || 'default'); } catch (e) {}
    const cur = projectList.find((p) => p.id === target);
    currentProjectName = cur ? cur.name : '个人空间';
    // 重置会话上下文（新项目新会话；分节缓存不复用旧项目前缀）
    history = []; saveHistory(); Core.resetSectionCache();
    sessionFile = null; sessionLog = []; sessionArchived = false;
    sbCache = null; sbFingerprint = '';
    if (topbarTitle) topbarTitle.textContent = '新对话';
    renderProjectChip();
    term.clear();
    printBanner();
    setEmptyState(true);
    paintSidebar();
    refreshStatus();
    busy = currentExeBusy();
    setBusyUI();
    logActivity('project', '切换项目 → ' + currentProjectName, { id: target });
  }
  // 新建项目向导：模板三卡（空白/律师/猎头）+ 命名 → 创建并进入
  const PROJECT_TEMPLATES = [
    { id: 'blank', icon: '📁', name: '空白项目', desc: '通用空间，自由组织' },
    { id: 'lawyer', icon: '⚖️', name: '律师案件', desc: '案件总览 · 证据清单 · 时间线 · 法律研究' },
    { id: 'headhunter', icon: '🎯', name: '猎头项目', desc: '职位需求 · 候选人台账 · 客户 · 沟通记录' },
  ];
  let npwSel = 'blank';
  function openNewProjectWizard() {
    closeProjectMenu();
    const box = document.getElementById('npw-overlay');
    if (!box) return;
    const wrap = document.getElementById('npw-tpls');
    wrap.innerHTML = '';
    npwSel = 'blank';
    for (const t of PROJECT_TEMPLATES) {
      const el = document.createElement('div');
      el.className = 'npw-tpl' + (t.id === npwSel ? ' sel' : '');
      el.dataset.tpl = t.id;
      el.innerHTML = '<div class="ti">' + t.icon + '</div><div class="tn">' + t.name + '</div><div class="td">' + t.desc + '</div>';
      el.addEventListener('click', () => {
        npwSel = t.id;
        wrap.querySelectorAll('.npw-tpl').forEach((x) => x.classList.toggle('sel', x.dataset.tpl === t.id));
      });
      wrap.appendChild(el);
    }
    const name = document.getElementById('npw-name');
    name.value = '';
    box.hidden = false;
    setTimeout(() => name.focus(), 50);
  }
  function closeNewProjectWizard() {
    const box = document.getElementById('npw-overlay');
    if (box) box.hidden = true;
  }

  // 命令参数弹窗（prefill 界面化）：面板点卡片 → 弹参 → run() 直接执行，不再预填命令到输入框。
  // 对非技术用户，这是"点按钮 + 填一个小输入框"的界面操作，不暴露斜杠命令
  const PREFILL_META = {
    '/search ':    { t: '检索知识库', d: '输入关键词（可多个，用空格分隔），从你的笔记里找相关内容', p: '例如：劳动仲裁 证据' },
    '/task plan ': { t: '任务拆解', d: '输入任务目标，AI 会把它拆解成可执行的任务链', p: '例如：整理本月客户回访' },
    '/fetch ':     { t: '抓取网页', d: '输入网页地址（可附标题），抓取内容并沉淀为笔记', p: '例如：https://example.com 行业报告' },
    '/page ':      { t: '动态网页读取', d: '输入网页地址，等待页面加载完成后读取内容', p: '例如：https://example.com' },
    '/suggest ':   { t: '补全缺失文档', d: '输入主题，AI 生成该主题的新笔记（先进入待审）', p: '例如：仲裁管辖' },
  };
  let ppPendingCmd = null;
  function openCmdPrompt(cmd) {
    const box = document.getElementById('pp-overlay');
    if (!box) return;
    const m = PREFILL_META[cmd] || { t: '输入参数', d: cmd || '', p: '' };
    ppPendingCmd = cmd || '';
    document.getElementById('pp-title').textContent = m.t || '输入参数';
    document.getElementById('pp-desc').textContent = m.d || '';
    const input = document.getElementById('pp-input');
    input.value = '';
    input.placeholder = m.p || '';
    box.hidden = false;
    setTimeout(() => input.focus(), 50);
  }
  function closeCmdPrompt() {
    const box = document.getElementById('pp-overlay');
    if (box) box.hidden = true;
  }
  function runCmdPrompt() {
    const input = document.getElementById('pp-input');
    const arg = (input.value || '').trim();
    const cmd = ppPendingCmd;
    closeCmdPrompt();
    if (!arg || !cmd) return;
    run(cmd + arg);   // cmd 自带尾空格
  }
  async function newProjectFlow() {
    closeProjectMenu();
    const name = (document.getElementById('npw-name').value || '').trim();
    if (!name) { document.getElementById('npw-name').focus(); return; }
    try {
      const r = await api('/api/projects', { method: 'POST', body: JSON.stringify({ name, template: npwSel }) });
      closeNewProjectWizard();
      await refreshProjects();
      await switchProject(r.project.id, r.project.name);
      term.writeln('\x1b[32m✓ 项目已创建：' + r.project.name + '（模板 ' + npwSel + '）\x1b[0m');
    } catch (e) {
      term.writeln('\x1b[31m新建项目失败: ' + e + '\x1b[0m');
    }
  }
  async function spacesCmd(arg) {
    // /spaces：列出全部项目空间（注意 /projects 是图谱的笔记目录统计，两者不同）
    await refreshProjects();
    term.writeln('\x1b[1;36m──── 项目空间 ────\x1b[0m');
    term.writeln(' 🗂️ 个人空间（默认）' + (currentProject ? '' : '  \x1b[32m← 当前\x1b[0m'));
    for (const p of projectList) {
      term.writeln(' ' + templateIcon(p.template) + ' ' + p.name + '  \x1b[90m[' + p.template + ']\x1b[0m' + (currentProject === p.id ? '  \x1b[32m← 当前\x1b[0m' : ''));
    }
    term.writeln('\x1b[90m切换：点顶栏项目名 · 新建：项目菜单「＋ 新建项目」\x1b[0m');
  }
  // /decide <主题> <结论>：未决决策拍板（B3 闭环）——从未决清单移除议题、结论入已决 + MEMORY 决策区
  async function decideCmd(rest) {
    const topic = String(rest[0] || '').trim();
    const conclusion = rest.slice(1).join(' ').trim();
    if (!topic) { term.writeln('\x1b[33m用法: /decide <主题> <结论>（主题从 /view pending 或 notes/决策/未决.md 取）\x1b[0m'); return; }
    if (!conclusion) { term.writeln('\x1b[33m缺结论: /decide <主题> <结论>\x1b[0m'); return; }
    try {
      const r = await api('/api/decide', { method: 'POST', body: JSON.stringify({ topic, conclusion }) });
      term.writeln('\x1b[32m✓ ' + (r.msg || '已拍板') + '\x1b[0m');
      refreshStatus();
    } catch (e) {
      term.writeln('\x1b[31m' + ((e && e.message) || e) + '\x1b[0m');
    }
  }
  // 项目切换器事件绑定（模块级初始化）
  loadProjectState();
  refreshProjects();
  (function bindProjectUI() {
    const chip = document.getElementById('project-chip');
    if (chip) chip.addEventListener('click', (e) => { e.stopPropagation(); toggleProjectMenu(); });
    // 输入框顶部项目入口
    const entry = document.getElementById('ds-project-entry');
    if (entry) entry.addEventListener('click', (e) => { e.stopPropagation(); toggleProjectMenu(undefined, entry); });
    const pmNew = document.getElementById('pm-new');
    if (pmNew) pmNew.addEventListener('click', openNewProjectWizard);
    const npwCancel = document.getElementById('npw-cancel');
    if (npwCancel) npwCancel.addEventListener('click', closeNewProjectWizard);
    const npwCreate = document.getElementById('npw-create');
    if (npwCreate) npwCreate.addEventListener('click', newProjectFlow);
    const npwName = document.getElementById('npw-name');
    if (npwName) npwName.addEventListener('keydown', (e) => { if (e.key === 'Enter') newProjectFlow(); });
    const npwOv = document.getElementById('npw-overlay');
    if (npwOv) npwOv.addEventListener('click', (e) => { if (e.target === npwOv) closeNewProjectWizard(); });
    // 命令参数弹窗绑定
    const ppRun = document.getElementById('pp-run');
    if (ppRun) ppRun.addEventListener('click', runCmdPrompt);
    const ppCancel = document.getElementById('pp-cancel');
    if (ppCancel) ppCancel.addEventListener('click', closeCmdPrompt);
    const ppInput = document.getElementById('pp-input');
    if (ppInput) ppInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') runCmdPrompt(); });
    const ppOv = document.getElementById('pp-overlay');
    if (ppOv) ppOv.addEventListener('click', (e) => { if (e.target === ppOv) closeCmdPrompt(); });
    // 软确认弹窗绑定：确认/取消按钮 + 遮罩点击取消 + Esc 取消（安全默认焦点在取消）
    const cfOkBtn = document.getElementById('cf-ok');
    if (cfOkBtn) cfOkBtn.addEventListener('click', () => { if (cfCb) cfCb(true); });
    const cfCancelBtn = document.getElementById('cf-cancel');
    if (cfCancelBtn) cfCancelBtn.addEventListener('click', () => { if (cfCb) cfCb(false); });
    const cfOv = document.getElementById('cf-overlay');
    if (cfOv) cfOv.addEventListener('click', (e) => { if (e.target === cfOv && cfCb) cfCb(false); });
    document.addEventListener('keydown', (e) => {
      if (cfCb && e.key === 'Escape') { e.preventDefault(); cfCb(false); }
    });
    document.addEventListener('click', (e) => {
      const menu = document.getElementById('project-menu');
      if (menu && !menu.hidden && !menu.contains(e.target)) {
        const isChip = e.target.id === 'project-chip';
        const isEntry = e.target.closest('#ds-project-entry');
        if (!isChip && !isEntry) closeProjectMenu();
      }
    });
    document.addEventListener('keydown', (e) => { if (e.key === 'Escape') closeProjectMenu(); });
  })();
  // 新手引导：首次启动且未配置连接 → 自动打开引导面板（老用户/已配置跳过）
  function maybeRunOnboarding() {
    try {
      if (localStorage.getItem('md-agent-onboarded') === '1') return;
    } catch (e) { return; }
    api('/api/config').then((cfg) => {
      if (cfg && cfg.llm && cfg.llm.endpoint) {
        try { localStorage.setItem('md-agent-onboarded', '1'); } catch (e) {}
        return;
      }
      viewCmd('onboarding').catch(() => {});
    }).catch(() => {});
  }
  setTimeout(maybeRunOnboarding, 800);

  // ---- 状态轮询（输入条状态行 + 欢迎横幅状态 + 顶栏模型芯片 + 侧边栏会话指纹；8s） ----
  function refreshStatus() {
    const px = currentProject ? { 'X-Project': currentProject } : {};
    Promise.all([
      fetch('/api/health', { headers: px }).then((r) => r.json()).catch(() => null),
      fetch('/api/config', { headers: px }).then((r) => r.json()).catch(() => null),
      fetch('/api/kb/pending', { headers: px }).then((r) => r.json()).catch(() => null),
      fetch('/api/tasks', { headers: px }).then((r) => r.json()).catch(() => null),
      fetch('/api/graph/stats', { headers: px }).then((r) => r.json()).catch(() => null),
      fetch('/api/heartbeat', { headers: px }).then((r) => r.json()).catch(() => null),
      fetch('/api/risk', { headers: px }).then((r) => r.json()).catch(() => null),
    ]).then(([h, c, p, t, g, hb, rk]) => {
      const ok = !!(h && h.status === 'ok');
      const model = (c && c.llm && c.llm.model) || '未配置 LLM';
      const endpoint = (c && c.llm && c.llm.endpoint) || '';
      const kb = (c && c.kb_root) || '-';
      applyLlmConfigured(!!endpoint); // 配置页改 endpoint 后 ≤8s 生效（无需重载页面）
      const pend = (p && Array.isArray(p.pending)) ? p.pending.length : '-';
      const todo = (t && t.stats) ? (t.stats.todo || 0) + (t.stats.doing || 0) : '-';
      const gs = (g && g.docs) ? (g.docs || 0) + ' 文档 / ' + (g.links || 0) + ' 链接' : '-';
      const hbTxt = hb ? (hb.enabled ? '心跳开' : '心跳关') : '';
      let auditTxt = '';
      let auditCount = 0; // 侧边栏「审计」徽标（与状态行同源）
      if (hb && hb.audit && (hb.audit.orphans || hb.audit.dangling || hb.audit.duplicates || hb.audit.mentions)) {
        const parts = [];
        if (hb.audit.orphans) { parts.push('孤立 ' + hb.audit.orphans); auditCount += hb.audit.orphans; }
        if (hb.audit.dangling) { parts.push('悬空 ' + hb.audit.dangling); auditCount += hb.audit.dangling; }
        if (hb.audit.duplicates) { parts.push('重复 ' + hb.audit.duplicates); auditCount += hb.audit.duplicates; }
        if (hb.audit.mentions) auditCount += hb.audit.mentions;
        auditTxt = ' ⚠ 审计：' + parts.join(' · ');
      }
      // 风控预警（律师案件：时效/证据缺口/信息缺失；/api/risk 项目感知，30s 心跳兜底）
      let riskTxt = '';
      let riskCount = 0;
      if (rk && (rk.deadlines || rk.evidence_gaps || rk.info_missing)) {
        const rp = [];
        if (rk.deadlines) { rp.push('⏰时效 ' + rk.deadlines); riskCount += rk.deadlines; }
        if (rk.evidence_gaps) { rp.push('📎证据 ' + rk.evidence_gaps); riskCount += rk.evidence_gaps; }
        if (rk.info_missing) { rp.push('📋信息 ' + rk.info_missing); riskCount += rk.info_missing; }
        riskTxt = ' ⚠风控：' + rp.join(' · ');
      }
      updateBadges(typeof pend === 'number' ? pend : 0, auditCount); // 侧边栏徽标（待审红/审计黄）
      updateModelChip(model, endpoint);
      const verEl = document.getElementById('sb-ver-num');
      if (verEl && h && h.version) verEl.textContent = h.version;
      statusLine = truncateW(
        '\x1b[' + (ok ? '32' : '31') + 'm●\x1b[0m ' + (ok ? '服务运行中' : '服务异常') +
        '\x1b[90m · 模型 ' + model + ' · KB ' + kb + ' · 待审 ' + pend +
        ' · 任务 ' + todo + ' · 图谱 ' + gs +
        (hbTxt ? ' · ' + hbTxt : '') +
        (auditTxt ? '\x1b[33m' + auditTxt + '\x1b[0m' : '') +
        (riskTxt ? '\x1b[31m' + riskTxt + '\x1b[0m' : '') + '\x1b[0m',
        trueCols() - 1);
      drawStatusRow();
      // 欢迎横幅状态行（R2：与输入条状态行同源，随轮询更新）
      const bannerStatus = document.querySelector('.welcome .w-status');
      if (bannerStatus) {
        const kbName = kb.split(/[\/\\]/).pop();
        bannerStatus.innerHTML = '版本 <span class="v">v' + ((h && h.version) || '?') + '</span> · 模型 <span class="v">' + model + '</span>' +
          ' · KB <span class="v">' + kbName + '</span> · 待审 <span class="v">' + pend + '</span>' +
          ' · 进行中任务 <span class="v">' + todo + '</span> · 图谱 <span class="v">' + gs + '</span>';
      }
      paintSidebarIfChanged(); // 会话列表变化才重绘（R6）
    }).catch(() => {});
  }
  refreshStatus();
  setInterval(refreshStatus, 8000);

  // ---- 快捷按钮徽标（data-badge="review" 审核按钮：待审红优先，无待审时审计告警黄；与状态行同源，8s 轮询一起更新）----
  function setBadge(btn, n, warn) {
    let b = btn.querySelector('.badge');
    if (!n) { if (b) b.remove(); return; }
    if (!b) { b = document.createElement('span'); b.className = 'badge'; btn.appendChild(b); }
    b.textContent = n > 99 ? '99+' : n;
    b.classList.toggle('warn', !!warn);
  }
  function updateBadges(pend, auditCount) {
    for (const btn of quickBtns) {
      if (btn.dataset.badge === 'review') {
        // 待审红优先；无待审但审计有告警 → 黄徽标
        if (pend > 0) setBadge(btn, pend, false);
        else setBadge(btn, auditCount > 0 ? auditCount : 0, true);
      }
    }
  }

  // 启动时注入 L1（类 CLAUDE.md：规范 / 记忆 / 索引层）
  (async function loadL1() {
    try {
      const res = await fetch('/api/l1?full=1');
      // 后端未启动 / 返回 HTML 错误页 → 友好提示，不抛 JSON 解析异常
      if (!res.ok) throw new Error('HTTP ' + res.status);
      const ct = res.headers.get('content-type') || '';
      if (!ct.includes('application/json')) throw new Error('服务未就绪（未返回 JSON）');
      const b = await res.json();
      if (b.l1 && b.l1.length) {
        L1_TEXT = b.l1.map((f) => '【' + f.name + '】\n' + f.content).join('\n\n');
        // CE 组装器 v1：L1 分区——规范层（稳定前缀）vs 记忆/索引层（易变），供 buildGuidePrefix
        GUIDE_TEXT = b.l1.filter((f) => /^(KB|FRAMEWORK|RULES)\./i.test(f.name)).map((f) => '【' + f.name + '】\n' + f.content).join('\n\n');
        MEMORY_TEXT = b.l1.filter((f) => !/^(KB|FRAMEWORK|RULES|memory_summary)\./i.test(f.name)).map((f) => '【' + f.name + '】\n' + f.content).join('\n\n');
        term.writeln(
          '\x1b[90m(L1 已注入 ' + b.l1.length + ' 个文件: ' + b.l1.map((f) => f.name).join(' ') + ')\x1b[0m'
        );
      } else {
        term.writeln('\x1b[33m警告: L1 层为空，规范/记忆未注入\x1b[0m');
      }
    } catch (e) {
      /* 后端未启动时静默处理，不显示错误 */
    }
    await bannerDone;
    // 恢复未提交草稿（刷新前未发送的输入）
    try {
      const d = localStorage.getItem('md-agent-draft');
      if (d) line = d.slice(0, 200);
    } catch (e) { /* 忽略 */ }
    showPrompt(); // 输入框（上边框/输入行/下边框/状态行）在内容末尾
  })();

  // 软确认弹窗（替代终端 y/n）：写操作人审走页面内 modal（#cf-overlay），不再挂起等按键
  // 可逆操作（新对话/归档/断开 hub）已改免确认直接执行；不可逆/安全敏感操作保留确认
  let cfCb = null;
  function uiConfirm(msg, opts) {
    return new Promise((resolve) => {
      const box = document.getElementById('cf-overlay');
      if (!box) { resolve(true); return; } // 弹窗缺失兜底：直接放行
      cfCb = (ok) => { box.hidden = true; cfCb = null; resolve(ok); };
      document.getElementById('cf-msg').textContent = msg;
      const danger = !!(opts && opts.danger);
      const okBtn = document.getElementById('cf-ok');
      okBtn.textContent = danger ? '确认删除' : '确认';
      okBtn.className = danger ? 'danger' : '';
      document.getElementById('cf-title').textContent = danger ? '危险操作' : '确认操作';
      box.hidden = false;
      setTimeout(() => document.getElementById('cf-cancel').focus(), 50); // 安全默认：焦点在取消
    });
  }
  function confirm(msg) { return uiConfirm(msg); } // 历史调用点语义不变（交互改为弹窗）

  // 输入提交（原生 textarea：字符编辑/IME 组合由浏览器处理，line 经 input 事件同步；这里只接 Enter）
  term.onData((data) => {
    touchActivity(); // A4 收尾归档：任意输入 = 活动（7 天空闲判定）
    if (data === '\r') {
      const cmd = line.trim();
      submitCmd(cmd); // 统一提交入口（提交块 + run + finally showPrompt）
    }
  });

  // 本地日期（YYYY-MM-DD），避免 toISOString 的 UTC 偏移把凌晨算成前一天
  function localToday() {
    const d = new Date();
    return (
      d.getFullYear() + '-' +
      String(d.getMonth() + 1).padStart(2, '0') + '-' +
      String(d.getDate()).padStart(2, '0')
    );
  }

  // ---------- 伪命令行 Markdown 渲染（ANSI 富渲染，零依赖） ----------
  // 原则：结构符号保留并弱化（# | > - 复制保 markdown 结构），样式符号渲染掉（** `）；
  // 表格按 markdown 行显示，不画框、不对齐（避开 CJK 宽度与流式缓冲成本）；frontmatter 变暗。

  const C = {
    reset: '\x1b[0m', bold: '\x1b[1m', dim: '\x1b[2m', italic: '\x1b[3m',
    underline: '\x1b[4m', cyan: '\x1b[36m', blue: '\x1b[34m',
    green: '\x1b[32m', yellow: '\x1b[33m', gray: '\x1b[90m',
  };

  // 行内：代码 > 加粗 > 斜体 > 链接 > 双链（顺序保证嵌套不串色）
  function renderInline(s) {
    return s
      .replace(/`([^`]+)`/g, (_, c) => C.yellow + c + C.reset)
      .replace(/\*\*([^*]+)\*\*/g, (_, c) => C.bold + c + C.reset)
      .replace(/__([^_]+)__/g, (_, c) => C.bold + c + C.reset)
      .replace(/\*([^*]+)\*/g, (_, c) => C.italic + c + C.reset)
      .replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_, t, u) => C.blue + C.underline + t + C.reset + C.dim + ' (' + u + ')' + C.reset)
      .replace(/\[\[([^\]]+)\]\]/g, (_, t) => C.cyan + '[[' + t + ']]' + C.reset);
  }

  function renderLine(line) {
    const t = line.trimStart();
    const h = t.match(/^(#{1,6})\s+(.*)/);
    if (h) return C.bold + C.cyan + '#'.repeat(h[1].length) + ' ' + renderInline(h[2]) + C.reset;
    if (/^(\s*)(-{3,}|\*{3,}|_{3,})\s*$/.test(line)) return C.gray + '─'.repeat(48) + C.reset;
    // 表格行：保留 markdown 管道结构，| 弱化，单元格内容富渲染
    if (t.startsWith('|')) {
      return t.split('|').map((c) => renderInline(c)).join(C.dim + '|' + C.reset);
    }
    const task = t.match(/^[-*]\s+\[([ xX])\]\s+(.*)/);
    if (task) return (task[1] === 'x' || task[1] === 'X' ? C.green + '[x] ' : C.gray + '[ ] ') + renderInline(task[2]) + C.reset;
    const li = t.match(/^([-*+]|\d+\.)\s+(.*)/);
    if (li) return C.cyan + li[1] + C.reset + ' ' + renderInline(li[2]);
    const bq = t.match(/^>\s?(.*)/);
    if (bq) return C.dim + '> ' + renderInline(bq[1]) + C.reset;
    return renderInline(line);
  }

  // 跨行状态机：代码围栏 / frontmatter（仅首行判定，避免正文 --- 分隔线误判）
  function createMdRenderer(opts = {}) {
    const frontmatter = opts.frontmatter || 'dim'; // 'dim' | 'hide'
    let inCode = false;
    let inFm = false;
    let first = true;
    return {
      feed(line) {
        const out = [];
        if (inCode) {
          if (/^```/.test(line)) { inCode = false; out.push(C.gray + line + C.reset); }
          else out.push(line);
          return out;
        }
        if (/^```/.test(line)) { inCode = true; out.push(C.gray + line + C.reset); return out; }
        if (first) {
          first = false;
          if (line.startsWith('---')) {
            inFm = true;
            if (frontmatter !== 'hide') out.push(C.dim + line + C.reset);
            return out;
          }
        }
        if (inFm) {
          if (line === '---') inFm = false;
          if (frontmatter !== 'hide') out.push(C.dim + line + C.reset);
          return out;
        }
        out.push(renderLine(line));
        return out;
      },
      flush() { return []; },
    };
  }

  // 整段渲染（open / /l1 / digest 等非流式场景）
  function renderMdFile(text, opts) {
    const md = createMdRenderer(opts);
    const out = [];
    for (const line of String(text).replace(/\r\n/g, '\n').split('\n')) {
      out.push(...md.feed(line));
    }
    out.push(...md.flush());
    return out.join('\n');
  }

  // 深度思考折叠块（DeepSeek 式）：reasoning_content 折叠展示，点击展开；返回行 HTML
  function renderThinkBlock(text, secs) {
    return '<div class="think-block collapsed">' +
      '<div class="think-head"><span>🧠 深度思考' + (secs ? ' · ' + secs + ' 秒' : '') + '</span><span class="chev">▸</span></div>' +
      '<div class="think-body">' + escHtml(String(text || '')) + '</div>' +
      '</div>';
  }
  // 思考块点击展开（renderThinkBlock 的行元素需绑定；DeepSeek 式折叠交互）
  function wireThink(rowEl) {
    if (!rowEl) return;
    const head = rowEl.querySelector('.think-head');
    const blk = rowEl.querySelector('.think-block');
    if (head && blk) head.addEventListener('click', () => blk.classList.toggle('open'));
  }

  async function api(path, opts) {
    // 项目制：统一附加 X-Project 头（无当前项目 → 后端回退全局「个人空间」根）；
    // 显式传入的 X-Project 优先（跨项目操作：归档/删除/读取其他项目的会话文件）
    // 有 body 但未显式 Content-Type → 默认 JSON（部分调用方未传 headers 曾致 415）
    const o = opts || {};
    const hdrs = Object.assign({}, o.headers || {});
    if (o.body !== undefined && !hdrs['Content-Type']) hdrs['Content-Type'] = 'application/json';
    if (currentProject && !hdrs['X-Project']) hdrs['X-Project'] = currentProject;
    o.headers = hdrs;
    const res = await fetch(path, o);
    const body = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error((body && body.error) || 'HTTP ' + res.status);
    return body;
  }

  async function run(cmd) {
    const [head, ...rest] = cmd.split(/\s+/);
    switch (head) {
      case '/help': help(); break;
      case '/cmd': case '/help-cmd': cmdList(); break;
      case '/search': await search(rest.join(' ')); break;
      case 'open': await openFile(rest[0]); break;
      case '/l1': await l1(); break;
      case '/sync': await sync(); break;
      case '/syncall': await syncAll(); break; // 内部命令：快捷按钮「同步」全量重建
      case '/config': await cfg(); break;
      case '/remember': await remember(rest); break;
      case '/digest': await digest(rest.join(' ')); break;
      case '/clear': if (sessionLog.length && !sessionArchived) archiveSession(); if (currentAbort) { try { currentAbort.abort(); } catch (e) { /* 忽略 */ } currentAbort = null; } for (const k in exes) exes[k].done = true; activeExe = null; history = []; saveHistory(); Core.resetSectionCache(); sessionFile = null; sessionLog = []; sessionArchived = false; sessionTaskId = null; term.writeln('多轮记忆已清空（系统提示词分节缓存已重置）'); break;
      case '/link-all': await linkAll(); break;
      case '/fetch': await fetchCmd(rest); break;
      case '/page': await pageCmd(rest); break;
      case '/task': await taskCmd(rest); break;
      case '/graph': await graph(rest.join(' ')); break;
      case '/orphans': await orphans(); break;
      case '/projects': await projects(); break;
      case '/spaces': await spacesCmd(rest[0]); break;
      case '/newproject': await newProjectFlow(); break;
      case '/tags': await tags(); break;
      case '/rescan': await rescan(); break;
      case '/pending': await pendingList(); break;
      case '/preview': await previewPending(rest[0]); break;
      case '/approve': await pendingAct('approve', rest[0]); break;
      case '/reject': await pendingAct('reject', rest[0]); break;
      case '/view': await viewCmd(rest[0]); break;
      case '/market': await marketCmd(rest); break;
      case '/side': toggleSide(); break;
      case '/consolidate': await consolidateCmd(rest[0]); break;
      case '/skills': await skillsCmd(); break;
      case '/audit': await auditCmd(); break;
      case '/risk': await riskCmd(); break;
      case '/conflicts': await conflicts(); break;
      case '/diff': await diffCmd(rest[0], rest[1]); break;
      case '/link': await linkCmd(rest[0], rest[1]); break;
      case '/suggest': await suggest(rest.join(' ')); break;
      case '/decide': await decideCmd(rest); break;
      case '/sessions': await sessionsCmd(); break;
      case '/resume': await resumeCmd(rest[0]); break;
      case '/dev': await devCmd(rest); break;
      case '/health': await health(); break;
      case '/heartbeat': await heartbeatCmd(rest); break;
      case 'clear': term.clear(); break;
      default:
        if (cmd.startsWith('/')) {
          term.writeln('\x1b[33m未知命令\x1b[0m：' + cmd + '（输入 /help 查看）');
        } else {
          await ask(cmd);
        }
    }
  }

  function help() {
    // 界面优先：非技术用户不记命令——所有常用功能都有界面入口，自然语言即可完成日常操作
    term.writeln('md-agent 使用指引：');
    term.writeln('');
    term.writeln('  \x1b[1m直接输入问题\x1b[0m  提问、让它整理笔记、记住事情——用大白话就行，例如：');
    term.writeln('      「帮我梳理这个案件的证据清单」「把这段资料存进笔记」「记住：客户预算 30 万」');
    term.writeln('');
    term.writeln('  \x1b[1m界面入口（不用记命令）\x1b[0m：');
    term.writeln('     左侧菜单     功能首页 / 知识图谱 / 待审 / 审计 / 自动化 / 市场 / 设置');
    term.writeln('     顶栏项目名   切换项目（每个项目独立空间，互不串用）');
    term.writeln('     会话列表     侧边栏点击即可切回该对话继续聊');
    term.writeln('     功能首页     所有功能的卡片入口（含检索/抓取/任务/配置）');
    term.writeln('     命令速览     Ctrl+K 或左侧「命令速览」');
    term.writeln('');
    term.writeln('  \x1b[90m高级命令（一般用不到）：输入 /cmd 查看完整列表\x1b[0m');
  }
  // 完整命令列表（高级通道）：帮助文案/命令补全已隐藏，这里保留可查
  function cmdList() {
    term.writeln('高级命令列表：');
    term.writeln('  直接输入问题          知识库问答（流式输出 + 多轮记忆 + 自动沉淀）');
    term.writeln('  /search <关键词>       检索双层库（多关键词任一命中，显示所属小节）');
    term.writeln('  open <路径>            查看 KB 内 MD 文件，如 open notes/rag/xxx.md');
    term.writeln('  /sync                  重建 INDEX.md（自动索引 L2）');
    term.writeln('  /digest <主题>         检索并把结果整理成新笔记写入 notes/');
    term.writeln('  /remember [路径] 内容  手动沉淀（默认追加到 MEMORY.md）');
    term.writeln('  /graph <路径>          知识图谱：出链/入链/关联簇');
    term.writeln('  /orphans               孤立文档（无入链也无出链）');
    term.writeln('  /projects              项目维度统计   /tags 标签统计');
    term.writeln('  /rescan                重建知识图谱（SQLite）');
    term.writeln('  /pending               查看待审（LLM 写回/生成笔记先进这里）');
    term.writeln('  /preview <待审路径>     行级预览：确认批准后将写入的内容');
    term.writeln('  /approve <路径|all>    批准待审 → 写入知识库   /reject 丢弃');
    term.writeln('  /view graph|<html>|off  面板渲染层：内置图谱可视化 / 本地 HTML 视图（Esc 关闭）');
    term.writeln('  /side                  速览侧边栏（任务/待审/图谱/审计，Ctrl+K 或快捷按钮同样唤出）');
    term.writeln('  /consolidate [llm]     巩固器：v1 规则（MEMORY 去重/重复标题提示）；llm 参数用 LLM 生成整合版');
    term.writeln('  /skills                列出技能注册表（Agent 技能提案经 /approve 安装）');
    term.writeln('  /sessions               历史会话列表（kb/sessions/ 全量归档，/view sessions 面板）');
    term.writeln('  /resume <id|标题>       恢复历史会话到当前对话（载入最近 4 轮，重置前缀缓存）');
    term.writeln('  /audit                知识库健康审计（盲区/冲突/补链接建议）');
    term.writeln('  /conflicts             冲突检查（重复标题/悬空链接）   /diff <A> <B> 行级对比');
    term.writeln('  /link <源> <目标>      补链接（在源文档追加 [[目标]]，人工确认）');
    term.writeln('  /link-all              一键应用 /audit 的全部补链接建议');
    term.writeln('  /suggest <主题>        LLM 补全缺失主题的新文档（进待审）');
    term.writeln('  /decide <主题> <结论>  未决决策拍板（未决清单 → 已决 + MEMORY 决策区）');
    term.writeln('  /fetch <url> [标题]    抓取网页：阅读视图 / 带标题则沉淀为待审笔记');
    term.writeln('  /page <url> [标题]     动态网页读取（headless Edge/Chrome，等 JS 渲染）');
    term.writeln('  /task                  任务看板（Phase 3-B 引擎）');
    term.writeln('    /task new <目标> [--title <标题>]  新建   start/done/drop <id> 流转');
    term.writeln('    /task note <id> <内容> 追加日志   dep <id> <依赖id...>   rm <id> 删除');
    term.writeln('    /task board           打开 HTML 看板（/view board）');
    term.writeln('  /clear                 清空多轮对话记忆');
    term.writeln('  /config                查看本地配置（掩码）  配置页: /config.html');
    term.writeln('  /heartbeat [on|off|status]  心跳自动同步：检测知识库变化自动重建索引/图谱 + 审计提示');
    term.writeln('  /health                服务健康检查');
    term.writeln('  clear                  清屏');
  }

  // ---------- Agent 问答回路 ----------


  // ---------- 工具注册表 + Agent Loop（Phase 3-C Step 1） ----------
  // 工具全部映射到现有端点；LLM 显式决策（软工具调用：输出一行 JSON），宿主执行回填

  let toolsCache = null;
  async function getTools() {
    if (toolsCache) return toolsCache;
    try { toolsCache = await api('/api/tools'); } catch (e) { toolsCache = []; }
    return toolsCache;
  }

  // 技能注册表（Phase 3-C Step 2：trigger 命中注入）
  let skillsCache = null;
  async function getSkills() {
    if (skillsCache) return skillsCache;
    try { skillsCache = (await api('/api/skills')).skills || []; } catch (e) { skillsCache = []; }
    return skillsCache;
  }

  // CE 第 4 步：记忆摘要（派生产物，如 INDEX.md——自动生成、无人审、可重建；正文以 MEMORY.md 为准）
  let memorySummaryCache = null;
  async function getMemorySummary() {
    if (memorySummaryCache !== null) return memorySummaryCache;
    try {
      const f = await api('/api/file?path=memory_summary.md');
      memorySummaryCache = (f && f.content) ? f.content : '';
    } catch (e) { memorySummaryCache = ''; }
    return memorySummaryCache;
  }

  // 应用市场（阶段 1）：已安装应用列表（manifest 在 kb/apps/<id>/app.json）
  let appsCache = null;
  async function getApps() {
    if (appsCache) return appsCache;
    try { appsCache = (await api('/api/apps')).apps || []; } catch (e) { appsCache = []; }
    return appsCache;
  }
  // 工作台子菜单：已安装应用前 3 个显示在左侧菜单「工作台」下（点击打开应用面板）
  async function renderSidebarApps(force) {
    const box = document.getElementById('sb-apps');
    if (!box) return;
    if (force) appsCache = null;
    let apps = [];
    try { apps = await getApps(); } catch (e) { apps = []; }
    box.innerHTML = '';
    apps.slice(0, 3).forEach((a) => {
      const b = document.createElement('button');
      b.dataset.cmd = '/view ' + a.id;
      b.title = a.name + (a.desc ? '：' + a.desc : '');
      b.innerHTML = '<span class="mi-ico">📦</span><span>' + escHtml(a.name || a.id) + '</span>';
      b.addEventListener('click', () => submitCmd('/view ' + a.id));
      box.appendChild(b);
    });
  }

  // ---------- CE 记账增强（per-source 分桶）：输入按源分桶估算 + 指纹，miss 归因在 /api/context/stats（ZCode inputBaselineBySource 思想）
  const estTokens = (s) => Math.ceil(String(s || '').length / 3);
  const fpOf = (s) => {
    let h = 5381;
    const str = String(s || '');
    for (let i = 0; i < str.length; i++) h = ((h << 5) + h + str.charCodeAt(i)) >>> 0;
    return h.toString(36);
  };

  // 工具名 → 端点调用（args 为 LLM 给的参数对象）
  const TOOL_API = {
    'search': (a) => api('/api/search?q=' + encodeURIComponent(a.q || '') + '&layer=' + encodeURIComponent(a.layer || 'notes') + (a.ctx ? '&ctx=1' : '')),
    'memory_search': (a) => api('/api/search?q=' + encodeURIComponent(a.q || '') + '&layer=all&ctx=1'),
    'read_l1': (a) => api('/api/l1/read?file=' + encodeURIComponent(a.file || '') + '&q=' + encodeURIComponent(a.q || '') + '&max=' + (a.max_chars || 1200)),
    'graph.linked': (a) => api('/api/graph/linked?path=' + encodeURIComponent(a.path || '')),
    'graph.backlinks': (a) => api('/api/graph/backlinks?path=' + encodeURIComponent(a.path || '')),
    'graph.paths': (a) => api('/api/graph/paths?from=' + encodeURIComponent(a.from || '') + '&to=' + encodeURIComponent(a.to || '') + '&max_depth=6'),
    'risk.check': async () => {
      // 风控预警：律师案件时效/证据缺口/信息缺失（纯规则，零 token）
      const r = await api('/api/risk');
      const items = (r.items || []).slice(0, 20);
      if (!items.length) return '无风控预警';
      return items.map((i) => i.label + ' [' + i.path + ']').join('\n');
    },
    'fetch': (a) => api('/api/fetch?url=' + encodeURIComponent(a.url || '')),
    'page': (a) => api('/api/page?url=' + encodeURIComponent(a.url || '')),
    'file': (a) => api('/api/file?path=' + encodeURIComponent(a.path || '')),
    'tasks': () => api('/api/tasks'),
    'pending.list': async () => {
      // 待审提案清单：写操作人审队列（记忆/技能/巩固/笔记）
      const r = await api('/api/kb/pending');
      const items = r.pending || [];
      if (!items.length) return '无待审提案';
      return items.map((i) => (i.kind ? '[' + i.kind + '] ' : '') + i.title + '（' + i.path + '）').join('\n');
    },
    'market.connect': (a) => api('/api/hubs/connect', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ url: a.hub_url || '' }),
    }),
    'market.search': (a) => api('/api/hubs/search', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ q: a.q || '' }),
    }),
    // C0 dev 工具链（自我开发执行层：让 agent 读自己代码）
    'dev.read': (a) => api('/api/dev/read?path=' + encodeURIComponent(a.path || '')),
    'dev.status': () => api('/api/dev/status'),
    'dev.diff': (a) => api('/api/dev/diff' + (a.path ? '?path=' + encodeURIComponent(a.path) : '')),
    // C3 代码提案通道（生成提案进待审 / 应用+构建验证）
    'dev.patch': (a) => api('/api/dev/patch', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ reason: a.reason || '', files: Array.isArray(a.files) ? a.files : [] }),
    }),
    'dev.apply': (a) => api('/api/dev/apply', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: a.path || '' }),
    }),
  };

  // 工具结果格式化（截断防超长；片段标注来源便于 LLM 引用）
  // B1 读时整理旁路（fire-and-forget，不阻塞回答）：runTool 成功后上报读取路径 → 热度记账 + 规则层自动补链
  function touchMemory(query, paths) {
    if (!paths || !paths.length) return;
    fetch('/api/memory/touch', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ query: query || '', paths: paths.slice(0, 5) }),
    })
      .then((r) => r.json())
      .then((j) => {
        if (j && j.applied && j.applied.length) {
          term.writeln('\x1b[90m自组织：自动补链 ' + j.applied.length + ' 条（' + j.applied.join('、') + '）\x1b[0m');
        }
      })
      .catch(() => {});
  }

  // 经验闭环 C1（审视层）：触发信号 → 后端 LLM 审视 → 经验提案进待审（fire-and-forget，零 token 触发）
  // C1 审视层触发：强信号（纠错 / 工具失败）→ 经验提案进待审（fire-and-forget，零 token 规则信号）
  // 节流：同信号同主题 5 分钟内只报一次（工具失败可能高频发生，防提案刷屏）
  const expThrottle = {};
  function touchExperience(signal, context) {
    const key = signal + '|' + String(context).slice(0, 60);
    const now = Date.now();
    if (expThrottle[key] && now - expThrottle[key] < 5 * 60 * 1000) return;
    expThrottle[key] = now;
    fetch('/api/experience/propose', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ signal, context: String(context || '').slice(0, 500) }),
    }).catch(() => {});
  }

  // 工具行 DOM 卡（demo .toolrow：三态 + 点击展开 + 失败自动展开）
  // 活动落盘（demo ops 时间线数据源；fire-and-forget，失败静默）
  function logActivity(kind, text, meta) {
    fetch('/api/activity', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ kind, text, meta: meta || {} }),
    }).catch(() => {});
  }

  function createToolRow(t, name, paramsTxt) {
    t = t || term;
    const t0 = Date.now();
    let timer = null;
    // 工具调用折叠卡（.msg.tool 容器：整宽灰色卡片，三态 + 点击展开 + 失败自动展开）
    t.beginMsg('tool');
    const rowEl = t.appendCard(
      '<div class="toolrow queued"><span class="tr-name"><span class="tr-ico">🔧</span>' + escHtml(name) + '</span>' +
      '<span class="tr-state">排队中</span>' +
      '<div class="tr-body"><div class="kv">参数: ' + escHtml(paramsTxt || '{}') + '</div><div class="res"></div></div>' +
      '</div>'
    );
    t.endMsg();
    if (!rowEl) {
      // 后台缓冲（切走会话的执行）：只记录工具事件，切回时重放终态卡
      const exe = t.exe;
      const ev = { name, paramsTxt, state: 'running', result: null };
      if (exe) exe.tools.push(ev);
      return {
        running() {},
        done(r) { if (exe) { ev.state = 'done'; ev.result = String(r).slice(0, 300); } },
        fail(m) { if (exe) { ev.state = 'fail'; ev.result = m; } },
      };
    }
    const card = rowEl.querySelector('.toolrow');
    const stateEl = card.querySelector('.tr-state');
    const bodyEl = card.querySelector('.tr-body');
    const resEl = card.querySelector('.res');
    let open = false;
    function toggleOpen(force) {
      open = force === undefined ? !open : !!open;
      card.classList.toggle('open', open);
    }
    card.addEventListener('click', () => toggleOpen());
    return {
      running() {
        clearInterval(timer);
        const tick = () => {
          card.classList.add('running');
          stateEl.textContent = '执行中 · ' + ((Date.now() - t0) / 1000).toFixed(1) + 's';
        };
        tick();
        timer = setInterval(tick, 200);
      },
      done(result) {
        clearInterval(timer);
        card.classList.remove('queued', 'running');
        card.classList.add('done');
        stateEl.textContent = '成功 · ' + ((Date.now() - t0) / 1000).toFixed(2) + 's';
        logActivity('tool', '工具 ' + name + ' · 成功', { tool: name, ok: true });
        resEl.textContent = '结果: ' + String(result).slice(0, 2000);
      },
      fail(reason) {
        clearInterval(timer);
        card.classList.remove('queued', 'running');
        card.classList.add('fail');
        stateEl.textContent = '失败 · ' + ((Date.now() - t0) / 1000).toFixed(2) + 's';
        logActivity('tool', '工具 ' + name + ' · 失败', { tool: name, ok: false });
        resEl.textContent = '原因: ' + String(reason || '未知错误');
        toggleOpen(true);
      },
    };
  }

  // ================= 交互卡片（对话流内旁路交互：工具结果富展示 + 操作按钮，不污染 LLM 上下文） =================
  // 机制：渲染器注册表 CARD_RENDERERS（工具名 → 渲染器）；attachCard 在工具行后追加卡片，
  // 后台执行时 beginMsg/appendCard/endMsg 三连自动入缓冲 → 切回重放天然还原卡片 DOM；
  // 按钮事件走 #stream 委托分发（data-act），重放后的卡片无需补绑。
  const CARD_KINDS = {
    deadline: { ico: '⏰', label: '时效', cls: 'c-k-deadline' },
    evidence_gap: { ico: '📋', label: '证据', cls: 'c-k-evidence' },
    info_missing: { ico: '📎', label: '缺失', cls: 'c-k-info' },
  };
  const CARD_PKINDS = { memory: '记忆', skill: '技能', consolidate: '巩固', note: '笔记', code: '代码' };
  function riskDaysBadge(days) {
    if (days === null || days === undefined) return '';
    const cls = days < 0 ? 'c-bad c-bad-red' : (days <= 7 ? 'c-bad c-bad-orange' : (days <= 30 ? 'c-bad c-bad-yellow' : 'c-bad'));
    const txt = days < 0 ? '已过期 ' + (-days) + ' 天' : (days === 0 ? '今天到期' : days + ' 天后');
    return '<span class="' + cls + '">' + txt + '</span>';
  }
  function riskCardHtml(r) {
    const items = (r.items || []).slice(0, 20);
    const rows = items.map((i) => {
      const k = CARD_KINDS[i.kind] || { ico: '⚠️', label: i.kind || '风险', cls: '' };
      return '<div class="card-row">' +
        '<span class="c-k ' + k.cls + '">' + k.ico + ' ' + k.label + '</span>' +
        '<span class="c-txt">' + escHtml(i.label) + '</span>' +
        riskDaysBadge(i.days) +
        '<button class="c-btn" data-act="locate" data-path="' + escHtml(i.path) + '">定位</button>' +
        '</div>';
    }).join('');
    return {
      head: '⚠ 风控预警 · ' + items.length + ' 条',
      rows: rows || '<div class="c-empty">✓ 无风控预警</div>',
    };
  }
  function pendingCardHtml(r) {
    const items = r.pending || [];
    const rows = items.map((i) => {
      const kind = CARD_PKINDS[i.kind] || i.kind || '提案';
      return '<div class="card-row">' +
        '<span class="c-k">' + kind + '</span>' +
        '<span class="c-txt" title="' + escHtml(i.path) + '">' + escHtml(i.title) + '</span>' +
        '<button class="c-btn" data-act="pend-preview" data-path="' + escHtml(i.path) + '">预览</button>' +
        '<button class="c-btn c-btn-ok" data-act="pend-approve" data-path="' + escHtml(i.path) + '">批准</button>' +
        '<button class="c-btn c-btn-danger" data-act="pend-reject" data-path="' + escHtml(i.path) + '">拒绝</button>' +
        '</div>';
    }).join('');
    return {
      head: '📝 待审提案 · ' + items.length + ' 项',
      rows: rows || '<div class="c-empty">✓ 无待审提案</div>',
    };
  }
  function cardShell(cls, head, rows) {
    return '<div class="card ' + cls + '"><div class="c-head">' + head + '</div>' +
      '<div class="c-rows">' + rows + '</div><div class="c-prev"></div></div>';
  }
  async function renderRiskCard(t) {
    let r;
    try { r = await api('/api/risk'); } catch (e) { return; }
    const h = riskCardHtml(r);
    t.beginMsg('tool');
    t.appendCard(cardShell('c-risk', h.head, h.rows), 'toolcard');
    t.endMsg();
  }
  async function renderPendingCard(t) {
    let r;
    try { r = await api('/api/kb/pending'); } catch (e) { return; }
    const h = pendingCardHtml(r);
    t.beginMsg('tool');
    t.appendCard(cardShell('c-pending', h.head, h.rows), 'toolcard');
    t.endMsg();
  }
  // 图谱链接卡（第二批）：出链/入链清单，带类型色点 + 定位按钮（复用 data-act=locate 委托）
  function linkCardHtml(r, head) {
    const rows = (r.linked || []).map((l) => {
      if (!l.resolved || !l.dst_path) {
        return '<div class="card-row"><span class="c-k">🔗</span>' +
          '<span class="c-txt" style="color:var(--muted)">悬空: [[未知]]</span></div>';
      }
      return '<div class="card-row"><span class="c-k">🔗</span>' +
        '<span class="c-txt" title="' + escHtml(l.dst_path) + '">' + escHtml(l.dst || l.dst_path) + '</span>' +
        '<button class="c-btn" data-act="locate" data-path="' + escHtml(l.dst_path) + '">定位</button>' +
        '</div>';
    }).join('');
    return { head, rows: rows || '<div class="c-empty">✓ 无</div>' };
  }
  async function renderLinkCard(t, args, kind) {
    const path = (args && args.path) || '';
    if (!path) return;
    let r;
    try {
      r = await api('/api/graph/' + kind + '?path=' + encodeURIComponent(path));
    } catch (e) { return; }
    const label = kind === 'linked' ? '出链' : '入链';
    const h = linkCardHtml(r, '🔗 ' + label + ' · ' + (r.linked || []).length + '（' + path + '）');
    t.beginMsg('tool');
    t.appendCard(cardShell('c-link', h.head, h.rows), 'toolcard');
    t.endMsg();
  }
  // 任务卡（第三批）：任务清单（状态徽标着色，纯展示；操作按钮后置）
  const TASK_STATES = {
    todo: { label: '待办', cls: '' },
    doing: { label: '进行中', cls: 'c-k-evidence' },
    done: { label: '完成', cls: 'c-k-ok' },
    abandoned: { label: '放弃', cls: '' },
  };
  async function renderTaskCard(t) {
    let r;
    try { r = await api('/api/tasks'); } catch (e) { return; }
    const items = r.tasks || [];
    const rows = items.map((x) => {
      const st = TASK_STATES[x.status] || { label: x.status || '待办', cls: '' };
      const title = x.title || x.goal || ('任务 #' + x.id);
      return '<div class="card-row"><span class="c-k ' + st.cls + '">' + st.label + '</span>' +
        '<span class="c-txt" title="任务 #' + escHtml(String(x.id)) + '">' + escHtml(title) + '</span></div>';
    }).join('');
    const h = { head: '📋 任务 · ' + items.length + ' 项', rows: rows || '<div class="c-empty">✓ 无任务</div>' };
    t.beginMsg('tool');
    t.appendCard(cardShell('c-tasks', h.head, h.rows), 'toolcard');
    t.endMsg();
  }
  // 路径图谱卡（第三批）：A→B 最短关联路径链（类型色 chips + 箭头 + 定位），纯 HTML 重放安全
  const PATH_TYPES = {
    case: '#f38ba8', party: '#89b4fa', evidence: '#fab387', timeline: '#94e2d5', law: '#f9e2af',
    candidate: '#89b4fa', position: '#cba6f7', company: '#a6e3a1', comm: '#eba0ac', skill: '#cba6f7',
    memory: '#94e2d5', rule: '#f9e2af', index: '#7f849c', note: '#a6adc8', doc: '#7f849c',
    dir: '#585b70', tag: '#b4befe',
  };
  async function renderPathCard(t, args) {
    const from = (args && args.from) || '';
    const to = (args && args.to) || '';
    if (!from || !to) return;
    let r;
    try {
      r = await api('/api/graph/paths?from=' + encodeURIComponent(from) + '&to=' + encodeURIComponent(to) + '&max_depth=6');
    } catch (e) { return; }
    const chain = r.path || [];
    const chips = chain.map((n, i) => {
      const color = PATH_TYPES[n.type] || '#7f849c';
      const chip = '<span class="card-row" style="gap:4px">' +
        '<button class="c-btn" data-act="locate" data-path="' + escHtml(n.path) + '" title="' + escHtml(n.path) + '">' +
        '<span class="lg-dot" style="background:' + color + '"></span> ' + escHtml(n.title || n.path.split('/').pop()) + '</button></span>';
      return (i > 0 ? '<span class="path-arrow">→</span>' : '') + chip;
    }).join(' ');
    const head = chain.length
      ? '🔗 关联路径 · ' + (chain.length - 1) + ' 跳'
      : '🔗 关联路径 · 未找到（6 跳内不连通）';
    const rows = chain.length
      ? '<div class="c-rows" style="flex-direction:row;flex-wrap:wrap;align-items:center">' + chips + '</div>'
      : '<div class="c-empty">⚠ 6 跳内未找到关联路径（两文档不连通）</div>';
    t.beginMsg('tool');
    t.appendCard(cardShell('c-path', head, rows), 'toolcard');
    t.endMsg();
  }
  const CARD_RENDERERS = {
    'risk.check': renderRiskCard,
    'pending.list': renderPendingCard,
    'graph.linked': (t, a) => renderLinkCard(t, a, 'linked'),
    'graph.backlinks': (t, a) => renderLinkCard(t, a, 'backlinks'),
    'graph.paths': renderPathCard,
    'tasks': renderTaskCard,
  };
  // 在工具行之后追加交互卡片（无渲染器/取数失败静默跳过，不打断回答）
  async function attachCard(t, toolName, args) {
    const renderer = CARD_RENDERERS[toolName];
    if (!renderer) return;
    try { await renderer(t, args); } catch (e) { /* 卡片是旁路展示，失败不打扰回答流 */ }
  }
  // 待审卡就地刷新（approve/reject 后重取列表，更新 head/rows，不重建容器）
  async function refreshPendingCard(card) {
    let r;
    try { r = await api('/api/kb/pending'); } catch (e) { return; }
    const h = pendingCardHtml(r);
    const head = card.querySelector('.c-head');
    const rows = card.querySelector('.c-rows');
    if (head) head.innerHTML = h.head;
    if (rows) rows.innerHTML = h.rows;
  }
  // #stream 事件委托：卡片按钮统一分发（重放后的卡片无需补绑）
  function initCardDelegation() {
    const stream = document.getElementById('stream');
    if (!stream) return;
    stream.addEventListener('click', async (ev) => {
      const btn = ev.target.closest('[data-act]');
      if (!btn || !btn.dataset.act) return;
      const card = btn.closest('.card');
      const act = btn.dataset.act;
      const path = btn.dataset.path || '';
      if (act === 'locate') { openViewForPath(path); return; }
      if (act === 'pend-preview') {
        let p;
        try { p = await api('/api/kb/pending/preview?path=' + encodeURIComponent(path)); } catch (e) { return; }
        const box = card ? card.querySelector('.c-prev') : null;
        if (!box) return;
        box.innerHTML = '<div class="c-prev-in"><div class="c-prev-t">目标: ' + escHtml(p.target || '') + '（' + (p.kind || '') + '）</div>' +
          '<pre>' + escHtml(String(p.added || '').slice(0, 1000)) + '</pre></div>';
        return;
      }
      if (act === 'pend-approve' || act === 'pend-reject') {
        const label = act === 'pend-approve' ? '批准' : '拒绝';
        btn.disabled = true;
        btn.textContent = '…';
        try {
          await api('/api/kb/pending/' + act.slice('pend-'.length), {
            method: 'POST', headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ path }),
          });
          const fb = card ? card.querySelector('.c-prev') : null;
          if (fb) fb.innerHTML = '<div class="c-fb-ok">✓ 已' + label + '：' + escHtml(path) + '</div>';
          if (card) refreshPendingCard(card);
        } catch (e) {
          btn.disabled = false;
          btn.textContent = label;
        }
        return;
      }
    });
  }

  async function runTool(name, args) {
    const fn = TOOL_API[name];
    if (!fn) throw new Error('未知工具: ' + name);
    let r;
    try {
      r = await fn(args || {});
    } catch (e) {
      // C1 触发层：工具失败 = 真实摩擦强信号（零 token）→ 经验提案进待审（后端审视分级决定是否沉淀）
      touchExperience('tool_failed', '工具 ' + name + ' 失败: ' + ((e && e.message) || e) + (args && args.q ? '（查询: ' + String(args.q).slice(0, 80) + '）' : ''));
      throw e;
    }
    // B1 读时整理：收集本次读取的 KB 路径（read_l1/file/graph/search 类）
    const readPaths = [];
    if (name === 'read_l1') { if (args.file) readPaths.push(args.file); }
    else if (name === 'file') { if (args.path) readPaths.push(args.path); }
    else if (name === 'graph.linked' || name === 'graph.backlinks') { if (args.path) readPaths.push(args.path); }
    else if (name === 'search' || name === 'memory_search') { (r.hits || []).forEach((h) => { if (h.file) readPaths.push(h.file); }); }
    if (readPaths.length) touchMemory(args.q || '', readPaths);
    if (name === 'search' || name === 'memory_search') {
      // 方向 4：命中 10→8、单条上下文截断 200——摘要注入，模型可重查（frc 指令节背书）
      const hits = r.hits || [];
      return hits.slice(0, 8).map((h) =>
        '[' + h.file + ':' + h.line + (h.section ? ' 小节:' + h.section : '') + '] ' + String(h.context || h.text || '').slice(0, 200)
      ).join('\n') || '(无命中)';
    }
    if (name === 'read_l1') {
      // L1 原文取用：section=小节原文；section_list=未定位到，给可用小节清单引导下一次调用；head=文件头
      if (r.mode === 'section') return r.content || '(未命中)';
      if (r.mode === 'section_list') return '未定位到含该定位词的 ## 小节。可用小节：\n' + (r.sections || []).map((s) => '- ' + s).join('\n');
      return (r.content || '') + ((r.sections || []).length ? '\n\n小节：' + (r.sections || []).map((s) => '- ' + s).join('\n') : '');
    }
    if (name === 'graph.linked') {
      return (r.linked || []).map((l) => '[[目标]] ' + (l.dst || '') + (l.resolved ? ' → ' + l.dst_path : ' (悬空)')).join('\n') || '(无出链)';
    }
    if (name === 'graph.backlinks') return (r.backlinks || []).join('\n') || '(无入链)';
    if (name === 'graph.paths') {
      // 路径链文本：A → B → C（含类型与标题）
      const chain = r.path || [];
      if (!chain.length) return '6 跳内未找到关联路径（两文档不连通）';
      return chain.map((n) => '[' + (n.type || 'doc') + '] ' + (n.title || n.path)).join(' → ');
    }
    if (name === 'risk.check') return r; // TOOL_API 已格式化为文本清单（label + [path] 每行一条）
    if (name === 'fetch' || name === 'page') return '标题: ' + (r.title || '') + '\n' + String(r.text || '').slice(0, 2000); // 方向 4：3000→2000 摘要注入
    if (name === 'file') return String(r.content || '').slice(0, 2000); // 方向 4：3000→2000 摘要注入
    if (name === 'tasks') {
      const t = r.tasks || [];
      return t.length ? t.map((x) => '#' + x.id + ' [' + x.status + '] ' + (x.title || x.goal)).join('\n') : '(无任务)';
    }
    if (name === 'market.connect') {
      const h = r.hub;
      if (!h) return JSON.stringify(r).slice(0, 3000);
      const nApp = (h.apps || []).filter((a) => a.kind === 'app').length;
      const nSkill = (h.apps || []).length - nApp;
      const apps = (h.apps || []).map((a) => a.id + ' v' + a.version + (a.kind === 'skill' ? ' [技能]' : '') + ' ' + a.name + ' — ' + a.description).join('\n');
      return '已连接 SkillHub「' + h.name + '」（分析出 ' + nApp + ' 个应用 + ' + nSkill + ' 个技能）：\n' + apps + '\n安装：/market install <id>（人审确认）';
    }
    if (name === 'market.search') {
      const apps = (r.apps || []);
      if (!apps.length) return '无匹配结果';
      return apps.map((a) => a.id + ' v' + a.version + ' ' + a.name + ' — ' + a.description).join('\n') + '\n安装：/market install <id>（人审确认）';
    }
    if (name === 'dev.read') return String(r.content || '(空文件)').slice(0, 3000) + (r.path ? '\n[来源 ' + r.path + ']' : '');
    if (name === 'dev.status' || name === 'dev.diff') return String(r.output || '(无改动/无输出)');
    if (name === 'dev.patch') return '代码提案已生成: ' + (r.path || '') + '（' + (r.files || 0) + ' 个文件）——待审人审后 /dev apply 应用';
    if (name === 'dev.apply') {
      if (r.ok) return '已应用 ' + ((r.applied || []).join(', ')) + '，构建验证通过';
      return '应用失败已回滚: ' + String(r.error || r.build || '未知');
    }
    return JSON.stringify(r).slice(0, 3000);
  }

  // 单次 LLM 流式调用（Agent Loop 一轮）：正常回答流式渲染；首个 content 以 { 开头 → 工具模式只收集不渲染
  // 返回 { full, reasoning, reasoningStartAt, firstContentAt, lastUsage, toolJson }；中断/失败返回 null
  // web=true → 联网通道（Responses API + 服务端 web_search，非流式；返回已归一化为 chat 结构）
  // ================= 会话后台执行（第二步）：AI 回答按会话缓冲，切走不打断 =================
  // exes: sessionKey → exe（sessionKey = sessionFile，首问未落盘 = null）；activeExe = 当前渲染目标。
  // 活跃会话的执行写 term DOM；后台会话的执行把行操作入缓冲（LLM 流继续跑），切回时重放缓冲 + 工具卡终态。
  const exes = {};
  let activeExe = null;
  const exeKey = () => sessionFile || null;
  function makeExeTerm(exe) {
    const rec = (op, args) => exe.lines.push({ op, args });
    return {
      exe,
      writeln: (x) => { if (exe.active) term.writeln(x); else rec('writeln', [x]); },
      write: (x) => { if (exe.active) term.write(x); else rec('write', [x]); },
      beginMsg: (k) => { if (exe.active) return term.beginMsg(k); rec('beginMsg', [k]); return null; },
      endMsg: (f) => { if (exe.active) return term.endMsg(f); rec('endMsg', [f]); },
      currentMsg: () => (exe.active ? term.currentMsg() : null),
      appendRow: (txt, cls) => { if (exe.active) return term.appendRow(txt, cls); rec('appendRow', [txt, cls]); return null; },
      appendCard: (html, cls) => { if (exe.active) return term.appendCard(html, cls); rec('appendCard', [html, cls]); return null; },
      removeRow: (el) => { if (exe.active) term.removeRow(el); else rec('removeRow', [el]); },
      replaceRow: (el, html, cls) => { if (exe.active) return term.replaceRow(el, html, cls); rec('replaceRow', [el, html, cls]); return null; },
      clear: () => { if (exe.active) term.clear(); else rec('clear', []); },
    };
  }
  // 切回会话：重放缓冲（含工具卡终态）+ 引用增强，恢复流式渲染目标。
  // done 的执行也重放（后台完成的回答需在切回时完整显示）；lines 重放后清空保证不重复
  function activateExe(exe) {
    if (!exe || exe.active) return;
    exe.active = true;
    for (const l of exe.lines) {
      if (l.op === 'writeln') term.writeln(l.args[0]);
      else if (l.op === 'write') term.write(l.args[0]);
      else if (l.op === 'beginMsg') term.beginMsg(l.args[0]);
      else if (l.op === 'endMsg') term.endMsg(l.args[0]);
      else if (l.op === 'appendRow') term.appendRow(l.args[0], l.args[1]);
      else if (l.op === 'appendCard') term.appendCard(l.args[0], l.args[1]);
      else if (l.op === 'clear') term.clear();
    }
    exe.lines = [];
    for (const ev of exe.tools) {
      const row = createToolRow(exe.t, ev.name, ev.paramsTxt);
      if (ev.state === 'fail') row.fail(ev.result || '失败');
      else if (ev.state === 'done') row.done(ev.result);
      else row.running();
    }
    exe.tools = [];
    enhanceRefs();
    activeExe = exe;
    refreshRunningUI();
  }
  // 挂起当前活跃执行（切走会话 → 转后台继续跑）
  function suspendExe() {
    if (activeExe && !activeExe.done) activeExe.active = false;
    activeExe = null;
  }
  // 开始一次 AI 执行：挂起旧执行（转后台），创建/复用当前会话的执行，返回渲染 term
  function beginExe() {
    suspendExe();
    const key = exeKey();
    let exe = exes[key];
    if (!exe) {
      exe = { key, active: true, done: false, lines: [], tools: [], t: null };
      exe.t = makeExeTerm(exe);
      exes[key] = exe;
    }
    exe.active = true;
    exe.done = false;
    activeExe = exe;
    refreshRunningUI(); // 开始执行 → 立即重绘侧边栏（运行图标出现）
    return exe.t;
  }
  // 结束执行：标记完成；首问落盘后迁移 key 归属（仅活跃完成时——后台首问无归属文件，保持 null）
  function finishExe(t) {
    const exe = t && t.exe;
    if (!exe) return;
    exe.done = true;
    if (activeExe === exe) activeExe = null;
    if (exe.active && exe.key === null && sessionFile && exes[null] === exe) {
      delete exes[null];
      exe.key = sessionFile;
      exes[sessionFile] = exe;
    }
    refreshRunningUI();
  }
  // 运行图标：重绘侧边栏（有未完成执行的会话行标运行点）
  function refreshRunningUI() {
    sbFingerprint = '';
    paintSidebar();
  }
  // 当前会话是否有未完成执行（busy 按会话判定）
  function currentExeBusy() {
    const e = exes[exeKey()];
    return !!(e && !e.done);
  }

  // llmStreamOnce(messages, web, onChunk?, t?)：t = 渲染目标（默认 term；会话后台执行传 exeTerm）
  async function llmStreamOnce(messages, web, onChunk, t) {
    const reasoningStartAt = Date.now();
    let full = '';
    let saveSeen = false;
    let reasoning = '';
    let firstContentAt = null;
    let lastUsage = null;
    let toolMode = false;
    let thoughtShown = false;
    let thoughtEl = null;   // 「思考中…」行元素（首个 content 到达时替换为深度思考折叠块）
    let toolJson = null;
    currentAbort = new AbortController();
    setStopBtn(true);
    try {
      const res = await fetch('/api/llm', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(web
          ? { messages, web: true, stream: false }
          : { messages, stream: true, stream_options: { include_usage: true } }),
        signal: currentAbort.signal,
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({}));
        throw new Error((err && err.error) || 'HTTP ' + res.status);
      }
      const ct = res.headers.get('content-type') || '';
      if (!ct.includes('text/event-stream')) {
        // 非流式兜底
        const body = await res.json();
        const msg = body.choices && body.choices[0] && body.choices[0].message;
        full = (msg && msg.content) || '';
        reasoning = (msg && msg.reasoning_content) || '';
        lastUsage = body.usage || null;
        toolJson = Core.tryParseTool(full, TOOL_API);
        // web 模式（非流式）：模型可能只产出 web_search_call 事件而无 message 文本（content 空）→ retry 标记，上层重试一轮
        if (!toolJson && !String(full).trim() && web) {
          return { full, reasoning, reasoningStartAt, firstContentAt, lastUsage, toolJson, retry: true };
        }
        if (!toolJson) {
          // 非流式回答：气泡 + 深度思考块 + 正文
          t.beginMsg('assistant');
          if (reasoning) wireThink(t.appendCard(renderThinkBlock(reasoning, null), 'think-card'));
          t.writeln(renderMdFile(full));
          if (onChunk && full) onChunk(full);
        }
        return { full, reasoning, reasoningStartAt, firstContentAt, lastUsage, toolJson };
      }
      const reader = res.body.getReader();
      const dec = new TextDecoder();
      let buf = '';
      // 流式 markdown：按完整行渲染（保留打字机效果，且行内样式正确）
      const md = createMdRenderer();
      let lineBuf = '';
      let held = []; // 含 <!-- 的行可能是写回块前奏，暂缓显示（避免露出一小段标记）
      // 气泡容器：本轮回答的容器（reasoning 或首个 content 时创建），后续写入自动进入
      const ensureBubble = () => { if (!t.currentMsg()) t.beginMsg('assistant'); };
      const feedDelta = (d) => {
        lineBuf += d;
        let nl;
        while ((nl = lineBuf.indexOf('\n')) !== -1) {
          const line = lineBuf.slice(0, nl);
          lineBuf = lineBuf.slice(nl + 1);
          if (line.includes('<!--')) held.push(line);
          else {
            if (onChunk) onChunk(line); // App 通道：推原始行文本
            for (const l of md.feed(line)) t.writeln(l);
          }
        }
      };
      // 解析 SSE：按 \n\n 切块，取 data: 行
      const consume = (s) => {
        let idx;
        while ((idx = s.indexOf('\n\n')) !== -1) {
          const block = s.slice(0, idx);
          s = s.slice(idx + 2);
          const line = block.split('\n').find((l) => l.startsWith('data:'));
          if (!line) continue;
          const data = line.slice(5).trim();
          if (data === '[DONE]') continue;
          try {
            const j = JSON.parse(data);
            if (j.usage) lastUsage = j.usage; // include_usage 结束块（该块 choices 为空）
            const d = j.choices && j.choices[0] && j.choices[0].delta;
            const rc = d && d.reasoning_content;
            if (rc) {
              reasoning += rc;
              if (!thoughtShown) {
                thoughtShown = true;
                ensureBubble();
                thoughtEl = t.appendRow('🧠 思考中…', 'think');
              }
            }
            const delta = d && d.content;
            if (!delta) continue;
            if (firstContentAt === null) {
              firstContentAt = Date.now(); // reasoning 结束 = 首个 content 到达
              // 工具调用识别：首个 content 以 { 开头 → 整轮工具模式（不渲染）
              toolMode = delta.trimStart().startsWith('{');
              if (toolMode) {
                // 工具轮：移除思考指示与空气泡（E4：不再用 \x1b[1A\x1b[2K 清除，DOM 行直接删）
                if (thoughtEl) { t.removeRow(thoughtEl); thoughtEl = null; }
                t.endMsg(true);
              } else {
                ensureBubble();
                if (thoughtEl) {
                  // 「思考中…」→ 深度思考折叠块（含完整 reasoning，默认折叠）
                  wireThink(t.replaceRow(thoughtEl, renderThinkBlock(reasoning, Math.max(1, Math.round((Date.now() - reasoningStartAt) / 1000))), 'think-card'));
                  thoughtEl = null;
                } else if (reasoning) {
                  wireThink(t.appendCard(renderThinkBlock(reasoning, Math.max(1, Math.round((Date.now() - reasoningStartAt) / 1000))), 'think-card'));
                }
              }
            }
            full += delta;
            if (toolMode) continue; // 工具轮只收集（不渲染）
            // 写回块起就不再展示（继续收集用于落盘）
            if (!saveSeen) {
              if (full.includes('<!-- md-agent-save -->')) {
                saveSeen = true;
                continue; // E5：DOM 无光标概念，不再补 \r\n
              }
              feedDelta(delta);
            }
          } catch (e) { /* 忽略非 JSON 行 */ }
        }
        return s;
      };
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buf = consume(buf + dec.decode(value, { stream: true }));
      }
      consume(buf + dec.decode());
      // 冲刷末尾未换行的行（写回块已触发时丢弃残留的标记前缀）
      if (!toolMode && lineBuf.length && !saveSeen) {
        for (const l of md.feed(lineBuf)) t.writeln(l);
      }
      if (!toolMode) {
        for (const l of md.flush()) t.writeln(l);
        // 写回块未触发（如正文含普通 HTML 注释）→ 补显暂缓行
        if (!saveSeen && held.length) {
          for (const line of held) {
            for (const l of md.feed(line)) t.writeln(l);
          }
          for (const l of md.flush()) t.writeln(l);
        }
      }
    } catch (e) {
      if (e && e.name === 'AbortError') {
        t.writeln('\x1b[33m(回答已中断)\x1b[0m');
      } else {
        const msg = (e && e.message) || '';
        // CE 双模式：上下文超限检测（不打印失败，返回标记由上层 llmOnceWithFresh 降级重试）
        if (Core.isOverflowError(msg)) {
          return { overflow: true };
        }
        term.writeln('\x1b[31mLLM 调用失败: ' + msg + '\x1b[0m');
        term.writeln('\x1b[33m提示: 配置页 http://127.0.0.1:8756/config.html（endpoint/model/api_key）\x1b[0m');
      }
      return null;
    } finally {
      currentAbort = null;
      setStopBtn(false);
    }
    // 工具识别：纯 JSON 开头（toolMode）→ 解析；否则全文检测（DeepSeek 可能先简短说明再调工具）
    if (toolMode) {
      toolJson = Core.tryParseTool(full, TOOL_API);
      if (!toolJson) {
        term.beginMsg('assistant');
        term.writeln(renderMdFile(full));
      }
    } else {
      toolJson = Core.detectToolInFull(full, TOOL_API);
    }
    return { full, reasoning, reasoningStartAt, firstContentAt, lastUsage, toolJson };
  }

  async function ask(question) {
    const t = beginExe(); // 会话后台执行：本回答绑定当前会话（切走转后台，切回重放）
    // 上下文组装 v2：有 LLM 配置 → 去掉启发式预检索，知识/记忆取用走 LLM 显式调工具（read_l1/search/memory_search）；
    // 无 LLM 配置 → 降级保留「启发式关键词提取 + 预检索注入」路径（Ollama 本地等场景兜底）
    t.writeln(llmConfigured
      ? '\x1b[90m(Agent: 知识取用工具化——LLM 显式调 read_l1/search 取规范/记忆/知识)\x1b[0m'
      : '\x1b[90m(Agent: 提取关键词 → 检索 L2 → 调用 LLM)\x1b[0m');
    const kws = Core.extractKeywords(question);

    // 1a. @ 文件提及：@路径 直接注入检索目标（用户显式指定，两分支均保留；全文进上下文；失败仅提示不阻塞）
    const atRefs = [...question.matchAll(/(?:^|\s)@([^\s]+)/g)]
      .map((m) => m[1])
      .filter((p) => p && !/^(\/|https?:)/.test(p));
    const atFrag = [];
    for (const p of atRefs) {
      try {
        const f = await api('/api/file?path=' + encodeURIComponent(p));
        if (f && f.content) {
          atFrag.push('[来源 @' + p + '（指定文档）]\n' + f.content.slice(0, 2000));
          // 路径关键词并入检索 query（文件名去掉扩展名/目录；仅降级分支使用）
          const name = p.split('/').pop().replace(/\.md$/i, '').replace(/[-_]/g, ' ');
          for (const w of Core.extractKeywords(name)) kws.push(w);
        }
      } catch (e) {
        t.writeln('\x1b[33m@ 文件未找到: ' + p + '\x1b[0m');
      }
    }

    // 1. 预检索：仅无 LLM 配置时保留（降级）；有 LLM 配置时去掉（检索片段不再注入，由 Agent Loop 里 LLM 显式调工具取用）
    let top = [];
    if (!llmConfigured) {
      const query = kws.length ? [...new Set(kws)].join(' ') : question;
      t.writeln('\x1b[90m关键词: ' + (kws.length ? query : '(无，用原文)') + '\x1b[0m');
      let sr;
      try {
        sr = await api('/api/search?q=' + encodeURIComponent(query) + '&layer=notes&ctx=1');
      } catch (e) {
        t.writeln('\x1b[31m检索失败: ' + e.message + '\x1b[0m');
        finishExe(t);
        return;
      }
      top = sr.hits.slice(0, 8);
      if (atFrag.length) {
        t.writeln('\x1b[90m@ 指定文档 ' + atFrag.length + ' 篇已注入（' + atRefs.join(' ') + '）\x1b[0m');
      }
      if (!top.length && !atFrag.length) {
        t.writeln('\x1b[33m知识库无相关片段（仅靠 L1 规范与模型自身知识回答）\x1b[0m');
      } else if (top.length) {
        t.writeln('\x1b[90m命中 ' + sr.file_count + ' 文件 / ' + sr.hit_count + ' 处，注入前 ' + top.length + ' 条（按相关度排序）\x1b[0m');
      }
    }

    // 2. 组装 Prompt（system + 多轮历史 + 当前问题）
    const tools = await getTools();
    const toolsTxt = tools.map((t) =>
      '  - ' + t.name + '(' + t.params.map((p) => p.name + (p.required ? '' : '?')).join(', ') + '): ' + t.desc +
      (t.example ? ' | 例: ' + t.example : '')
    ).join('\n');
    // 技能触发注入：输入命中技能 trigger → 注入技能正文（按需引导记忆）
    const hitSkills = (await getSkills()).filter((sk) => sk.trigger && question.includes(sk.trigger));
    let skillTxt = '';
    for (const sk of hitSkills) {
      try {
        const f = await api('/api/file?path=skills/' + encodeURIComponent(sk.name));
        if (f && f.content) skillTxt += '\n【技能:' + sk.title + '（trigger:' + sk.trigger + '）】\n' + f.content.slice(0, 1500) + '\n';
      } catch (e) { /* 忽略 */ }
    }
    const frag = atFrag
      .concat(
        top.map(
          (h) =>
            '[来源 ' + h.file + ':' + h.line + ' 小节:' + (h.section || '(frontmatter)') + ']\n' +
            (h.context || h.text)
        )
      )
      .join('\n\n');
    // CE 组装器（稳定前缀在前）：规范层 + 记忆层 + 工具清单 + 回答规则 = 稳定前缀（会话内字节级稳定）；
    // 技能等易变内容不进 system（命中即炸前缀缓存）→ 放 userMsg 尾部（tail attachment 语义，cc-haha 背书）
    // C 半步：llmConfigured（工具化取用）时前缀瘦身——L1 全文移出前缀，规范/记忆由 LLM 显式调 read_l1 按需取；
    // 无 LLM 配置降级路径保留注入（启发式预检索）
    const system = Core.buildGuidePrefix({
      guideText: llmConfigured ? '' : (GUIDE_TEXT || L1_TEXT), // 规范层优先，L1 全量兜底
      memoryText: llmConfigured ? '' : ((await getMemorySummary()) || MEMORY_TEXT), // CE 第 4 步：注入摘要（派生产物），全文兜底
      toolsTxt,
      today: localToday(),
    });
    const skillTail = Core.buildSkillTail(skillTxt);
    // 上下文组装 v2：有 LLM 配置 → userMsg 只含「问题」+ 技能 + @ 指定文档（检索片段不再预注入，由 LLM 显式调工具取用）；
    // 无 LLM 配置 → 降级注入启发式预检索片段
    const userMsg = llmConfigured
      ? ['问题：' + question]
          .concat(skillTail ? ['', skillTail] : [])
          .concat(atFrag.length ? ['', '指定文档（用户 @ 提及，据此回答）：'].concat(atFrag) : [])
          .join('\n')
      : ['问题：' + question, '', '知识库检索片段（L2）：', frag || '(无片段)']
          .concat(skillTail ? ['', skillTail] : [])
          .join('\n');
    const messages = [
      { role: 'system', content: system },
      ...history,
      { role: 'user', content: userMsg },
    ];

    // 3. Agent Loop：LLM 显式调工具 → 宿主执行回填 → 循环（≤MAX_TOOL 次）；无工具 → 最终回答
    let full = '';
    let reasoning = '';
    let reasoningStartAt = null;
    let firstContentAt = null;
    let lastUsage = null;
    const MAX_TOOL = 8;
    let toolCount = 0;
    // 联网通道开关：输入条开关（webToggle）或触发词首轮开启；知识检索 0 命中时自动开启
    let web = webToggle || Core.webTrigger(question);
    // CE 双模式（fresh-window 默认）：上下文超限 → 降级最小上下文重试（只保留引导前缀 + 当前问题）
    const llmOnceWithFresh = async (msgs) => {
      const r = await llmStreamOnce(msgs, web, undefined, t);
      if (!r || !r.overflow) return r;
      t.writeln('\x1b[33m(上下文超限 → 降级最小上下文重试)\x1b[0m');
      const fresh = [
        { role: 'system', content: Core.buildGuidePrefix({ guideText: llmConfigured ? '' : (GUIDE_TEXT || L1_TEXT), memoryText: '', toolsTxt, today: localToday() }, { fresh: true }) },
        { role: 'user', content: '问题：' + question },
      ];
      const r2 = await llmStreamOnce(fresh, web, undefined, t);
      if (r2 && r2.overflow) return null;
      if (r2 && !r2.toolJson) t.writeln('\x1b[90m(已丢失历史与检索片段，基于最小上下文回答)\x1b[0m');
      return r2;
    };
    for (;;) {
      t.writeln('\x1b[90m(' + (toolCount ? '继续' : '回答中') + (web ? '·联网' : '') + '...)\x1b[0m');
      const r = await llmOnceWithFresh(messages);
      if (!r) { t.endMsg(); finishExe(t); return; } // 中断/失败（内部已提示）；关闭残留气泡
      if (r.retry) {
        // 联网轮无文本返回（web_search_call 事件轮）→ 重试一轮；计入上限防死循环
        t.writeln('\x1b[90m(联网检索无文本返回，继续等待模型作答…)\x1b[0m');
        toolCount++;
        if (toolCount >= MAX_TOOL) { t.endMsg(); finishExe(t); return; }
        continue;
      }
      if (!r.toolJson) {
        // 最终回答：llmStreamOnce 已流式渲染；收尾信息用最后一轮
        full = r.full;
        reasoning = r.reasoning;
        reasoningStartAt = r.reasoningStartAt;
        firstContentAt = r.firstContentAt;
        lastUsage = r.lastUsage;
        break;
      }
      // 工具调用轮：终端审计行 + 执行 + 结果回填
      const tj = r.toolJson;
      const toolRow = createToolRow(t, tj.tool, JSON.stringify(tj.args || {}));
      toolRow.running();
      let result;
      let toolFail = null;
      try { result = await runTool(tj.tool, tj.args); }
      catch (e) { toolFail = ((e && e.message) || e); result = '工具调用失败: ' + toolFail; touchExperience('tool_failure', tj.tool + ': ' + toolFail); }
      if (toolFail) toolRow.fail(toolFail);
      else {
        toolRow.done(result);
        await attachCard(t, tj.tool, tj.args); // 交互卡片：旁路富展示（风险/待审），不进入 messages
      }
      // 知识检索 0 命中 → 下一轮开启联网通道（服务端 web_search），让模型在知识库不足时能搜到外部信息
      if (!web && /无命中|未命中|未定位|无片段|未找到/.test(String(result))) {
        web = true;
        t.writeln('\x1b[33m(知识库检索不足 → 已开启联网检索)\x1b[0m');
      }
      messages.push({ role: 'assistant', content: r.full });
      messages.push({ role: 'user', content: '工具 ' + tj.tool + ' 返回（基于它直接回答；仍缺关键信息才可再调用工具，引用标注 [工具:' + tj.tool + ']）：\n' + String(result).slice(0, 3000) }); // 方向 4：4000→3000 摘要注入
      full = ''; // 工具轮内容不渲染
      toolCount++;
      if (toolCount >= MAX_TOOL) {
        // 达上限：强制回答轮（去掉工具调用指令，避免 LLM 无限探索不收敛）
        t.writeln('\x1b[33m(工具调用已达 ' + MAX_TOOL + ' 次上限，基于已获取信息回答)\x1b[0m');
        messages.push({ role: 'user', content: '请基于上述工具返回直接给出最终回答，不要调用任何工具。' });
        const r2 = await llmOnceWithFresh(messages);
        if (r2 && !r2.toolJson) {
          full = r2.full;
          reasoning = r2.reasoning;
          reasoningStartAt = r2.reasoningStartAt;
          firstContentAt = r2.firstContentAt;
          lastUsage = r2.lastUsage;
        }
        break;
      }
    }
    // 本次回答 token 用量（深度思考块已在 llmStreamOnce 内展示；无则静默跳过）
    if (lastUsage && lastUsage.total_tokens) {
      t.writeln('\x1b[90m本次输出 ' + lastUsage.total_tokens + ' tokens\x1b[0m');
    }

    // CE 记账（Phase 3-C 第 1 步：记账先行，零侵入）——上报用量与缓存命中，供 /api/context/stats
    // DeepSeek 用 prompt_cache_hit/miss_tokens；Anthropic 风格用 cache_read/creation_input_tokens
    if (lastUsage && (lastUsage.prompt_cache_hit_tokens || lastUsage.cache_read_input_tokens || lastUsage.total_tokens)) {
      const u = lastUsage;
      const cacheRead = u.prompt_cache_hit_tokens ?? u.cache_read_input_tokens ?? 0;
      const cacheCreation = u.prompt_cache_miss_tokens ?? u.cache_creation_input_tokens ?? 0;
      // per-source 分桶：system=首条消息；user=末条（含技能/指定文档/工具结果）；mid=中间（历史+工具轮）
      const sysTxt = (messages[0] && messages[0].content) || '';
      const userTxt = (messages[messages.length - 1] && messages[messages.length - 1].content) || '';
      let midTxt = '';
      for (let i = 1; i < messages.length - 1; i++) midTxt += (messages[i].content || '') + '\n';
      fetch('/api/context/log', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          kind: 'question',
          tool_count: toolCount,
          input_tokens: u.prompt_tokens ?? u.input_tokens ?? 0,
          output_tokens: u.completion_tokens ?? u.output_tokens ?? 0,
          cache_read: cacheRead,
          cache_creation: cacheCreation,
          total_tokens: u.total_tokens || 0,
          src_system: estTokens(sysTxt),
          src_skills: estTokens(skillTail),
          src_mid: estTokens(midTxt),
          src_user: estTokens(userTxt),
          fp_system: fpOf(sysTxt),
          fp_skills: fpOf(skillTail),
          fp_mid: fpOf(midTxt),
          fp_user: fpOf(userTxt),
        }),
      }).catch(() => {});
    }


    const cleanFull = full.replace(/\n?<!--\s*md-agent-save\s*-->[\s\S]*$/, '').trim();

    // 4. 写回沉淀（解析回答末尾的 md-agent-save 块 → 进待审）
    const save = parseSaveBlock(full);
    if (save) {
      try {
        const savedPath = await applySave(save);
        t.writeln('\x1b[33m💾 已进入待审: \x1b[0m' + savedPath + '  （/view automation 审核 · /approve 确认 · /reject 丢弃）');
      } catch (e) {
        t.writeln('\x1b[31m写回失败: ' + e.message + '\x1b[0m');
      }
    }

    // 5. 多轮记忆 + L0 会话快照：按执行归属写入——
    // 活跃执行写当前会话（全局状态）；后台执行（切走完成）写 exe 所属会话文件（不动当前全局状态）
    if (t.exe.active) {
      history.push({ role: 'user', content: question });
      if (cleanFull) history.push({ role: 'assistant', content: cleanFull });
      if (history.length > MAX_HISTORY) history = history.slice(-MAX_HISTORY);
      saveHistory();
      sessionLog.push({ q: question, a: cleanFull || '(无回答/中断)', ts: Date.now() });
      if (sessionLog.length > MAX_SESSION_LOG) sessionLog = sessionLog.slice(-MAX_SESSION_LOG);
      scheduleL0Snapshot();
    } else if (t.exe.key) {
      // 后台完成：追加问答对到 exe 所属会话文件（缓冲已渲染过/待重放，记录必须归属正确）
      appendSessionQA(t.exe.key, question, cleanFull || '(无回答/中断)');
    } else {
      // 后台首问完成：无归属文件 → 生成新会话文件落盘（回答不丢，恢复时可见）
      appendSessionQA('sessions/' + sessionStamp() + '.md', question, cleanFull || '(无回答/中断)');
    }

    if (top.length) {
      t.writeln('\x1b[1;32m──── 引用来源 ────\x1b[0m');
      const seen = new Set();
      for (const h of top) {
        const k = h.file + ':' + h.line;
        if (seen.has(k)) continue;
        seen.add(k);
        t.writeln('\x1b[90m' + k + '  ' + (h.section || '') + '\x1b[0m');
      }
    }
    enhanceRefs(); // 回答渲染完成后：把 [文件:行号] 引用增强为可点击（点击 → 图谱高亮）
    t.endMsg(); // 关闭本轮回答气泡（工具轮无气泡，无操作）
    finishExe(t); // 回答完成：标记执行结束（运行图标清除；首问迁移归属）
  }

  // ---------- App → Agent（agent:ask 执行体）：App 提问走 Agent 回路 ----------
  // 上下文 = 引导前缀 + 工具清单 + 问题 + 应用上下文（context，MCP Apps 式结构化片段，不拼进提问文本）；
  // 回答流式渲染到终端（可审计）+ 按行推回 App；工具调用事件同步推送；完成信号携带结构化 data（约定标记提取）
  async function runAsApp(text, tab, context, spaceHint) {
    const post = (m) => { try { tab.iframe.contentWindow.postMessage(m, '*'); } catch (e) { /* 视图已关 */ } };
    const t = beginExe(); // App 请求同样绑定当前会话（后台执行语义）
    // 终端显示 App 提问（用户气泡样式，可审计）
    t.beginMsg('user');
    t.writeln(renderInline ? renderInline('[App: ' + tab.title + '] ' + text) : '[App: ' + tab.title + '] ' + text);
    t.endMsg(true);
    // context 结构化入参：数组/单对象统一规范化；单片段 4000 字、总长 8000 字截断
    let ctxFrag = '';
    const ctxList = Array.isArray(context) ? context : (context ? [context] : []);
    const ctxParts = [];
    for (const c of ctxList) {
      if (!c) continue;
      const title = String(c.title || '上下文片段').slice(0, 80);
      const content = String(c.content || c.text || '').slice(0, 4000);
      if (content) ctxParts.push('[' + title + ']\n' + content);
    }
    if (ctxParts.length) ctxFrag = '\n\n[应用上下文]\n' + ctxParts.join('\n\n').slice(0, 8000);
    const tools = await getTools();
    const toolsTxt = tools.map((tool) =>
      '  - ' + tool.name + '(' + tool.params.map((p) => p.name + (p.required ? '' : '?')).join(', ') + '): ' + tool.desc +
      (tool.example ? ' | 例: ' + tool.example : '')
    ).join('\n');
    const system = Core.buildGuidePrefix({
      guideText: llmConfigured ? '' : (GUIDE_TEXT || L1_TEXT),
      memoryText: llmConfigured ? '' : ((await getMemorySummary()) || MEMORY_TEXT),
      toolsTxt,
      today: localToday(),
    }) + (tab.appId
      ? '\n\n【应用任务授权】你是 ' + tab.title + ' 的智能分析引擎（受信任本地组件经授权通道调用）。' +
        '应用请求你执行的分析/评估/提炼/抽取/生成任务属于你的正常职责——请按应用指令完成，' +
        '可先检索知识库或调用工具获取依据再作答；不要以「知识库助手」身份拒绝应用的分析请求，' +
        '也不要输出与任务无关的免责声明。'
      : '');
    const messages = [
      { role: 'system', content: system },
      { role: 'user', content: '问题：' + text + ctxFrag + (spaceHint || '') },
    ];
    let full = '';
    let toolCount = 0;
    const MAX_TOOL = 8;
    for (;;) {
      t.writeln('\x1b[90m(App 请求' + (toolCount ? ' · 继续' : '') + '...)\x1b[0m');
      const r = await llmStreamOnce(messages, false, (line) => post({ type: 'agent:chunk', text: line, done: false }), t);
      if (!r) { post({ type: 'agent:error', message: '回答中断' }); finishExe(t); return; }
      if (!r.toolJson) { full = r.full; break; }
      // 工具轮：终端审计卡 + App 事件
      const tj = r.toolJson;
      const toolRow = createToolRow(t, tj.tool, JSON.stringify(tj.args || {}));
      toolRow.running();
      post({ type: 'agent:tool', name: tj.tool, status: 'running' });
      let result;
      try {
        // 应用上下文限定（Phase A）：应用 agent 的 dev.patch 只能提案修改自己的应用目录，防改主项目/其他应用
        if (tj.tool === 'dev.patch' && tab.appId) {
          const f2 = (tj.args && tj.args.files) || [];
          const bad = !f2.length || f2.some((x) => !String((x && x.path) || '').startsWith('apps/' + tab.appId + '/'));
          if (bad) throw new Error('应用只能提案修改自己的目录（apps/' + tab.appId + '/）');
        }
        result = await runTool(tj.tool, tj.args);
      }
      catch (e) { result = '工具调用失败: ' + ((e && e.message) || e); toolRow.fail((e && e.message) || e); }
      if (!/调用失败/.test(result)) {
        toolRow.done(result);
        await attachCard(t, tj.tool, tj.args); // 交互卡片：旁路富展示（风险/待审），不进入 messages
      }
      post({ type: 'agent:tool', name: tj.tool, status: 'done', result: String(result).slice(0, 500) });
      messages.push({ role: 'assistant', content: r.full });
      messages.push({ role: 'user', content: '工具 ' + tj.tool + ' 返回（基于它直接回答；仍缺关键信息才可再调用工具，引用标注 [工具:' + tj.tool + ']）：\n' + String(result).slice(0, 3000) });
      toolCount++;
      if (toolCount >= MAX_TOOL) {
        t.writeln('\x1b[33m(工具调用已达 ' + MAX_TOOL + ' 次上限，基于已获取信息回答)\x1b[0m');
        messages.push({ role: 'user', content: '请基于上述工具返回直接给出最终回答，不要调用任何工具。' });
        const r2 = await llmStreamOnce(messages, false, (line) => post({ type: 'agent:chunk', text: line, done: false }), t);
        if (r2 && !r2.toolJson) full = r2.full;
        break;
      }
    }
    // 完成信号：剥离约定标记 + 提取结构化 data 回推（App 直接渲染；旧应用忽略 data 字段零破坏）
    const clean = Core.extractAppData(full);
    post({ type: 'agent:chunk', text: '', done: true, data: clean.data, textFull: clean.text });
    finishExe(t);
  }

  // ---------- 引用增强：回答/工具结果中的 [文件:行号] → 可点击（点击打开图谱并高亮该文档局部图） ----------
  // 遍历 #stream 内所有行与工具卡展开内容（.stream-row / .tr-body）的文本节点替换，
  // 不动 ANSI→HTML 渲染链路；命中路径在图谱中才可点击。工具轮气泡已关闭，
  // 故不能只查 currentMsg，需全流扫描（只处理本轮新增：增强过的行有 .ref-link 会跳过）。
  const REF_RE = /\[([^\s\]]+?\.md):(\d+)[^\]]*\]/g;
  function enhanceRefs() {
    const rows = document.querySelectorAll('#stream .stream-row, #stream .tr-body');
    for (const row of rows) {
      walkTextNodes(row, (node) => {
        const txt = node.nodeValue;
        if (!txt || !REF_RE.test(txt)) return;
        REF_RE.lastIndex = 0;
        const frag = document.createDocumentFragment();
        let last = 0, m;
        while ((m = REF_RE.exec(txt))) {
          if (m.index > last) frag.appendChild(document.createTextNode(txt.slice(last, m.index)));
          const path = m[1];
          const a = document.createElement('span');
          a.className = 'ref-link';
          a.dataset.path = path;
          a.textContent = m[0];
          a.title = '在知识图谱中查看「' + path + '」';
          frag.appendChild(a);
          last = m.index + m[0].length;
        }
        if (last < txt.length) frag.appendChild(document.createTextNode(txt.slice(last)));
        node.parentNode.replaceChild(frag, node);
      });
    }
  }
  // 遍历文本节点（跳过已有 .ref-link）
  function walkTextNodes(root, fn) {
    const it = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
      acceptNode(n) {
        if (n.parentNode && n.parentNode.classList && n.parentNode.classList.contains('ref-link')) return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      },
    });
    let n;
    const targets = [];
    while ((n = it.nextNode())) targets.push(n);
    for (const t of targets) fn(t);
  }
  // 点击引用 → 打开图谱面板 + 高亮（图谱加载完成后经 postMessage 通知；BRIDGE 无 highlight 需宿主转发）
  document.addEventListener('click', (e) => {
    const el = e.target.closest('.ref-link');
    if (!el || !el.dataset.path) return;
    e.preventDefault();
    openViewForPath(el.dataset.path);
  });
  let graphHighlightPending = null;
  async function openViewForPath(path) {
    try {
      await viewCmd('graph');
      graphHighlightPending = path;
      // 等图谱 iframe 加载 + 数据就绪后注入高亮
      setTimeout(() => {
        if (!currentView || currentView.specArg !== 'graph') return;
        try {
          currentView.iframe.contentWindow.postMessage({ type: 'highlight', path }, '*');
        } catch (err) { /* iframe 未就绪则跳过 */ }
        graphHighlightPending = null;
      }, 400);
    } catch (err) { /* 打开失败静默 */ }
  }

  // ---------- 写回（Agent 沉淀） ----------

  // 解析回答末尾的写回块：<!-- md-agent-save --> + JSON
  function parseSaveBlock(answer) {
    const MARK = '<!-- md-agent-save -->';
    const i = answer.indexOf(MARK);
    if (i === -1) return null;
    const raw = answer.slice(i + MARK.length).trim();
    try {
      const obj = JSON.parse(raw);
      if (obj && typeof obj.path === 'string' && typeof obj.content === 'string') return obj;
    } catch (e) { /* 非 JSON 则忽略 */ }
    return null;
  }

  async function fileExists(path) {
    return (await fetch('/api/file?path=' + encodeURIComponent(path))).ok;
  }

  async function getFileContent(path) {
    const r = await api('/api/file?path=' + encodeURIComponent(path));
    return r.content || '';
  }

  // /fetch <url> [标题] —— 静态抓取网页：阅读视图；带标题则全文沉淀为待审笔记
  async function fetchCmd(parts) {
    const url = parts[0];
    if (!url) {
      term.writeln('\x1b[33m用法: /fetch <url> [标题]\x1b[0m');
      term.writeln('\x1b[90m  不带标题 → 仅抓取并展示正文前 4000 字（阅读视图）\x1b[0m');
      term.writeln('\x1b[90m  带标题   → 全文写入 notes/ 待审，/approve 后入库\x1b[0m');
      return;
    }
    const titleArg = parts.slice(1).join(' ').trim();
    term.writeln('\x1b[90m(抓取 ' + url + ' …)\x1b[0m');
    let r;
    try {
      r = await api('/api/fetch?url=' + encodeURIComponent(url));
    } catch (e) {
      term.writeln('\x1b[31m抓取失败: ' + e + '\x1b[0m');
      return;
    }
    term.writeln('\x1b[1;36m' + r.title + '\x1b[0m' + (r.truncated ? ' \x1b[33m(正文已截断至 2 万字)\x1b[0m' : ''));
    term.writeln('\x1b[90m来源: ' + url + '\x1b[0m');
    term.writeln('─'.repeat(48));
    r.text.slice(0, 4000).split('\n').forEach((l) => term.writeln(l));
    if (r.text.length > 4000) term.writeln('\x1b[90m… 正文共 ' + r.text.length + ' 字\x1b[0m');
    term.writeln('─'.repeat(48));
    if (!titleArg) {
      term.writeln('\x1b[90m(沉淀全文 → /fetch <url> <标题>)\x1b[0m');
      return;
    }
    const safe = (titleArg.replace(/[\\/:*?"<>|\s]+/g, '-').replace(/^-+|-+$/g, '') || '网页摘录').slice(0, 30);
    const content = '# ' + titleArg + '\n\n> 来源: ' + url + '\n\n' + r.text + '\n';
    const saved = await applySave({ path: 'notes/' + safe + '.md', mode: 'new', content });
    term.writeln('\x1b[33m💾 已进入待审: \x1b[0m' + saved + '  （/view automation 审核 · /approve 确认 · /reject 丢弃）');
  }

  // 任务驱动会话（调研结论：会话按任务/主题划分，不用空闲时钟切分）：
  // 新建任务成功 → 若当前会话有对话则归档收尾 → 开新会话并绑定任务 id（frontmatter task: <id>）。
  // 同一任务后续对话续用本会话；/resume 恢复时按 frontmatter task 还原绑定。
  function beginTaskSession(taskId, title) {
    if (sessionLog.length && !sessionArchived) archiveSession().catch(() => {});
    history = [];
    saveHistory();
    Core.resetSectionCache();
    sessionFile = null;
    sessionLog = [];
    sessionArchived = false;
    sessionTaskId = String(taskId);
    term.writeln('\x1b[90m(已开启任务会话 #' + taskId + '：后续对话归入此会话，frontmatter 记录 task: ' + taskId + ')\x1b[0m');
    if (topbarTitle) topbarTitle.textContent = String(title || '任务 #' + taskId).slice(0, 30);
  }

  // /task —— Phase 3-B 任务引擎：文字看板 + 流转/依赖/日志（HTML 看板: /task board）
  async function taskCmd(parts) {
    const sub = parts[0] || 'list';
    const s = parts.slice(1);
    const idOf = (x) => parseInt(x, 10);
    const patch = async (id, body) => {
      const r = await api('/api/tasks/' + id, { method: 'PATCH', body: JSON.stringify(body) });
      return r.task;
    };
    if (sub === 'new') {
      const ti = s.indexOf('--title');
      const title = ti >= 0 ? s.slice(ti + 1).join(' ') : '';
      const goal = (ti >= 0 ? s.slice(0, ti) : s).join(' ').trim();
      if (!goal) { term.writeln('\x1b[33m用法: /task new <目标> [--title <标题>]\x1b[0m'); return; }
      const r = await api('/api/tasks', { method: 'POST', body: JSON.stringify({ goal, title }) });
      term.writeln('\x1b[32m✓ 新任务 #' + r.task.id + '\x1b[0m ' + (r.task.title || r.task.goal));
      beginTaskSession(r.task.id, title || goal); // 任务驱动会话：新任务=新会话
      return;
    }
    if (sub === 'board') { await viewCmd('board'); return; }
    if (sub === 'rm') {
      await api('/api/tasks/' + idOf(s[0]), { method: 'DELETE' });
      term.writeln('\x1b[32m✓ 已删除任务 #' + s[0] + '\x1b[0m');
      return;
    }
    if (sub === 'start' || sub === 'done' || sub === 'drop') {
      const st = { start: 'doing', done: 'done', drop: 'dropped' }[sub];
      const id = idOf(s[0]);
      const note = s.slice(1).join(' ').trim() || null;
      const t = await patch(id, { status: st, note });
      term.writeln('\x1b[32m✓ 任务 #' + id + ' → ' + t.status + '\x1b[0m' + (t.title ? ' ' + t.title : ''));
      if (note) term.writeln('  \x1b[90m日志: ' + note + '\x1b[0m');
      return;
    }
    if (sub === 'note') {
      const id = idOf(s[0]);
      const note = s.slice(1).join(' ').trim();
      if (!note) { term.writeln('\x1b[33m用法: /task note <id> <内容>\x1b[0m'); return; }
      await patch(id, { note });
      term.writeln('\x1b[32m✓ 已追加日志到 #' + id + '\x1b[0m');
      return;
    }
    if (sub === 'dep') {
      const id = idOf(s[0]);
      const deps = s.slice(1);
      if (!id || !deps.length) { term.writeln('\x1b[33m用法: /task dep <id> <依赖id...>\x1b[0m'); return; }
      await patch(id, { deps });
      term.writeln('\x1b[32m✓ 依赖已更新: #' + id + ' ← #' + deps.join(' #') + '\x1b[0m');
      return;
    }
    if (sub === 'plan') {
      const goal = s.join(' ').trim();
      if (!goal) {
        term.writeln('\x1b[33m用法: /task plan <目标> —— LLM 拆解为串行子任务链\x1b[0m');
        return;
      }
      term.writeln('\x1b[90m(LLM 拆解中…)\x1b[0m');
      const text = await llmText([
        { role: 'system', content:
          '你是任务规划器。把用户目标拆解为 3-8 个可执行的子任务，按依赖顺序排列。' +
          '只输出任务列表，每行一个，格式：- 子任务描述。不要输出任何其他内容。' },
        { role: 'user', content: goal },
      ]);
      const items = text.split('\n').map((l) => l.trim()).filter(Boolean)
        .map((l) => {
          const m = l.match(/^[-*•]\s+(.+)$/) || l.match(/^\d+[.、]\s*(.+)$/);
          return m ? m[1].replace(/^"|"$/g, '').trim() : null;
        })
        .filter((t) => t && t.length > 1);
      if (!items.length) { term.writeln('\x1b[31m拆解结果无法解析:\x1b[0m ' + text.slice(0, 200)); return; }
      term.writeln('\x1b[33m拆解出 ' + items.length + ' 个子任务，创建任务链…\x1b[0m');
      const main = await api('/api/tasks', { method: 'POST', body: JSON.stringify({ goal, title: goal.slice(0, 30) }) });
      let prev = main.task.id;
      for (const it of items) {
        const r = await api('/api/tasks', { method: 'POST', body: JSON.stringify({ goal: it }) });
        await patch(r.task.id, { deps: [String(prev)] }); // 串行依赖链
        prev = r.task.id;
      }
      term.writeln('\x1b[32m✓ 任务链已创建: #' + main.task.id + ' → ' + items.length + ' 个子任务（/task 查看，可删改；依赖未完成时无法流转）\x1b[0m');
      beginTaskSession(main.task.id, goal); // 任务驱动会话：新任务链=新会话
      refreshStatus();
      return;
    }
    // 默认 list：文字看板
    const d = await api('/api/tasks');
    const tasks = d.tasks || [];
    if (!tasks.length) {
      term.writeln('\x1b[90m(暂无任务。新建: /task new <目标> [--title <标题>])\x1b[0m');
      return;
    }
    const stName = { todo: '待办', doing: '进行中', done: '完成', dropped: '已放弃' };
    const stColor = { todo: '\x1b[90m', doing: '\x1b[33m', done: '\x1b[32m', dropped: '\x1b[31m' };
    ['todo', 'doing', 'done', 'dropped'].forEach((st) => {
      const group = tasks.filter((t) => t.status === st);
      if (!group.length) return;
      term.writeln('\x1b[1m── ' + stName[st] + ' (' + group.length + ')\x1b[0m');
      group.forEach((t) => {
        const dep = t.deps.length ? ' \x1b[90m依赖: #' + t.deps.join(' #') + '\x1b[0m' : '';
        term.writeln(stColor[st] + '  #' + t.id + ' ' + (t.title || t.goal) + dep + '\x1b[0m');
      });
    });
    const st = d.stats || {};
    term.writeln('\x1b[90m统计: ' + Object.keys(st).map((k) => k + '=' + st[k]).join(' ') + '\x1b[0m');
    term.writeln('\x1b[90m(流转: start/done/drop <id> [备注]  |  日志: note  |  依赖: dep  |  删除: rm  |  看板: board)\x1b[0m');
  }

  // /page act <url> <json> —— 写侧：动作清单人审确认后执行（click/fill/select/scroll）
  async function pageActCmd(parts) {
    const url = parts[0];
    const rest = parts.slice(1).join(' ');
    if (!url || !rest) {
      term.writeln('\x1b[33m用法: /page act <url> <json 动作数组>\x1b[0m');
      term.writeln('\x1b[90m  例: /page act https://example.com [{"kind":"click","selector":"#btn"},{"kind":"fill","selector":"#q","value":"hello"}]\x1b[0m');
      term.writeln('\x1b[90m  动作: click / fill(值) / select(值) / scroll\x1b[0m');
      return;
    }
    let actions;
    try { actions = JSON.parse(rest); } catch (e) {
      term.writeln('\x1b[31m动作 JSON 解析失败: ' + e.message + '\x1b[0m');
      return;
    }
    if (!Array.isArray(actions) || !actions.length) {
      term.writeln('\x1b[31m动作列表不能为空\x1b[0m');
      return;
    }
    // 人审闭环：先出动作清单，确认后才执行
    term.writeln('\x1b[1m动作清单（人工确认）\x1b[0m');
    actions.forEach((a, i) => {
      const v = a.value !== undefined ? ' = ' + a.value : '';
      term.writeln('  ' + (i + 1) + '. \x1b[36m' + (a.kind || 'click') + '\x1b[0m ' + a.selector + v);
    });
    const ok = await confirm('确认执行以上 ' + actions.length + ' 个动作？');
    if (!ok) { term.writeln('\x1b[90m(已取消，未执行任何操作)\x1b[0m'); return; }
    term.writeln('\x1b[90m(执行中…)\x1b[0m');
    let r;
    try {
      r = await api('/api/page/act', { method: 'POST', body: JSON.stringify({ url, actions }) });
    } catch (e) {
      term.writeln('\x1b[31m执行失败: ' + e + '\x1b[0m');
      return;
    }
    term.writeln('\x1b[32m✓ ' + r.message + '\x1b[0m' + (r.title ? ' · ' + r.title : ''));
    term.writeln('─'.repeat(40));
    (r.text || '').split('\n').slice(0, 20).forEach((l) => term.writeln(l));
    if ((r.text || '').length > 800) term.writeln('\x1b[90m…\x1b[0m');
  }

  // /page <url> [标题] —— 动态网页读取（headless Edge/Chrome，等待 JS 渲染后取正文）
  async function pageCmd(parts) {
    // /page act <url> <json 动作数组>：写侧——动作清单人审确认后执行
    if (parts[0] === 'act') {
      await pageActCmd(parts.slice(1));
      return;
    }
    const url = parts[0];
    if (!url) {
      term.writeln('\x1b[33m用法: /page <url> [标题]\x1b[0m');
      term.writeln('\x1b[90m  动态/JS 渲染页面用本命令；纯静态页面建议 /fetch（更快）\x1b[0m');
      term.writeln('\x1b[90m  操作页面: /page act <url> <json 动作数组>（动作清单人工确认后执行）\x1b[0m');
      return;
    }
    const titleArg = parts.slice(1).join(' ').trim();
    term.writeln('\x1b[90m(动态读取 ' + url + ' … 约 5-10s)\x1b[0m');
    let r;
    try {
      r = await api('/api/page?url=' + encodeURIComponent(url));
    } catch (e) {
      term.writeln('\x1b[31m读取失败: ' + e + '\x1b[0m');
      return;
    }
    term.writeln('\x1b[1;36m' + r.title + '\x1b[0m' + (r.truncated ? ' \x1b[33m(正文已截断至 2 万字)\x1b[0m' : ''));
    term.writeln('\x1b[90m来源: ' + url + '  [' + r.engine + ' headless]\x1b[0m');
    term.writeln('─'.repeat(48));
    r.text.slice(0, 4000).split('\n').forEach((l) => term.writeln(l));
    if (r.text.length > 4000) term.writeln('\x1b[90m… 正文共 ' + r.text.length + ' 字\x1b[0m');
    term.writeln('─'.repeat(48));
    if (!titleArg) {
      term.writeln('\x1b[90m(沉淀全文 → /page <url> <标题>)\x1b[0m');
      return;
    }
    const safe = (titleArg.replace(/[\\/:*?"<>|\s]+/g, '-').replace(/^-+|-+$/g, '') || '网页摘录').slice(0, 30);
    const content = '# ' + titleArg + '\n\n> 来源: ' + url + '\n\n' + r.text + '\n';
    const saved = await applySave({ path: 'notes/' + safe + '.md', mode: 'new', content });
    term.writeln('\x1b[33m💾 已进入待审: \x1b[0m' + saved + '  （/view automation 审核 · /approve 确认 · /reject 丢弃）');
  }

  // 写回落盘（Phase 3 前置：待审机制）
  // pending=true（默认）：LLM 生成的新笔记 / MEMORY 条目先进 pending/，/approve 确认后落地
  // pending=false（/remember 用户主动沉淀）：直接写盘 + 刷新 INDEX/图谱
  async function applySave(save, opts = {}) {
    const pending = opts.pending !== false;
    const path = save.path.trim().replace(/\\/g, '/').replace(/^\/+/, '');
    let content = save.content.trim();
    if (!path || !content) return '写回内容为空';
    if (path === 'INDEX.md') return '拒绝写回自动生成的 INDEX.md';
    const today = localToday();
    const title = (content.match(/^#\s+(.+)/m) || [])[1] ||
      path.split('/').pop().replace(/\.md$/i, '') || '未命名';

    if (pending) {
      // —— 待审：不直接落知识库 ——
      let target;
      if (path === 'MEMORY.md') {
        target = 'pending/MEMORY.' + Date.now() + '.md';
        // 保留条目原文（approve 时后端按当日小节合并）
        if (!/^##\s/.test(content)) content = '## ' + today + '\n' + content;
        // 记忆摘要只收 `- `/`* ` 开头行：裸文本正文 bullet 化，否则该记忆对后续会话不可见
        content = content.split('\n').map((l, i) => {
          if (i === 0 && /^##\s/.test(l)) return l; // 日期标题保留
          const t = l.trim();
          if (!t || /^[-*]\s/.test(t)) return l; // 已是 bullet 或空行保留
          return '- ' + l; // 其余正文加 bullet
        }).join('\n');
        // 记忆断链修复 B：自动生成到相关 L2 文档的双链建议（进待审人审确认，编辑后批准可增删）
        try {
          const sug = await api('/api/link/suggest', {
            method: 'POST', headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ content: content.replace(/^##\s+[^\n]*\n?/, '') }), // 去掉日期标题，避免日期词噪声
          });
          const fresh = (sug.links || [])
            .map((l) => (l.path || '').split('/').pop().replace(/\.md$/i, ''))
            .filter((s) => s && !content.includes('[[' + s + ']]'));
          if (fresh.length) content += '\n相关：' + fresh.map((s) => '[[' + s + ']]').join(' ');
        } catch (e) { /* 关联建议失败不阻塞写回 */ }
      } else {
        target = 'pending/' + path;
        if (!/^---/.test(content)) {
          content = '---\ntype: note\ntitle: ' + title + '\ntags: []\nupdated: ' + today + '\n---\n\n' + content;
        }
      }
      await api('/api/file', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ path: target, content }),
      });
      return target;
    }

    // —— 直接写（用户主动沉淀）——
    const exists = await fileExists(path);
    const old = exists ? await getFileContent(path) : '';
    if (!exists) {
      if (path === 'MEMORY.md') {
        content = '## ' + today + '\n- ' + content.replace(/^[-#\s]+/, '');
      } else {
        content = '---\ntype: note\ntitle: ' + title + '\ntags: []\nupdated: ' + today + '\n---\n\n' + content;
      }
    } else if (path === 'MEMORY.md' && !/^##\s/.test(content)) {
      // 已有当日小节则加 bullet，否则新起标题
      content = (old.includes('## ' + today) ? '- ' : '## ' + today + '\n- ') + content;
    }
    const merged = exists ? old.trimEnd() + '\n\n' + content + '\n' : content;
    await api('/api/file', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path, content: merged }),
    });
    try { await api('/api/kb/sync', { method: 'POST' }); } catch (e) { /* 不阻塞 */ }
    try { await api('/api/graph/sync', { method: 'POST' }); } catch (e) { /* 不阻塞 */ }
    return path;
  }

  // 手动沉淀：/remember [路径] 内容（默认 MEMORY.md）
  async function remember(parts) {
    if (!parts.length) {
      term.writeln('\x1b[33m用法：/remember [路径] 内容（默认追加到 MEMORY.md）\x1b[0m');
      return;
    }
    let path = 'MEMORY.md';
    let text = parts.join(' ');
    if (parts[0].includes('/') || /\.md$/i.test(parts[0])) {
      path = parts.shift();
      text = parts.join(' ');
    }
    if (!text) {
      term.writeln('\x1b[33m内容为空\x1b[0m');
      return;
    }
    const saved = await applySave({ path, mode: 'append', content: text }, { pending: false });
    term.writeln('\x1b[32m✓ 已沉淀: \x1b[0m' + saved);
  }

  // ---------- 检索整理成新笔记 ----------

  // 非流式文本补全（digest 用，返回完整文本）
  async function llmText(messages) {
    const llm = await api('/api/llm', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ messages }),
    });
    const t = llm.choices && llm.choices[0] && llm.choices[0].message && llm.choices[0].message.content;
    if (!t) throw new Error('LLM 响应异常: ' + JSON.stringify(llm).slice(0, 200));
    return t;
  }

  // /digest <主题>：检索知识库 → LLM 整理成结构化笔记 → 写入 notes/
  async function digest(topic) {
    if (!topic) {
      term.writeln('\x1b[33m用法：/digest <主题> —— 检索知识库并把结果整理成新笔记写入 notes/\x1b[0m');
      return;
    }
    term.writeln('\x1b[90m(整理: 检索「' + topic + '」→ LLM 生成笔记 → 写入 L2)\x1b[0m');
    const kws = Core.extractKeywords(topic);
    const query = kws.length ? kws.join(' ') : topic;
    const sr = await api('/api/search?q=' + encodeURIComponent(query) + '&layer=notes&ctx=1');
    const top = sr.hits.slice(0, 15);
    if (!top.length) {
      term.writeln('\x1b[33m知识库无相关片段，无法整理。\x1b[0m');
      return;
    }
    term.writeln('\x1b[90m命中 ' + sr.file_count + ' 文件 / ' + sr.hit_count + ' 处，整理前 ' + top.length + ' 条\x1b[0m');
    const frag = top
      .map((h) => '[来源 ' + h.file + ':' + h.line + ']\n' + (h.context || h.text))
      .join('\n\n');
    const system =
      '你是知识整理助手。把给定的检索片段整理成一篇结构化 Markdown 笔记：' +
      '以 # 标题开头，包含 概述、要点列表、相关条目（条目标注来源 [文件:行号]）。' +
      '直接输出笔记正文，不要额外解释。';
    // 知识链路：取前 3 个命中文件的关联簇，让整理沿知识网络走
    let chain = [];
    try {
      const seen = new Set(top.map((h) => h.file));
      for (const h of top.slice(0, 3)) {
        const rel = await api('/api/graph/related?path=' + encodeURIComponent(h.file));
        for (const p of rel.related) {
          if (!seen.has(p)) { seen.add(p); chain.push(p); }
        }
      }
    } catch (e) { /* 图谱不可用则忽略 */ }
    const userMsg =
      '主题：' + topic + '\n\n检索片段：\n' + frag +
      (chain.length
        ? '\n\n知识链路（关联文档，整理时可引用其脉络）:\n' + chain.map((p) => '- ' + p).join('\n')
        : '');
    const noteText = await llmText([
      { role: 'system', content: system },
      { role: 'user', content: userMsg },
    ]);
    term.writeln('\x1b[1;32m──── 生成笔记 ────\x1b[0m');
    term.writeln(renderMdFile(noteText));
    const safe = (topic.replace(/[\\/:*?"<>|\s]+/g, '-').replace(/^-+|-+$/g, '') || '整理笔记').slice(0, 30);
    const saved = await applySave({ path: 'notes/' + safe + '.md', mode: 'new', content: noteText });
    term.writeln('\x1b[32m✓ 笔记已进入待审: \x1b[0m' + saved + '  （/view automation 审核 · /approve 确认 · /reject 丢弃）');
  }

  // ---------- 知识图谱命令 ----------

  async function graph(path) {
    if (!path) {
      // 无参：结构导航（目录树 + 度数 + 孤立标记）——信息密度比全图高，不发散
      const [g, orph] = await Promise.all([
        api('/api/graph/graph'),
        api('/api/graph/orphans').catch(() => null),
      ]);
      const orphans = orph && orph.orphans ? orph.orphans : [];
      const byPath = {};
      g.nodes.forEach((n) => { byPath[n.path] = n; });
      // 目录树（按路径层级缩进）
      const root = {};
      g.nodes.forEach((n) => {
        const parts = n.path.split('/');
        let cur = root;
        for (let i = 0; i < parts.length - 1; i++) {
          cur.dirs = cur.dirs || {};
          cur = cur.dirs[parts[i]] = cur.dirs[parts[i]] || {};
        }
        cur.files = cur.files || [];
        cur.files.push(n);
      });
      term.writeln('\x1b[1m知识库结构（' + g.nodes.length + ' 篇 · ' + g.edges.length + ' 链接）\x1b[0m');
      const walk = (node, depth) => {
        if (node.dirs) Object.keys(node.dirs).sort().forEach((d) => {
          term.writeln('  '.repeat(depth) + '\x1b[90m▸\x1b[0m ' + d + '/');
          walk(node.dirs[d], depth + 1);
        });
        if (node.files) node.files.sort((a, b) => a.path < b.path ? -1 : 1).forEach((n) => {
          const iso = orphans.includes(n.path);
          term.writeln('  '.repeat(depth) +
            (iso ? '\x1b[31m●\x1b[0m ' : '\x1b[36m·\x1b[0m ') +
            (n.title || n.path.split('/').pop()) +
            (iso ? ' \x1b[31m(孤立)\x1b[0m' : '') +
            '\x1b[90m [' + n.in_degree + '↔' + n.out_degree + ']\x1b[0m');
        });
      };
      walk(root, 0);
      if (orphans.length) term.writeln('\x1b[31m孤立 ' + orphans.length + ' 篇（无入链也无出链）\x1b[0m');
      term.writeln('\x1b[90m(查看单篇链接: /graph <路径> · 可视化: /view graph)\x1b[0m');
      return;
    }
    const [bl, lk, rel] = await Promise.all([
      api('/api/graph/backlinks?path=' + encodeURIComponent(path)),
      api('/api/graph/linked?path=' + encodeURIComponent(path)),
      api('/api/graph/related?path=' + encodeURIComponent(path)),
    ]);
    term.writeln('\x1b[1;36m' + path + '\x1b[0m');
    term.writeln('  出链（' + lk.linked.length + '）:');
    for (const l of lk.linked) {
      term.writeln('    \x1b[90m[[\x1b[0m' + l.dst + '\x1b[90m]]\x1b[0m ' + (l.resolved ? '\x1b[32m→ ' + l.dst_path + '\x1b[0m' : '\x1b[33m→ (悬空)\x1b[0m'));
    }
    term.writeln('  入链（' + bl.backlinks.length + '）:');
    for (const s of bl.backlinks) term.writeln('    \x1b[90m←\x1b[0m ' + s);
    term.writeln('  关联簇（' + rel.related.length + '）: ' + rel.related.join('  '));
  }

  async function orphans() {
    const r = await api('/api/graph/orphans');
    if (!r.orphans.length) { term.writeln('无孤立文档。'); return; }
    term.writeln('孤立文档（' + r.orphans.length + '）——无入链也无出链:');
    for (const p of r.orphans) term.writeln('  \x1b[33m' + p + '\x1b[0m');
  }

  async function projects() {
    const r = await api('/api/graph/projects');
    term.writeln('项目维度统计:');
    for (const p of r.projects) term.writeln('  \x1b[1;36m' + p.project + '\x1b[0m  ' + p.docs + ' 篇');
  }

  async function tags() {
    const r = await api('/api/graph/tags');
    if (!r.tags.length) { term.writeln('暂无标签。'); return; }
    term.writeln('标签统计:');
    for (const t of r.tags) term.writeln('  \x1b[1;36m' + t.tag + '\x1b[0m  ' + t.docs + ' 篇');
  }

  async function rescan() {
    term.writeln('重建知识图谱...');
    const r = await api('/api/graph/sync', { method: 'POST' });
    const g = r.graph;
    term.writeln('\x1b[32m✓\x1b[0m 图谱已重建: ' + g.docs + ' 文档 / ' + g.links + ' 链接 / ' + g.dangling + ' 悬空');
  }

  // ---------- 待审机制（生成 → 预览 → 确认） ----------

  async function pendingList() {
    const r = await api('/api/kb/pending');
    if (!r.pending.length) {
      term.writeln('待审区为空（LLM 写回/生成笔记先进这里，/approve 确认后落地）。');
      return;
    }
    term.writeln('待审文件（/approve <路径或 all> 确认 | /reject 丢弃 | open 预览）:');
    for (const p of r.pending) {
      term.writeln(
        '  ' + ({ memory: '\x1b[33m[记忆]\x1b[0m', skill: '\x1b[35m[技能]\x1b[0m', consolidate: '\x1b[36m[巩固]\x1b[0m' }[p.kind] || '\x1b[34m[笔记]\x1b[0m') +
        '  ' + p.path + (p.title ? '  \x1b[90m(' + p.title + ')\x1b[0m' : '')
      );
    }
  }

  async function pendingAct(act, name) {
    if (!name) {
      term.writeln('\x1b[33m用法：/' + act + ' <路径或 all>\x1b[0m');
      return;
    }
    const r = await api('/api/kb/pending/' + act, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: name }),
    });
    if (r.errors && r.errors.length) {
      term.writeln('\x1b[31m' + act + ' 失败: ' + r.errors.join('; ') + '\x1b[0m');
    }
    if (r.ok && r.ok.length) {
      for (const o of r.ok) {
        term.writeln(
          '\x1b[32m✓ ' + (act === 'approve' ? '已批准' : '已拒绝') + ': \x1b[0m' + o.path +
          (o.target && o.target !== o.path ? ' → ' + o.target : '') +
          (o.note ? '  \x1b[90m' + o.note + '\x1b[0m' : '')
        );
      }
      if (act === 'approve') term.writeln('\x1b[90m(INDEX 与知识图谱已重建)\x1b[0m');
    }
  }

  // /preview <待审路径>：行级预览批准后将写入的内容
  async function previewPending(path) {
    if (!path) {
      term.writeln('\x1b[33m用法：/preview <待审路径>（如 pending/notes/xxx.md）\x1b[0m');
      return;
    }
    const r = await api('/api/kb/pending/preview?path=' + encodeURIComponent(path));
    term.writeln(
      '\x1b[1;36m' + r.path + '\x1b[0m → 落地目标: \x1b[1m' + r.target + '\x1b[0m  (' +
      (r.kind === 'memory' ? '记忆追加' : '新笔记整篇') + ')'
    );
    term.writeln('\x1b[32m' + r.added.split('\n').map((l) => '+' + l).join('\n') + '\x1b[0m');
    term.writeln('\x1b[90m(/approve ' + r.path + ' 确认写入，/reject ' + r.path + ' 丢弃)\x1b[0m');
  }

  // ---------- /view 面板渲染层（iframe 沙箱 + postMessage 桥） ----------

  const viewOverlay = document.getElementById('view-overlay');
  const viewPanes = document.getElementById('view-panes');

  // 注入视图的桥脚本：window.hostApi(path, opts) → postMessage 给宿主 → 宿主调 /api/* 后回传；
  // 焦点在 sandbox iframe 内时 Esc 不冒泡出 frame，iframe 内监听并转发给宿主；
  // hostApi 带 20s 超时；视图脚本错误/未处理拒绝上报宿主（避免沙箱内静默崩溃）
  const BRIDGE = '<script>' +
    'window.hostApi=function(path,opts){return new Promise(function(res,rej){' +
    'var id=Math.random().toString(36).slice(2);' +
    'var timer=setTimeout(function(){window.removeEventListener("message",h);rej(new Error("桥请求超时: "+path));},20000);' +
    'function h(ev){if(ev.data&&ev.data.id===id){clearTimeout(timer);window.removeEventListener("message",h);ev.data.ok?res(ev.data.data):rej(new Error(ev.data.error||"host error"));}}' +
    'window.addEventListener("message",h);' +
    'window.parent.postMessage({type:"api",id:id,method:(opts&&opts.method)||"GET",path:path,body:opts&&opts.body},"*");' +
    '});};' +
    // 应用市场（阶段 1）：沙箱内直连 /api/* 的 fetch 自动改走桥（宿主按 app.json 权限白名单放行）
    // body 兼容：字符串 JSON 原样解析，对象直接透传（面板误传对象的旧写法也能工作）
    'var _nf=window.fetch;window.fetch=function(u,o){var s=String(u),i=s.indexOf("/api/");' +
    'if(i!==-1){var p=s.slice(i),bd;try{bd=o&&o.body?(typeof o.body==="string"?JSON.parse(o.body):o.body):undefined}catch(e){bd=undefined}' +
    'return window.hostApi(p,{method:(o&&o.method)||"GET",body:bd}).then(function(d){return{ok:true,status:200,json:function(){return Promise.resolve(d)},text:function(){return Promise.resolve(JSON.stringify(d))}}});}' +
    'return _nf.apply(this,arguments);};' +
    'window.addEventListener("keydown",function(e){if(e.key==="Escape"){window.parent.postMessage({type:"escape"},"*");}});' +
    // 主题同步：宿主切换主题时广播，iframe 跟随（初始值由 openView 注入 data-theme）
    'window.addEventListener("message",function(ev){if(ev.data&&ev.data.type==="theme"){document.documentElement.dataset.theme=ev.data.theme;}});' +
    'window.addEventListener("error",function(e){window.parent.postMessage({type:"view-error",msg:(e&&e.message)||"视图脚本错误"},"*");});' +
    'window.addEventListener("unhandledrejection",function(e){var r=e&&e.reason;window.parent.postMessage({type:"view-error",msg:(r&&r.message)||String(r)},"*");});' +
    // App 状态持久化（方案 A）：沙箱无 allow-same-origin → localStorage 不可用。
    // 注入内存+持久化代理：同步读写内存，防抖 500ms 经 /api/apps/<id>/data 落盘 kb/apps/<id>/data/localstorage.json。
    // 变量名 __appId（不被 replace 匹配）、值占位符 __APP_ID__（app 视图由 openView 全局替换为真实 id）；
    // 内置面板保留占位符字面量 → 仅内存、不落盘。
    'window.__appId="__APP_ID__";' +
    '(function(){var __ls={};' +
    'if(window.__appId&&window.__appId.indexOf("__APP")!==0){' +
    'window.hostApi("/api/apps/"+window.__appId+"/data").then(function(d){' +
    'if(d&&d.data&&typeof d.data==="object"){for(var k in d.data){if(!(k in __ls))__ls[k]=d.data[k];}}' + // 启动竞态兜底：本地已设键不被旧数据覆盖
    '}).catch(function(){});' +
    '}' +
    'var __lsTimer=null;function __lsFlush(){clearTimeout(__lsTimer);__lsTimer=setTimeout(function(){' +
    'if(!window.__appId||window.__appId.indexOf("__APP")===0)return;' +
    'window.hostApi("/api/apps/"+window.__appId+"/data",{method:"POST",body:{data:__ls}}).catch(function(){});' +
    '},500);}' +
    'var __lsProxy={' +
    'getItem:function(k){return Object.prototype.hasOwnProperty.call(__ls,k)?__ls[k]:null;},' +
    'setItem:function(k,v){__ls[k]=String(v);__lsFlush();},' +
    'removeItem:function(k){delete __ls[k];__lsFlush();},' +
    'clear:function(){__ls={};__lsFlush();},' +
    'key:function(i){var ks=Object.keys(__ls);return i>=0&&i<ks.length?ks[i]:null;},' +
    'get length(){return Object.keys(__ls).length;}' +
    '};' +
    'try{Object.defineProperty(window,"localStorage",{configurable:true,value:__lsProxy});}catch(e){}' +
    '})();' +
    '<\/script>';

  // 视图（单视图：同一时刻只开一个详情页，新开替换旧的）
  let currentView = null; // {title, iframe, loaded, busy, err, appId, perms, specKind, specArg}

  function activateView() {
    updateMenuActive(currentView && currentView.specKind === 'builtin' ? { arg: currentView.specArg } : null);
    viewOverlay.classList.remove('hidden');
    if (currentView) document.getElementById('view-title').textContent = currentView.title;
    // 焦点移出 xterm textarea：聚焦时 xterm 拦截 Esc（stopPropagation），父页监听收不到
    if (document.activeElement && document.activeElement.blur) document.activeElement.blur();
  }

  function closeView() {
    if (currentView) { currentView.iframe.remove(); currentView = null; }
    viewOverlay.classList.add('hidden');
    updateMenuActive(null);
    applySplit(false); // 视图关闭：退出分屏（终端恢复全宽）+ 焦点归还终端
    renderSidebarApps(true); // 面板关闭后刷新工作台子菜单（装/卸应用立即反映）
    term.focus();
  }

  function openView(title, html, app, spec) {
    // 单视图：同 (kind, arg) 已开则仅更新标题激活，不重复加载（保留面板内状态）；否则替换旧视图
    if (currentView && spec && currentView.specKind === spec.kind && currentView.specArg === spec.arg) {
      currentView.title = title;
      activateView();
      return;
    }
    if (currentView) currentView.iframe.remove();
    // 应用市场（阶段 1）：app 视图注入 <base href="/apps/<id>/">（相对 vendor/ 等资源正确解析）
    if (app) {
      const baseHref = '<base href="/apps/' + app.id + '/">';
      if (/<base[^>]*>/i.test(html)) html = html.replace(/<base[^>]*>/i, baseHref);
      else if (/<head[^>]*>/i.test(html)) html = html.replace(/<head[^>]*>/i, (m) => m + '\n  ' + baseHref);
      else html = baseHref + '\n' + html;
    }
    const iframe = document.createElement('iframe');
    // allow-modals：面板内 confirm/alert 生效（市场卸载/看板删除的人审确认）；仍无 allow-same-origin，桥层权限白名单不变
    iframe.sandbox = 'allow-scripts allow-modals';
    const view = { title, iframe, loaded: false, busy: false, err: null, appId: app ? app.id : null, perms: app ? app.permissions : null, specKind: spec ? spec.kind : null, specArg: spec ? spec.arg : null };
    iframe.addEventListener('load', () => { view.loaded = true; });
    currentView = view;
    viewPanes.appendChild(iframe);
    // 面板主题：注入 theme.css 变量定义（iframe srcdoc 无法引用外部相对路径）+ 初始 data-theme（宿主当前主题）
    iframe.srcdoc = '<style>' + themeCss + '</style>' +
      '<script>document.documentElement.dataset.theme="' + currentTheme + '"<\/script>' +
      (app ? BRIDGE.replace(/__APP_ID__/g, app.id) : BRIDGE) + html;
    activateView();
  }

  // /view 分屏参照：分屏模式终端左 40%（FitAddon 重算列数）+ 视图右 60%，对话流可见可参照
  // 选择实时记忆；关闭全部视图时自动退出分屏（终端恢复全宽）
  const viewSplitBtn = document.getElementById('view-split');
  const SPLIT_KEY = 'md-agent-view-split';
  function refitTerm() {
    // DOM 流自适应（stream.js 已按容器宽度计算列宽），无需重算
  }
  function applySplit(on) {
    document.body.classList.toggle('view-split', on);
    try { localStorage.setItem(SPLIT_KEY, on ? '1' : '0'); } catch (e) { /* 忽略 */ }
    updateSplitBtn();
    setTimeout(refitTerm, 50); // overlay 布局变化后重算终端列数
  }
  function updateSplitBtn() {
    if (viewSplitBtn) viewSplitBtn.textContent = document.body.classList.contains('view-split') ? '全屏' : '分屏';
  }
  if (viewSplitBtn) {
    viewSplitBtn.addEventListener('click', () => applySplit(!document.body.classList.contains('view-split')));
    try { if (localStorage.getItem(SPLIT_KEY) === '1') applySplit(true); } catch (e) { /* 忽略 */ }
    updateSplitBtn();
  }

  document.getElementById('view-close').addEventListener('click', () => closeView());
  window.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape' && !viewOverlay.classList.contains('hidden')) closeView();
  });

  // postMessage 桥：当前视图 iframe → 宿主 API（只允许 /api/ 前缀）；escape 关视图；view-error 写终端
  window.addEventListener('message', async (ev) => {
    if (!currentView || currentView.iframe.contentWindow !== ev.source) return;
    const tab = currentView;
    const msg = ev.data;
    if (!msg) return;
    if (msg.type === 'escape') { closeView(); return; }
    if (msg.type === 'cmd') {
      // 面板 → 宿主命令（应用市场「运行」/ 功能首页命令卡片等）；仅信任的内置面板（非 app 视图）可发。
      // 走 panelCmd 而非裸 run()：与键盘提交同输入条生命周期，否则 atPrompt 悬空会致状态行乱入输出
      if (!tab.appId && msg.cmd) panelCmd(msg.cmd);
      return;
    }
    if (msg.type === 'project') {
      // 面板 → 宿主：切换项目（首页「项目空间」卡片；仅内置面板）
      if (!tab.appId) switchProject(msg.id || null, msg.name || '个人空间');
      return;
    }
    if (msg.type === 'prefill') {
      // 面板 → 宿主：命令参数弹窗（界面操作：不再预填命令到输入框，点卡片 → 弹参 → 直接执行）。
      // 内置面板（!tab.appId）可信；busy 时忽略（命令执行中不接受）
      if (!tab.appId && !busy) openCmdPrompt(msg.cmd || '', null);
      return;
    }
    if (msg.type === 'flag') {
      // 面板 → 宿主：读写 md-agent-* 标志位（沙箱内 localStorage 是内存代理不落盘，标志位宿主持有）
      // 仅内置面板可发；键白名单 md-agent- 前缀（防面板越权读写其他存储）
      if (tab.appId) return;
      if (msg.get && String(msg.get).startsWith('md-agent-')) {
        let v = null;
        try { v = localStorage.getItem(msg.get); } catch (e) { /* 忽略 */ }
        tab.iframe.contentWindow.postMessage({ id: msg.id, get: msg.get, value: v }, '*');
      } else if (msg.key && String(msg.key).startsWith('md-agent-') && msg.set !== undefined) {
        try { if (msg.set === null) localStorage.removeItem(msg.key); else localStorage.setItem(msg.key, msg.set); } catch (e) { /* 忽略 */ }
      }
      return;
    }
    if (msg.type === 'view-error') {
      // 视图脚本错误/未处理拒绝：写终端（让沙箱内崩溃可见）
      if (!tab.err) {
        tab.err = msg.msg;
        term.writeln('\x1b[31m视图 [' + tab.title + '] 脚本错误: ' + (msg.msg || '') + '\x1b[0m');
      }
      return;
    }
    // App → Agent 通道（agent:ask）：App 提问走 Agent 回路（项目知识库+工具+记忆），
    // 终端显示可审计（不进会话历史），流式/工具事件推回 App；需 manifest 声明「agent」权限
    if (msg.type === 'agent:ask') {
      if (!tab.appId || !(tab.perms || []).includes('agent')) {
        tab.iframe.contentWindow.postMessage({ type: 'agent:error', message: '权限不足：应用未声明「agent」权限' }, '*');
        return;
      }
      if (busy) {
        tab.iframe.contentWindow.postMessage({ type: 'agent:error', message: 'Agent 正忙，请稍后再试' }, '*');
        return;
      }
      const text = String(msg.text || '').slice(0, 2000);
      if (!text) { tab.iframe.contentWindow.postMessage({ type: 'agent:error', message: '空提问' }, '*'); return; }
      // 应用空间（Phase A）：space 参数 → 注入应用 notes/ 摘要（应用私有知识；agent 可用 file 工具读详情）
      let spaceHint = '';
      if (msg.space && tab.appId) {
        try {
          const sn = await api('/api/apps/' + tab.appId + '/notes');
          const items = sn.notes || [];
          if (items.length) {
            spaceHint = '\n\n[应用知识空间 apps/' + tab.appId + '/notes/]（应用私有知识，需要时用 file 工具读取详情）\n' +
              items.map((x) => '- ' + x.file + '：' + (x.snippet || '')).join('\n').slice(0, 4000);
          }
        } catch (e) { /* 无 notes 或失败静默 */ }
      }
      busy = true;
      setBusyUI();
      try {
        await runAsApp(text, tab, msg.context, spaceHint);
      } catch (e) {
        tab.iframe.contentWindow.postMessage({ type: 'agent:error', message: (e && e.message) || String(e) }, '*');
      } finally {
        busy = false;
        setBusyUI();
        refreshStatus();
      }
      return;
    }
    if (msg.type !== 'api') return;
    tab.busy = true;               // 收到桥请求 → 视图活跃
    if (!msg.path || !msg.path.startsWith('/api/')) {
      tab.iframe.contentWindow.postMessage({ id: msg.id, ok: false, error: '仅允许 /api/ 接口' }, '*');
      return;
    }
    // 应用市场（阶段 1）权限白名单：app 视图只放行 app.json 声明的权限
    if (tab.appId && tab.perms) {
      // App 状态持久化（storage）：data 路径的 id 必须等于当前 tab 的 appId（防越权读写其他应用数据）
      const dataMatch = /^\/api\/apps\/([^/]+)\/data/.exec(msg.path || '');
      if (dataMatch && dataMatch[1] !== tab.appId) {
        tab.iframe.contentWindow.postMessage({ id: msg.id, ok: false, error: 'appId 不匹配，禁止访问其他应用数据' }, '*');
        return;
      }
      const perm = Core.permForPath(msg.path);
      if (!Core.appCan(msg.path, msg.method || 'GET', tab.perms)) {
        tab.iframe.contentWindow.postMessage({ id: msg.id, ok: false, error: '权限不足：应用未声明「' + (perm || '该') + '」权限' }, '*');
        return;
      }
      // 应用空间（Phase A）：应用写文件限定自己目录（apps/<appId>/ 私有空间）或待审队列（pending/ 人审提案），防越权写主库/其他应用
      if (msg.method === 'POST' && /^\/api\/file/.test(msg.path || '')) {
        let wp = '';
        try {
          const bd = typeof msg.body === 'string' ? JSON.parse(msg.body) : (msg.body || {});
          wp = String(bd.path || '');
        } catch (e) { wp = ''; }
        if (!(wp.startsWith('apps/' + tab.appId + '/') || wp.startsWith('pending/'))) {
          tab.iframe.contentWindow.postMessage({ id: msg.id, ok: false, error: '应用只能写入自己的空间（apps/<id>/）或待审队列（pending/）' }, '*');
          return;
        }
      }
    }
    try {
      const res = await fetch(msg.path, {
        method: msg.method || 'GET',
        // 项目制：面板请求跟随宿主当前项目（隔离根由后端 X-Project 解析）
        headers: { 'Content-Type': 'application/json', 'X-Project': currentProject || '' },
        body: msg.body ? JSON.stringify(msg.body) : undefined,
      });
      const data = await res.json().catch(() => ({}));
      // 非 2xx 时透传后端错误消息（面板不再只见笼统的 "host error"）
      tab.iframe.contentWindow.postMessage({
        id: msg.id, ok: res.ok, data,
        error: res.ok ? undefined : ((data && data.error) || 'HTTP ' + res.status),
      }, '*');
      // 面板写操作成功 → 主界面状态即时刷新（状态栏待审/任务数字不等 8s 轮询；面板内批准/改任务/补链/卸载后立即反映）
      if (res.ok && (msg.method || 'GET') !== 'GET') {
        refreshStatus();
        if (msg.path && msg.path.indexOf('/api/market/') === 0) appsCache = null; // 市场变更 → /view <id> 查表失效
      }
    } catch (e) {
      tab.iframe.contentWindow.postMessage({ id: msg.id, ok: false, error: String(e) }, '*');
    }
  });

  // /view graph      内置知识图谱可视化
  // /view <路径>      kb 内本地 HTML 视图
  // /view off        关闭（或 Esc）
  async function viewCmd(arg) {
    if (!arg || arg === 'off') {
      closeView();
      return;
    }
    // 收敛面板兼容：/view ops、/view pending、/view audit 统一映射到「自动化」面板（旧引用不失效）
    if (arg === 'ops' || arg === 'pending' || arg === 'audit') arg = 'automation';
    if (arg === 'graph' || arg === 'board' || arg === 'automation' || arg === 'market' || arg === 'home' || arg === 'sessions' || arg === 'config' || arg === 'onboarding') {
      const path = arg === 'config' ? '/config.html' : arg === 'onboarding' ? '/onboarding.html' : '/views/' + arg + '.html';
      const r = await fetch(path);
      if (!r.ok) throw new Error('内置视图加载失败: HTTP ' + r.status);
      const titles = { graph: '知识图谱', board: '任务看板', automation: '自动化（控制 / 审核 / 运营数据）', market: '工作台', home: '功能首页', sessions: '历史会话', config: '设置', onboarding: '开始使用' };
      openView(titles[arg], await r.text(), null, { kind: 'builtin', arg });
      return;
    }
    // 应用市场（阶段 1）：/view <app-id> 打开已安装应用（manifest 权限白名单）
    const apps = await getApps();
    const app = apps.find((a) => a.id === arg);
    if (app) {
      // App 是全局资源（kb/apps/），entry 读取不受当前项目影响 → 显式空 X-Project 回退主根
      const r = await api('/api/file?path=apps/' + app.id + '/' + encodeURIComponent(app.entry), { headers: { 'X-Project': '' } });
      openView(app.name, r.content, { id: app.id, permissions: app.permissions }, { kind: 'app', arg: app.id });
      return;
    }
    const r = await api('/api/file?path=' + encodeURIComponent(arg));
    openView(r.path, r.content, null, { kind: 'file', arg });
  }

  // 应用市场（阶段 3）：URL ?view=<id> 自动打开面板/应用（托盘「应用市场/已安装应用」入口用）
  // 详情页布局下启动不默认打开任何面板（避免覆盖聊天区），?view= 参数指定时直开
  (async function autoView() {
    const v = new URLSearchParams(location.search).get('view');
    if (v) { try { await viewCmd(v); } catch (e) { term.writeln('\x1b[31m自动打开失败: ' + e.message + '\x1b[0m'); } }
  })();
  renderSidebarApps(); // 启动渲染工作台子菜单（已安装应用前 3 个）

  // ---------- 应用市场（阶段 2）：/market list | import <路径> | uninstall <id> | update <id> <路径> ----------
  // ---------- SkillHub（阶段 4）：connect <url> | hubs | disconnect <name> | refresh <name> | catalog | install <id> ----------
  async function marketCmd(args) {
    const sub = (args && args[0]) || 'list';
    if (sub === 'connect') {
      const url = (args[1] || '').trim();
      if (!url) { term.writeln('\x1b[33m用法：/market connect <hub-url>（默认 skillhub.cn 商店：https://skillhub.cn/install/skillhub.md）\n   git 仓库：git+https://github.com/user/skills-repo\n   GitHub zip：https://github.com/user/skills-repo/archive/refs/heads/main.zip\n   本地目录：local:C:/path/to/skills\n（skillhub.cn=API 检索商店；其他源=自动分析 md 文档生成目录；旧 skillhub.md 索引兼容）\x1b[0m'); return; }
      const r = await api('/api/hubs/connect', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ url }),
      }).catch((e) => { term.writeln('\x1b[31m连接失败: ' + e.message + '\x1b[0m'); return null; });
      if (!r) return;
      const h = r.hub;
      const kinds = {}; (h.apps || []).forEach((a) => { kinds[a.kind] = (kinds[a.kind] || 0) + 1; });
      const kt = Object.keys(kinds).map((k) => kinds[k] + ' 个' + (k === 'app' ? '应用' : '技能')).join('、') || '0 个条目';
      term.writeln('\x1b[32m✓ 已连接 SkillHub: \x1b[0m' + h.name + '（分析出 ' + kt + '）');
      for (const a of h.apps) term.writeln('  \x1b[36m' + a.id + '\x1b[0m v' + a.version + (a.kind === 'skill' ? ' \x1b[90m[技能]\x1b[0m' : '') + '  ' + a.name + ' \x1b[90m' + a.description + '\x1b[0m');
      term.writeln('安装：/market install <id>（人审确认）· /view market 查看目录');
      return;
    }
    if (sub === 'hubs') {
      const a = await api('/api/hubs').catch(() => null);
      const hubs = (a && a.hubs) || [];
      term.writeln('已连接 SkillHub：');
      for (const h of hubs) {
        const nApp = h.apps.filter((x) => x.kind === 'app').length;
        const nSkill = h.apps.length - nApp;
        term.writeln('  \x1b[36m' + h.name + '\x1b[0m v' + h.version + ' · ' + nApp + ' 应用 + ' + nSkill + ' 技能 · ' + h.url);
      }
      if (!hubs.length) term.writeln('  (无 —— /market connect <hub-url> 连接)');
      return;
    }
    if (sub === 'disconnect') {
      const name = (args[1] || '').trim();
      if (!name) { term.writeln('\x1b[33m用法：/market disconnect <hub名>\x1b[0m'); return; }
      // 断开可逆（随时可重连），免确认直接断开
      const r = await api('/api/hubs/disconnect', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name }),
      }).catch((e) => { term.writeln('\x1b[31m断开失败: ' + e.message + '\x1b[0m'); return null; });
      if (r) term.writeln('\x1b[32m✓ 已断开: \x1b[0m' + name);
      return;
    }
    if (sub === 'refresh') {
      const name = (args[1] || '').trim();
      if (!name) { term.writeln('\x1b[33m用法：/market refresh <hub名>\x1b[0m'); return; }
      const r = await api('/api/hubs/refresh', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name }),
      }).catch((e) => { term.writeln('\x1b[31m刷新失败（旧索引已保留）: ' + e.message + '\x1b[0m'); return null; });
      if (r) term.writeln('\x1b[32m✓ 已刷新: \x1b[0m' + r.hub.name + '（' + r.hub.apps.length + ' 个条目）');
      return;
    }
    if (sub === 'catalog') {
      const a = await api('/api/market/catalog').catch(() => null);
      const apps = (a && a.apps) || [];
      term.writeln('已连接 hub 目录（/market install <id> 安装）：');
      for (const app of apps) term.writeln('  \x1b[36m' + app.id + '\x1b[0m v' + app.version + (app.kind === 'skill' ? ' \x1b[90m[技能]\x1b[0m' : '') + '  ' + app.name + ' \x1b[90m[' + app.hub + ']\x1b[0m');
      if (!apps.length) term.writeln('  (无 —— /market connect <hub-url> 连接第三方 SkillHub)');
      return;
    }
    if (sub === 'search') {
      const q = (args[1] || '').trim();
      if (!q) { term.writeln('\x1b[33m用法：/market search <关键词>（检索 skillhub.cn 商店技能）\x1b[0m'); return; }
      const r = await api('/api/hubs/search', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ q }),
      }).catch((e) => { term.writeln('\x1b[31m检索失败: ' + e.message + '\x1b[0m'); return null; });
      if (!r) return;
      const apps = (r.apps || []);
      term.writeln('「' + q + '」搜索结果（/market install <id> 安装）：');
      for (const app of apps) term.writeln('  \x1b[36m' + app.id + '\x1b[0m v' + app.version + '  ' + app.name + ' \x1b[90m' + app.description + '\x1b[0m');
      if (!apps.length) term.writeln('  (无匹配 —— 换个关键词试试)');
      return;
    }
    if (sub === 'install') {
      const id = (args[1] || '').trim();
      if (!id) { term.writeln('\x1b[33m用法：/market install <id>（从已连接 hub 目录安装，人审确认）\x1b[0m'); return; }
      const cat = await api('/api/market/catalog').catch(() => null);
      const entry = ((cat && cat.apps) || []).find((a) => a.id === id);
      if (!entry) { term.writeln('\x1b[31m目录中找不到: \x1b[0m' + id + '（先 /market connect <hub-url>，/market catalog 看清单）'); return; }
      // 1) dry_run（集合缓存直装 / source 下载校验并展示 manifest）→ 2) 人审确认 → 3) 落盘
      const installBody = entry.source ? { source: entry.source } : { hub: entry.hub, id: entry.id };
      const probe = await api('/api/market/install', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(Object.assign({}, installBody, { dry_run: true })),
      }).catch((e) => { term.writeln('\x1b[31m下载校验失败: ' + e.message + '\x1b[0m'); return null; });
      if (!probe) return;
      const m = probe.app;
      term.writeln('应用: \x1b[36m' + m.name + '\x1b[0m v' + m.version + ' (id: ' + m.id + ') · 来源 ' + entry.hub);
      term.writeln('权限: [' + (m.permissions.join(', ') || '无') + ']');
      if (m.description) term.writeln('描述: ' + m.description);
      const ok = await confirm('确认安装该应用？');
      if (!ok) { term.writeln('已取消'); return; }
      const r = await api('/api/market/install', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(installBody),
      }).catch((e) => { term.writeln('\x1b[31m安装失败: ' + e.message + '\x1b[0m'); return null; });
      if (r) {
        if (r.kind === 'skill') {
          term.writeln('\x1b[32m✓ 已安装技能: \x1b[0m' + r.id + '（trigger 命中自动注入；/skills 查看）');
        } else {
          term.writeln('\x1b[32m✓ 已安装: \x1b[0m' + r.app.id + '（/view ' + r.app.id + ' 打开）');
        }
        appsCache = null; refreshStatus();
      }
      return;
    }
    if (sub === 'list') {
      const a = await api('/api/apps').catch(() => null);
      term.writeln('已安装应用（/view <id> 打开）：');
      for (const app of (a && a.apps) || []) {
        term.writeln('  \x1b[36m' + app.id + '\x1b[0m v' + app.version + ' · 权限 [' + (app.permissions.join(', ') || '无') + ']  ' + app.name);
      }
      if (!a || !a.apps || !a.apps.length) term.writeln('  (无 —— /market import <本地应用目录路径> 安装)');
      return;
    }
    if (sub === 'import') {
      const path = args.slice(1).join(' ');
      if (!path) { term.writeln('\x1b[33m用法：/market import <本地应用目录路径>\x1b[0m'); return; }
      // 1) dry_run 校验并展示 manifest → 2) 人审确认 → 3) 落盘
      const probe = await api('/api/market/install', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ source: 'local', path, dry_run: true }),
      }).catch((e) => { term.writeln('\x1b[31m导入校验失败: ' + e.message + '\x1b[0m'); return null; });
      if (!probe) return;
      const m = probe.app;
      term.writeln('应用: \x1b[36m' + m.name + '\x1b[0m v' + m.version + ' (id: ' + m.id + ')');
      term.writeln('权限: [' + (m.permissions.join(', ') || '无') + ']');
      if (m.description) term.writeln('描述: ' + m.description);
      const ok = await confirm('确认安装该应用？');
      if (!ok) { term.writeln('已取消'); return; }
      const r = await api('/api/market/install', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ source: 'local', path }),
      }).catch((e) => { term.writeln('\x1b[31m安装失败: ' + e.message + '\x1b[0m'); return null; });
      if (r) {
        if (r.kind === 'skill') {
          term.writeln('\x1b[32m✓ 已安装技能: \x1b[0m' + r.id + '（trigger 命中自动注入；/skills 查看）');
        } else {
          term.writeln('\x1b[32m✓ 已安装: \x1b[0m' + r.app.id + '（/view ' + r.app.id + ' 打开）');
        }
        appsCache = null; refreshStatus();
      }
      return;
    }
    if (sub === 'uninstall') {
      const id = (args[1] || '').trim();
      if (!id) { term.writeln('\x1b[33m用法：/market uninstall <id>\x1b[0m'); return; }
      const ok = await confirm('确认卸载 ' + id + '？');
      if (!ok) { term.writeln('已取消'); return; }
      const r = await api('/api/market/uninstall', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id }),
      }).catch((e) => { term.writeln('\x1b[31m卸载失败: ' + e.message + '\x1b[0m'); return null; });
      if (r) { term.writeln('\x1b[32m✓ 已卸载: \x1b[0m' + id); appsCache = null; refreshStatus(); }
      return;
    }
    if (sub === 'update') {
      const id = (args[1] || '').trim();
      const path = args.slice(2).join(' ');
      if (!id || !path) { term.writeln('\x1b[33m用法：/market update <id> <本地新版本目录路径>\x1b[0m'); return; }
      const ok = await confirm('确认更新 ' + id + '？');
      if (!ok) { term.writeln('已取消'); return; }
      const r = await api('/api/market/update', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id, path }),
      }).catch((e) => { term.writeln('\x1b[31m更新失败: ' + e.message + '\x1b[0m'); return null; });
      if (r) { term.writeln('\x1b[32m✓ 已更新: \x1b[0m' + r.app.id + ' v' + r.app.version); appsCache = null; refreshStatus(); }
      return;
    }
    term.writeln('/market 子命令：\n  list                    已安装应用\n  connect <url>          连接第三方 SkillHub（如 skillhub.cn/install/skillhub.md）\n  hubs                    已连接 hub 列表\n  catalog                 已连接 hub 目录\n  install <id>            从目录安装（人审确认）\n  refresh <name>          刷新 hub 索引\n  disconnect <name>       断开 hub（已装应用不受影响）\n  import <路径>           从本地目录安装（人审确认，手动导入兜底）\n  uninstall <id>          卸载\n  update <id> <路径>       更新（本地新版本目录）');
  }

  // ---------- 命令面板 + 状态中心（Ctrl+K | /side | 速览按钮唤出；上部模糊搜索直达视图/命令/@文档，下部实时速览卡） ----------

  const sideDrawer = document.getElementById('side-drawer');
  const sideBody = document.getElementById('side-body');
  const sideSearch = document.getElementById('side-search');
  const sideResults = document.getElementById('side-results');

  // 命令面板候选：内置视图 + / 命令 + @KB 文档 + 已装应用（均以「可执行的命令串」为 run）
  const VIEW_TARGETS = [
    { k: '首页', d: '功能总览（全部功能一键进入）', run: '/view home' },
    { k: '图谱', d: '知识图谱（类型化/局部图/关系探索）', run: '/view graph' },
    { k: '自动化', d: '自动化控制 + 审核 + 运营数据', run: '/view automation' },
    { k: '看板', d: '任务看板', run: '/view board' },
    { k: '工作台', d: '我的应用 + 常用功能 + 应用市场', run: '/view market' },
  ];
  let sideTimer = null;   // 状态中心 8s 轮询（与状态行同源数据）
  let sideSel = -1;       // 候选选中下标
  let sideItems = [];     // 当前候选 [{k, d, run}]

  function escapeHtml(s) {
    return String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  }

  async function toggleSide() {
    if (sideDrawer.classList.contains('hidden')) {
      sideDrawer.classList.remove('hidden');
      loadSide();
      sideSearch.value = '';
      renderSideItems(await filterSide(''));
      clearInterval(sideTimer);
      sideTimer = setInterval(loadSide, 8000); // 打开期间状态中心自动刷新（与状态行同源）
      sideSearch.focus();
    } else closeSide();
  }
  function closeSide() {
    sideDrawer.classList.add('hidden');
    clearInterval(sideTimer);
    sideTimer = null;
  }

  // ---- 命令面板：候选过滤（视图/命令同步匹配；@ 前缀进文档模式；应用异步加载） ----
  async function filterSide(q) {
    const ql = (q || '').trim().toLowerCase();
    const items = [];
    if (!ql) {
      // 空查询：仅视图目标（简洁首页）；已装应用输入关键词才匹配，避免应用多时列表变长
      for (const v of VIEW_TARGETS) items.push({ k: v.k, d: v.d, run: v.run });
      return items;
    }
    if (ql.startsWith('@')) {
      const kw = ql.slice(1);
      for (const p of await loadAtDocs()) {
        if (p.toLowerCase().includes(kw)) items.push({ k: '@' + p, d: 'KB 文档（open 全文）', run: 'open ' + p });
      }
      return items;
    }
    for (const v of VIEW_TARGETS) {
      if (v.k.toLowerCase().includes(ql) || v.d.toLowerCase().includes(ql) || v.run.includes(ql)) items.push({ k: v.k, d: v.d, run: v.run });
    }
    for (const [c, d] of COMMANDS) {
      if (c.toLowerCase().includes(ql) || d.toLowerCase().includes(ql)) items.push({ k: c, d, run: c });
    }
    try {
      for (const a of await getApps()) {
        if (a.name.toLowerCase().includes(ql) || a.id.toLowerCase().includes(ql)) items.push({ k: a.name, d: a.id + ' v' + a.version + '（应用）', run: '/view ' + a.id });
      }
    } catch (e) { /* 忽略 */ }
    return items;
  }
  function renderSideItems(items) {
    sideItems = items;
    sideSel = items.length ? 0 : -1;
    if (!items.length) { sideResults.innerHTML = '<div class="side-result" style="color:#7f849c">(无匹配)</div>'; return; }
    sideResults.innerHTML = items.map((it, i) =>
      '<div class="side-result' + (i === sideSel ? ' sel' : '') + '" data-i="' + i + '">' +
      '<span class="k">' + escapeHtml(it.k) + '</span><span class="d">' + escapeHtml(it.d || '') + '</span></div>'
    ).join('');
  }

  // 搜索输入防抖过滤；↑↓ 选择、Enter 执行、Esc 关闭（输入框内键盘不经过 xterm）
  let sideSearchTimer = null;
  sideSearch.addEventListener('input', () => {
    clearTimeout(sideSearchTimer);
    sideSearchTimer = setTimeout(async () => { renderSideItems(await filterSide(sideSearch.value)); }, 80);
  });
  sideSearch.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.preventDefault(); closeSide(); term.focus(); return; }
    if (e.key === 'ArrowDown' && sideItems.length) { e.preventDefault(); sideSel = (sideSel + 1) % sideItems.length; renderSideItems(sideItems); return; }
    if (e.key === 'ArrowUp' && sideItems.length) { e.preventDefault(); sideSel = (sideSel - 1 + sideItems.length) % sideItems.length; renderSideItems(sideItems); return; }
    if (e.key === 'Enter') {
      e.preventDefault();
      const it = sideItems[sideSel >= 0 ? sideSel : 0];
      if (it) runSideItem(it);
    }
  });
  function runSideItem(it) {
    closeSide();
    submitCmd(it.run);
  }
  sideResults.addEventListener('click', (e) => {
    const el = e.target.closest('.side-result');
    if (!el || !el.dataset.i) return;
    const it = sideItems[parseInt(el.dataset.i, 10)];
    if (it) runSideItem(it);
  });

  function sideSec(title, cmd, body, hint) {
    return '<div class="side-sec" data-cmd="' + cmd + '">' +
      '<div class="side-title">' + title + '</div>' +
      '<div class="side-body">' + body + '</div>' +
      (hint ? '<div class="side-hint">' + hint + '</div>' : '') + '</div>';
  }

  async function loadSide() {
    sideBody.innerHTML = '<div class="side-sec"><div class="side-body" style="color:#7f849c">加载中…</div></div>';
    const [t, p, g, o, a] = await Promise.all([
      api('/api/tasks').catch(() => null),
      api('/api/kb/pending').catch(() => null),
      api('/api/graph/stats').catch(() => null),
      api('/api/graph/orphans').catch(() => null),
      api('/api/audit').catch(() => null),
    ]);
    const secs = [];
    if (t && t.stats) {
      const s = t.stats;
      secs.push(sideSec('任务', '/view board',
        (s.todo || 0) + ' 待办 · ' + (s.doing || 0) + ' 进行中 · ' + (s.done || 0) + ' 完成' + (s.dropped ? ' · ' + s.dropped + ' 放弃' : ''),
        '点击打开看板'));
    }
    if (p) {
      const n = (p.pending || []).length;
      secs.push(sideSec('审核', '/view automation',
        n + ' 篇待审' + (n ? '（' + (p.pending[0].kind === 'memory' ? '记忆' : '笔记') + (n > 1 ? ' 等' : '') + '）' : ''),
        n ? '点击图形审核' : ''));
    }
    if (g) {
      secs.push(sideSec('图谱', '/view graph',
        (g.docs || 0) + ' 文档 · ' + (g.links || 0) + ' 链接' + (g.dangling ? ' · 悬空 ' + g.dangling : '') +
        (o && o.orphans ? ' · 孤立 ' + o.orphans.length : ''),
        '点击结构导航'));
    }
    if (a) {
      const w = (a.orphans ? a.orphans.length : 0) + (a.dangling ? a.dangling.length : 0) + (a.duplicates ? a.duplicates.length : 0) + (a.mentions ? a.mentions.length : 0);
      secs.push(sideSec('审计', '/view automation',
        w ? '⚠ 孤立 ' + (a.orphans || []).length + ' / 悬空 ' + (a.dangling || []).length + ' / 重复 ' + (a.duplicates || []).length + ' / 建议 ' + (a.mentions || []).length
          : '✓ 知识库健康',
        w ? '点击健康审计（自动化面板内）' : ''));
    }
    sideBody.innerHTML = secs.join('');
  }

  // 点击卡片 → 关抽屉 + 执行对应命令（统一提交入口）
  sideBody.addEventListener('click', (e) => {
    const sec = e.target.closest('.side-sec');
    if (!sec) return;
    closeSide();
    submitCmd(sec.dataset.cmd);
  });
  document.getElementById('side-refresh').addEventListener('click', (e) => { e.stopPropagation(); loadSide(); });
  // 点抽屉外关闭（侧边栏菜单区域除外，避免与菜单唤出竞态）
  document.addEventListener('click', (e) => {
    if (sideDrawer.classList.contains('hidden')) return;
    if (sideDrawer.contains(e.target) || (e.target.closest && e.target.closest('#sb-menu'))) return;
    closeSide();
  });

  // ---------- 巩固器 + 技能（Phase 3-C Step 2） ----------

  // /consolidate：运行巩固器，生成巩固提案进待审（MEMORY 去重 / 重复标题提示）
  async function consolidateCmd(opt) {
    const llm = opt === 'llm' || opt === '--llm';
    term.writeln('\x1b[90m(巩固器' + (llm ? ' v2 LLM' : ' v1 规则') + ': ' + (llm ? '重复标题 LLM 整合' : 'MEMORY 去重 + 重复标题检测') + ')\x1b[0m');
    const r = await api('/api/consolidate' + (llm ? '?llm=1' : ''), { method: 'POST' });
    if (r.created && r.created.length) {
      term.writeln('\x1b[32m✓ 生成 ' + r.created.length + ' 条巩固提案:\x1b[0m');
      for (const c of r.created) {
        term.writeln('  ' + c + '  \x1b[90m(/preview 预览 · /approve 确认 · /reject 丢弃)\x1b[0m');
      }
    } else {
      term.writeln('\x1b[90m无巩固提案（MEMORY 无重复行、无重复标题文档）\x1b[0m');
    }
  }

  // /skills：列出技能注册表（技能 = 程序性记忆，trigger 命中自动注入）
  async function skillsCmd() {
    const r = await api('/api/skills');
    const sk = r.skills || [];
    if (!sk.length) {
      term.writeln('\x1b[90m技能库为空（Agent 生成的技能提案经 /approve 后安装到 kb/skills/）\x1b[0m');
      return;
    }
    term.writeln('技能注册表（' + sk.length + ' 项）:');
    for (const s of sk) {
      term.writeln(
        '  \x1b[35m' + s.title + '\x1b[0m' +
        (s.trigger ? '  \x1b[90mtrigger: ' + s.trigger + '\x1b[0m' : '') +
        '  \x1b[90m' + (s.desc || '') + '\x1b[0m'
      );
    }
    term.writeln('\x1b[90m(提问命中 trigger 时技能正文自动注入)\x1b[0m');
  }

  // ---------- 记忆自组织（Phase 3-A：审计 / 补链接 / 补文档） ----------

  async function auditCmd() {
    term.writeln('知识库健康审计中...');
    const r = await api('/api/audit');
    term.writeln('\x1b[1m文档 ' + r.docs + ' / 链接 ' + r.links + '\x1b[0m' + (r.dangling.length ? '\x1b[33m（悬空 ' + r.dangling.length + '）\x1b[0m' : ''));
    if (r.orphans.length) {
      term.writeln('\x1b[33m⚠ 孤立文档（' + r.orphans.length + '）——无入链也无出链:\x1b[0m');
      for (const p of r.orphans) term.writeln('  ' + p + '  \x1b[90m(建议 /link ' + p + ' <关联文档> 或归档)\x1b[0m');
    }
    if (r.no_out.length) {
      term.writeln('\x1b[33m⚠ 无出链文档（' + r.no_out.length + '）——从不链向别人:\x1b[0m');
      for (const p of r.no_out) term.writeln('  ' + p);
    }
    if (r.duplicates.length) {
      term.writeln('\x1b[31m⚠ 重复标题（' + r.duplicates.length + '）:\x1b[0m');
      for (const [t, n] of r.duplicates) term.writeln('  "' + t + '" × ' + n);
    }
    if (r.dangling.length) {
      term.writeln('\x1b[33m⚠ 悬空链接（' + r.dangling.length + '）:\x1b[0m');
      for (const [s, d] of r.dangling) term.writeln('  ' + s + ' → [[' + d + ']]');
    }
    if (r.mentions.length) {
      term.writeln('\x1b[36m💡 建议补链接（' + r.mentions.length + '）——正文提到但未链接:\x1b[0m');
      for (const m of r.mentions.slice(0, 20)) {
        term.writeln('  ' + m.src + '  →  [[\x1b[1m' + m.dst + '\x1b[0m]]  \x1b[90m(/link ' + m.src + ' ' + m.dst_path + ')\x1b[0m');
      }
      if (r.mentions.length > 20) term.writeln('  ... 共 ' + r.mentions.length + ' 条');
    }
    if (!r.orphans.length && !r.no_out.length && !r.duplicates.length && !r.dangling.length && !r.mentions.length) {
      term.writeln('\x1b[32m✓ 知识库健康，无盲区无冲突\x1b[0m');
    }
    term.writeln('\x1b[90m(图形版: /view automation 审核栏目 · 补链建议一键应用 · 审计按钮/状态行 ⚠ 直达)\x1b[0m');
  }

  // /risk：风控预警（律师案件：时效到期/证据缺口/信息缺失；纯规则零 LLM）
  async function riskCmd() {
    term.writeln('风控扫描中...');
    const r = await api('/api/risk');
    const items = r.items || [];
    if (!items.length) {
      term.writeln('\x1b[32m✓ 无风控预警（律师项目案件笔记的 deadline/证据状态会自动检测）\x1b[0m');
      return;
    }
    // 紧急优先：已过期/7 天内时效排前
    const sorted = items.slice().sort((a, b) => {
      const ad = a.kind === 'deadline' ? (a.days == null ? 99 : a.days) : 50;
      const bd = b.kind === 'deadline' ? (b.days == null ? 99 : b.days) : 50;
      return ad - bd;
    });
    for (const it of sorted) {
      const color = it.kind === 'deadline' && it.days != null && it.days <= 7 ? '\x1b[31m' : '\x1b[33m';
      term.writeln(color + '⚠ ' + it.label + '\x1b[0m  \x1b[90m[' + it.path + ']\x1b[0m');
    }
    term.writeln('\x1b[90m(共 ' + items.length + ' 条：时效 ' + (r.deadlines || 0) + ' · 证据缺口 ' + (r.evidence_gaps || 0) + ' · 信息缺失 ' + (r.info_missing || 0) + '；打开对应笔记补充即可消除)\x1b[0m');
  }

  // /conflicts：重复标题（带路径）+ 悬空链接
  async function conflicts() {
    const a = await api('/api/audit');
    term.writeln('冲突与盲区检查:');
    if (a.duplicates.length) {
      term.writeln('\x1b[31m重复标题（' + a.duplicates.length + '）:\x1b[0m');
      for (const [t, n, paths] of a.duplicates) {
        term.writeln('  "' + t + '" × ' + n);
        const ps = paths.split(' | ');
        for (const p of ps) term.writeln('    ' + p);
        if (ps.length === 2) term.writeln('    \x1b[90m(/diff ' + ps[0] + ' ' + ps[1] + ' 对比内容)\x1b[0m');
      }
    } else {
      term.writeln('\x1b[32m✓ 无重复标题\x1b[0m');
    }
    if (a.dangling.length) {
      term.writeln('\x1b[33m悬空链接（' + a.dangling.length + '）:\x1b[0m');
      for (const [s, d] of a.dangling) term.writeln('  ' + s + ' → [[' + d + ']]');
    } else {
      term.writeln('\x1b[32m✓ 无悬空链接\x1b[0m');
    }
  }

  // /diff <A> <B>：行级对比（+ 绿 / - 红）
  async function diffCmd(a, b) {
    if (!a || !b) {
      term.writeln('\x1b[33m用法：/diff <文档A> <文档B>\x1b[0m');
      return;
    }
    const [ra, rb] = await Promise.all([
      api('/api/file?path=' + encodeURIComponent(a)),
      api('/api/file?path=' + encodeURIComponent(b)),
    ]);
    term.writeln('\x1b[1;36m' + ra.path + '  vs  ' + rb.path + '\x1b[0m');
    const la = ra.content.replace(/\r\n/g, '\n').split('\n');
    const lb = rb.content.replace(/\r\n/g, '\n').split('\n');
    const d = Core.diffLines(la, lb);
    let shown = 0;
    for (const x of d) {
      if (x.t === ' ') continue;
      shown++;
      term.writeln((x.t === '+' ? '\x1b[32m+ ' : '\x1b[31m- ') + x.line + '\x1b[0m');
    }
    term.writeln('\x1b[90m(共 ' + shown + ' 处差异行)\x1b[0m');
  }

  async function linkCmd(src, dst) {
    if (!src || !dst) {
      term.writeln('\x1b[33m用法：/link <源文档> <目标文档>\x1b[0m');
      return;
    }
    const r = await api('/api/link', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ src, dst }),
    });
    if (r.ok) {
      term.writeln('\x1b[32m✓ 已补链接: \x1b[0m' + r.src + ' 追加 ' + r.link + '（INDEX 与图谱已重建）');
    } else {
      term.writeln('\x1b[33m' + (r.note || '未添加') + ': ' + r.src + ' → ' + r.dst + '\x1b[0m');
    }
  }

  // /link-all：一键应用 audit 的全部补链接建议（逐条调用 /api/link，自带去重）
  async function linkAll() {
    term.writeln('获取审计建议...');
    const r = await api('/api/audit');
    if (!r.mentions.length) {
      term.writeln('\x1b[32m没有可应用的补链接建议。\x1b[0m');
      return;
    }
    term.writeln('将批量应用 ' + r.mentions.length + ' 条补链接建议:');
    for (const m of r.mentions.slice(0, 20)) term.writeln('  ' + m.src + ' → [[' + m.dst + ']]');
    if (r.mentions.length > 20) term.writeln('  ... 共 ' + r.mentions.length + ' 条');
    term.writeln('\x1b[90m(执行中...)\x1b[0m');
    let ok = 0, skipped = 0, failed = 0;
    for (const m of r.mentions) {
      try {
        const res = await api('/api/link', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ src: m.src, dst: m.dst_path }),
        });
        if (res.ok) ok++;
        else skipped++;
      } catch (e) {
        failed++;
        term.writeln('\x1b[31m失败: ' + m.src + ' → ' + m.dst + ': ' + e.message + '\x1b[0m');
      }
    }
    term.writeln(
      '\x1b[32m✓ 完成: \x1b[0m新增 ' + ok + ' 条，跳过 ' + skipped + ' 条' +
      (failed ? '，失败 ' + failed + ' 条' : '') + '（INDEX 与图谱已重建）'
    );
  }

  // /suggest：无参 = 盲区主题自动提议；带主题 = 补全该主题的新文档。产物均进待审
  async function suggest(topic) {
    if (!topic) {
      // —— 盲区分析模式：审计 → LLM 提议缺失主题 → 生成盲区文档 → 待审 ——
      term.writeln('\x1b[90m(盲区分析: 审计 → LLM 提议缺失主题 → 生成盲区文档 → 待审)\x1b[0m');
      const a = await api('/api/audit');
      const summary = [
        '文档数: ' + a.docs + '，链接: ' + a.links,
        '孤立文档: ' + (a.orphans.join('、') || '无'),
        '无出链文档: ' + (a.no_out.join('、') || '无'),
        '重复标题: ' + (a.duplicates.map((d) => '"' + d[0] + '"×' + d[1]).join('、') || '无'),
        '悬空链接: ' + a.dangling.length + ' 条',
        '建议补链接: ' + a.mentions.length + ' 条',
      ].join('\n');
      const system =
        '你是知识库盲区分析师。基于审计数据指出知识库的薄弱点，并提议 2-3 个应补充的主题。' +
        '生成一篇「知识盲区分析」文档：以 # 标题开头，包含 ## 盲区清单（列出具体薄弱点与依据）、' +
        '## 建议新文档（每个主题给出理由与 [[相关链接]]）。直接输出文档正文，不要额外解释。';
      const noteText = await llmText([
        { role: 'system', content: system },
        { role: 'user', content: '知识库审计数据：\n' + summary },
      ]);
      term.writeln('\x1b[1;32m──── 盲区分析（待审）────\x1b[0m');
      term.writeln(renderMdFile(noteText));
      const saved = await applySave({ path: 'notes/知识盲区分析-' + Date.now() + '.md', mode: 'new', content: noteText });
      term.writeln('\x1b[33m💾 已进入待审: \x1b[0m' + saved + '  （/view automation 审核 · /approve 确认 · /reject 丢弃）');
      return;
    }
    term.writeln('\x1b[90m(补全: LLM 生成「' + topic + '」新文档 → 待审)\x1b[0m');
    const kws = Core.extractKeywords(topic);
    const query = kws.length ? kws.join(' ') : topic;
    let sr;
    try {
      sr = await api('/api/search?q=' + encodeURIComponent(query) + '&layer=notes&ctx=1');
    } catch (e) {
      sr = { hits: [] };
    }
    const top = sr.hits.slice(0, 8);
    const frag = top
      .map((h) => '[来源 ' + h.file + ':' + h.line + ']\n' + (h.context || h.text))
      .join('\n\n');
    const system =
      '你是知识库补全助手。主题在知识库中可能资料不足，请生成一篇新文档：' +
      '以 # 标题开头，包含 概述、要点列表、相关条目；' +
      '相关条目中引用知识库已有文档用 [[文件名]] 双链（只引用确实相关的）。' +
      '若已有片段，基于片段补全；若知识库确实没有该主题资料，基于模型知识撰写并注明"知识库暂无该主题资料"。' +
      '直接输出文档正文，不要额外解释。';
    const userMsg = '主题：' + topic + '\n\n知识库相关片段（可能为空）：\n' + (frag || '(无)');
    const noteText = await llmText([
      { role: 'system', content: system },
      { role: 'user', content: userMsg },
    ]);
    term.writeln('\x1b[1;32m──── 生成文档（待审）────\x1b[0m');
    term.writeln(renderMdFile(noteText));
    const safe = (topic.replace(/[\\/:*?"<>|\s]+/g, '-').replace(/^-+|-+$/g, '') || '新文档').slice(0, 30);
    const saved = await applySave({ path: 'notes/' + safe + '.md', mode: 'new', content: noteText });
    term.writeln('\x1b[33m💾 已进入待审: \x1b[0m' + saved + '  （/view automation 审核 · /approve 确认 · /reject 丢弃）');
  }

  // ---------- 管理命令 ----------

  async function search(q) {
    if (!q) {
      term.writeln('\x1b[33m用法：/search <关键词>（空格分隔，任一命中；含大写字母则区分大小写）\x1b[0m');
      return;
    }
    const r = await api('/api/search?q=' + encodeURIComponent(q));
    term.writeln(
      '检索 \x1b[1m' + r.query + '\x1b[0m（layer=' + r.layer + '）→ ' +
      r.file_count + ' 个文件 / ' + r.hit_count + ' 处命中'
    );
    if (!r.hits.length) {
      term.writeln('无结果。');
      return;
    }
    let last = '';
    for (const h of r.hits) {
      if (h.file !== last) {
        const sec = h.section ? ' — \x1b[1m' + h.section + '\x1b[0m' : '';
        term.writeln('\x1b[1;36m' + h.file + '\x1b[0m' + sec);
        last = h.file;
      }
      term.writeln('  \x1b[90m' + String(h.line).padStart(3) + '\x1b[0m  ' + h.text);
    }
    term.writeln('提示：输入 \x1b[1mopen ' + r.hits[0].file + '\x1b[0m 查看全文');
  }

  async function openFile(p) {
    if (!p) {
      term.writeln('\x1b[33m用法：open <路径>\x1b[0m');
      return;
    }
    const r = await api('/api/file?path=' + encodeURIComponent(p));
    term.writeln('\x1b[1;36m' + r.path + '\x1b[0m');
    term.writeln(renderMdFile(r.content));
  }

  async function l1() {
    const r = await api('/api/l1');
    if (!r.l1 || !r.l1.length) {
      term.writeln('L1 层为空。');
      return;
    }
    for (const f of r.l1) {
      term.writeln('\x1b[1;36m' + f.name + '\x1b[0m  (\x1b[90m' + f.path + '\x1b[0m)');
      term.writeln(renderMdFile(f.head, { frontmatter: 'hide' }));
      term.writeln('');
    }
  }

  async function sync() {
    const r = await api('/api/kb/sync', { method: 'POST' });
    term.writeln('\x1b[32m✓\x1b[0m INDEX 已重建：' + r.files + ' 篇 -> ' + r.index);
  }

  // 全量同步（快捷按钮「同步」用）：INDEX + 技能注册表 + 图谱，与心跳自动同步口径一致
  async function syncAll() {
    const [idx, g] = await Promise.all([
      api('/api/kb/sync', { method: 'POST' }).catch((e) => term.writeln('\x1b[31mINDEX 重建失败: ' + e.message + '\x1b[0m')),
      api('/api/graph/sync', { method: 'POST' }).catch((e) => term.writeln('\x1b[31m图谱重建失败: ' + e.message + '\x1b[0m')),
    ]);
    if (idx) term.writeln('\x1b[32m✓\x1b[0m INDEX 已重建：' + idx.files + ' 篇');
    if (g && g.graph) term.writeln('\x1b[32m✓\x1b[0m 图谱已重建：' + g.graph.docs + ' 文档 / ' + g.graph.links + ' 链接');
    refreshStatus();
  }

  async function cfg() {
    const r = await api('/api/config');
    term.writeln(JSON.stringify(r, null, 2));
  }

  async function health() {
    const r = await api('/api/health');
    term.writeln(JSON.stringify(r));
  }

  // /heartbeat [on|off|interval <秒>|status] —— 心跳自动同步（自组织自动发现）
  async function heartbeatCmd(parts) {
    const act = parts[0] || 'status';
    try {
      if (act === 'on' || act === 'off') {
        const r = await api('/api/heartbeat', { method: 'POST', body: JSON.stringify({ enabled: act === 'on' }) });
        term.writeln('\x1b[32m✓ 心跳自动同步: ' + (r.enabled ? '开' : '关') + '\x1b[0m（' + r.interval_secs + 's 周期，变化自动重建 INDEX+图谱+审计）');
      } else if (act === 'interval') {
        const v = parseInt(parts[1], 10);
        if (!v || v < 5) { term.writeln('\x1b[33m用法: /heartbeat interval <秒≥5>\x1b[0m'); return; }
        const r = await api('/api/heartbeat', { method: 'POST', body: JSON.stringify({ interval_secs: v }) });
        term.writeln('\x1b[32m✓ 检查周期: ' + r.interval_secs + 's\x1b[0m');
      } else {
        const r = await api('/api/heartbeat');
        term.writeln('心跳自动同步: ' + (r.enabled ? '\x1b[32m开\x1b[0m' : '\x1b[90m关\x1b[0m') +
          ' · 周期 ' + r.interval_secs + 's' +
          (r.last_sync ? ' · 上次同步 ' + r.last_sync + ' · ' + r.files + ' 篇' : ''));
        if (r.audit && (r.audit.orphans || r.audit.dangling || r.audit.duplicates || r.audit.mentions)) {
          term.writeln('\x1b[33m⚠ 最近审计发现: 孤立 ' + r.audit.orphans + ' · 悬空 ' + r.audit.dangling +
            ' · 重复 ' + r.audit.duplicates + ' · 提及未链接 ' + r.audit.mentions + '（/audit 看详情，/link-all 一键修复）\x1b[0m');
        } else if (r.audit) {
          term.writeln('\x1b[32m✓ 最近审计无发现\x1b[0m');
        }
      }
    } catch (e) {
      term.writeln('\x1b[31m失败: ' + e + '\x1b[0m');
    }
    refreshStatus();
  }
})();
