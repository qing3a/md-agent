/* md-agent 终端前端：xterm.js 命令行操作双层知识库 + Agent 问答回路
 * 回路：启动注入 L1（规范/记忆/索引）→ 用户提问 → 提取关键词 → 检索 L2 → 拼 Prompt → /api/llm 代理 → 回答
 */
(function () {
  if (typeof Terminal === 'undefined') {
    document.body.innerHTML =
      '<pre style="color:#f38ba8;padding:16px">xterm.js 未加载（需联网访问 CDN）。请检查网络后刷新。</pre>';
    return;
  }
  const term = new Terminal({
    cursorBlink: true,
    fontSize: 14,
    fontFamily: 'Consolas, "Cascadia Mono", monospace',
    theme: { background: '#1e1e2e', foreground: '#cdd6f4', cursor: '#f5e0dc' },
    scrollback: 2000,
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  term.open(document.getElementById('terminal'));
  fit.fit();
  // resize 防抖（150ms），避免拖动窗口时频繁重排终端
  let fitTimer = null;
  window.addEventListener('resize', () => {
    clearTimeout(fitTimer);
    fitTimer = setTimeout(() => fit.fit(), 150);
  });

  const PROMPT = '\x1b[1;34mmd-agent>\x1b[0m ';
  let L1_TEXT = ''; // 启动时注入的 L1 层全文
  let history = loadHistory(); // 多轮对话记忆（localStorage 持久化，刷新不丢）
  const MAX_HISTORY = 8; // 最近 4 轮

  function loadHistory() {
    try {
      const h = JSON.parse(localStorage.getItem('md-agent-history') || '[]');
      return Array.isArray(h) ? h.slice(-MAX_HISTORY) : [];
    } catch (e) { return []; }
  }
  function saveHistory() {
    try { localStorage.setItem('md-agent-history', JSON.stringify(history.slice(-MAX_HISTORY))); } catch (e) { /* 忽略 */ }
  }

  // ---- DOM chrome（输入条 / 状态条 / 补全面板）----
  // UI 迁出终端流：输入框、边框、状态栏全部为 DOM 元素（业界标准——resize 由 CSS 自动适配，不再有字符边框变形）
  const cmdInput = document.getElementById('cmd-input');
  const sendBtn = document.getElementById('send-btn');
  const autoPanel = document.getElementById('autocomplete');
  const statusDot = document.getElementById('status-dot');
  const statusText = document.getElementById('status-text');
  const statusWarn = document.getElementById('status-warn');
  const quickBtns = [...document.querySelectorAll('#quick-btns button')];

  let busy = false;          // 命令/回答进行中（提交/按钮禁用，输入框仍可编辑）
  let cmdHistory = [];       // 命令行历史（会话内；区别于多轮对话 history）
  let histIdx = -1;
  let currentAbort = null;   // 当前 LLM 流的 AbortController（Ctrl+C 中断回答）

  // 终端实测列宽（消息块铺背景用；term.cols 是 fit 估算值，实测偏大 2 列）
  function trueCols() {
    try {
      const cw = term._core.dimensions.css.cell.width; // xterm 内部 cell 宽（像素）
      if (cw && cw > 0) {
        const w = document.querySelector('.xterm-rows').clientWidth;
        return Math.max(20, Math.floor(w / cw));
      }
    } catch (e) { /* fallthrough */ }
    return term.cols - 2; // 兜底：留 2 列余量防 wrap
  }

  // ---------- 输入与提交 ----------

  // 提交一条命令/问题：流内整行背景块 + 执行；回答期间（busy）忽略
  async function submitCmd(text) {
    const t = String(text || '').trim();
    if (!t || busy || confirmCb) return;
    const bg = '\x1b[48;2;49;50;68m'; // #313244 深蓝灰背景（提交消息块）
    for (const seg of t.split('\n')) {
      term.write(bg + ' '.repeat(trueCols()) + '\r' + PROMPT + seg + '\x1b[0m\r\n');
    }
    cmdInput.value = '';
    autoSize();
    pushHistory(t);
    closeAuto();
    busy = true;
    setBusyUI();
    try {
      await run(t);
    } catch (e) {
      term.writeln('\x1b[31m' + ((e && e.message) || e) + '\x1b[0m');
    } finally {
      busy = false;
      setBusyUI();
      refreshStatus();
      cmdInput.focus();
    }
  }

  function setBusyUI() {
    sendBtn.disabled = busy;
    quickBtns.forEach((b) => (b.disabled = busy));
  }

  // textarea 自适应高度（1~3 行）
  function autoSize() {
    cmdInput.rows = Math.min(1 + (cmdInput.value.match(/\n/g) || []).length, 3);
  }

  cmdInput.addEventListener('input', () => { autoSize(); renderAutoComplete(); });
  cmdInput.addEventListener('focus', () => { if (!cmdInput.value) openAuto(true); });
  cmdInput.addEventListener('keydown', (ev) => {
    const k = ev.key;
    if (k === 'Enter' && !ev.shiftKey) {
      ev.preventDefault();
      if (autoOpen() && acSel >= 0) { acceptAuto(); return; }
      submitCmd(cmdInput.value);
    } else if (k === 'ArrowDown' || k === 'ArrowUp') {
      if (autoOpen()) { ev.preventDefault(); moveAc(k === 'ArrowDown' ? 1 : -1); return; }
      if (!cmdInput.value) { ev.preventDefault(); navHistory(k === 'ArrowUp' ? 1 : -1); }
    } else if (k === 'Tab') {
      if (autoOpen()) { ev.preventDefault(); acceptAuto(); }
      else { ev.preventDefault(); openAuto(true); }
    } else if (k === 'Escape') {
      if (autoOpen()) { ev.preventDefault(); closeAuto(); }
    } else if (k === 'c' && ev.ctrlKey && !cmdInput.value && currentAbort) {
      ev.preventDefault();
      currentAbort.abort();
      term.writeln('\x1b[90m(已发送中断)\x1b[0m');
    }
  });
  sendBtn.addEventListener('click', () => submitCmd(cmdInput.value));
  quickBtns.forEach((btn) => btn.addEventListener('click', () => submitCmd(btn.dataset.cmd)));
  // 点击输入区外部关闭补全
  document.addEventListener('click', (ev) => {
    if (!cmdInput.contains(ev.target) && !autoPanel.contains(ev.target)) closeAuto();
  });

  // ---------- 输入历史（会话内，空输入时 ArrowUp/Down 翻动） ----------
  function pushHistory(t) {
    if (cmdHistory[cmdHistory.length - 1] === t) return;
    cmdHistory.push(t);
    if (cmdHistory.length > 100) cmdHistory.shift();
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
      if (histIdx >= cmdHistory.length) { histIdx = -1; cmdInput.value = ''; autoSize(); return; }
    }
    cmdInput.value = cmdHistory[histIdx] || '';
    autoSize();
    const len = cmdInput.value.length;
    cmdInput.setSelectionRange(len, len);
  }

  // ---------- 命令补全面板 ----------
  const COMMANDS = [
    ['/help', '命令列表'], ['/search', '检索双层库'], ['open', '查看 KB 内 MD 文件'],
    ['/l1', '查看 L1 规范/索引/记忆层'], ['/sync', '重建 INDEX.md'], ['/digest', '整理新笔记'],
    ['/remember', '手动沉淀到记忆'], ['/graph', '知识图谱/关联簇'], ['/orphans', '孤立文档'],
    ['/projects', '项目统计'], ['/tags', '标签统计'], ['/rescan', '重建知识图谱'],
    ['/pending', '查看待审'], ['/preview', '行级预览'], ['/approve', '批准待审'],
    ['/reject', '拒绝待审'], ['/view', '面板渲染层'], ['/audit', '知识库健康审计'],
    ['/conflicts', '冲突检查'], ['/diff', '行级对比'], ['/link', '补链接'],
    ['/link-all', '批量补链接'], ['/suggest', '补全缺失文档'], ['/fetch', '抓取网页'],
    ['/page', '动态网页读取'], ['/task', '任务引擎'], ['/clear', '清空多轮记忆'],
    ['/config', '查看配置'], ['/heartbeat', '心跳自动同步'], ['/health', '健康检查'],
    ['clear', '清屏'],
  ];
  let acItems = [];
  let acSel = -1;
  function autoOpen() { return !autoPanel.classList.contains('hidden'); }
  function closeAuto() { autoPanel.classList.add('hidden'); acItems = []; acSel = -1; }
  function escHtml(s) {
    return String(s).replace(/[&<>"']/g, (m) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[m]));
  }
  function renderAutoComplete() {
    const v = cmdInput.value;
    const head = (v.match(/^\s*(\/[^\s]*)?/) || [])[1] || '';
    const list = head
      ? COMMANDS.filter(([c]) => c.startsWith(head)).slice(0, 8)
      : COMMANDS.slice(0, 8); // 空输入 → 常用命令
    if (!list.length || (head && list[0][0] === head)) { closeAuto(); return; }
    acItems = list;
    acSel = Math.max(0, list.findIndex(([c]) => c === head));
    autoPanel.innerHTML = acItems.map(([c, d], i) =>
      '<div class="ac-item' + (i === acSel ? ' sel' : '') + '" data-i="' + i + '">' +
      escHtml(c) + '<span class="ac-desc">' + escHtml(d) + '</span></div>'
    ).join('');
    autoPanel.classList.remove('hidden');
  }
  function updateSelUI() {
    autoPanel.querySelectorAll('.ac-item').forEach((el, i) => el.classList.toggle('sel', i === acSel));
  }
  function moveAc(d) {
    if (!acItems.length) return;
    acSel = (acSel + d + acItems.length) % acItems.length;
    updateSelUI();
    const sel = autoPanel.querySelector('.ac-item.sel');
    if (sel) sel.scrollIntoView({ block: 'nearest' });
  }
  function acceptAuto() {
    if (acSel < 0 || !acItems.length) return;
    const cmd = acItems[acSel][0];
    cmdInput.value = cmd + cmdInput.value.replace(/^\s*\/?[^\s]*/, '');
    autoSize();
    closeAuto();
    const len = cmdInput.value.length;
    cmdInput.setSelectionRange(len, len);
    cmdInput.focus();
  }
  function openAuto(selectFirst) {
    renderAutoComplete();
    if (selectFirst && acItems.length) { acSel = 0; updateSelUI(); }
  }
  autoPanel.addEventListener('click', (ev) => {
    const el = ev.target.closest('.ac-item');
    if (!el) return;
    acSel = parseInt(el.dataset.i, 10);
    acceptAuto();
  });

  // ---- 启动欢迎 banner + 状态信息（异步汇总；bannerDone 保证状态行先于输入框打印）----
  let bannerDone = Promise.resolve();
  function printBanner() {
    term.writeln('\x1b[90m' + '─'.repeat(Math.min(trueCols(), 64)) + '\x1b[0m');
    term.writeln('\x1b[1;36m  md-agent\x1b[0m  \x1b[90m本地双层 MD 知识库 Agent\x1b[0m');
    term.writeln('\x1b[90m' + '─'.repeat(Math.min(trueCols(), 64)) + '\x1b[0m');
    term.writeln('  直接输入问题 → 知识库问答（流式）·  /help 查看命令 ·  配置页 /config.html');
    term.writeln('\x1b[90m  状态加载中…\x1b[0m');
    // 状态汇总：健康 / KB / 模型 / 待审 / 任务 / 图谱
    bannerDone = Promise.all([
      fetch('/api/health').then((r) => r.json()).catch(() => null),
      fetch('/api/config').then((r) => r.json()).catch(() => null),
      fetch('/api/kb/pending').then((r) => r.json()).catch(() => null),
      fetch('/api/tasks').then((r) => r.json()).catch(() => null),
      fetch('/api/graph/stats').then((r) => r.json()).catch(() => null),
    ]).then(([h, c, p, t, g]) => {
      const ver = (h && h.version) || '?';
      const kb = (c && c.kb_root) || '-';
      const model = (c && c.llm && c.llm.model) || '未配置';
      const pend = (p && Array.isArray(p.pending)) ? p.pending.length : '-';
      const todo = (t && t.stats) ? (t.stats.todo || 0) + (t.stats.doing || 0) : '-';
      const gs = (g && g.docs) ? (g.docs || 0) + ' 文档 / ' + (g.links || 0) + ' 链接' : '-';
      term.writeln('\x1b[90m  v' + ver + ' · KB: ' + kb + ' · 图谱 ' + gs + '\x1b[0m');
      term.writeln('\x1b[90m  模型 ' + model + ' · 待审 ' + pend + ' · 进行中任务 ' + todo + '\x1b[0m');
    }).catch(() => {});
  }
  printBanner();

  // ---- DOM 状态条（输入框下方固定行；命令后刷新 + 8s 轮询，不随终端滚动）----
  function refreshStatus() {
    Promise.all([
      fetch('/api/health').then((r) => r.json()).catch(() => null),
      fetch('/api/config').then((r) => r.json()).catch(() => null),
      fetch('/api/kb/pending').then((r) => r.json()).catch(() => null),
      fetch('/api/tasks').then((r) => r.json()).catch(() => null),
      fetch('/api/graph/stats').then((r) => r.json()).catch(() => null),
      fetch('/api/heartbeat').then((r) => r.json()).catch(() => null),
    ]).then(([h, c, p, t, g, hb]) => {
      const ok = !!(h && h.status === 'ok');
      const model = (c && c.llm && c.llm.model) || '未配置 LLM';
      const kb = (c && c.kb_root) || '-';
      const pend = (p && Array.isArray(p.pending)) ? p.pending.length : '-';
      const todo = (t && t.stats) ? (t.stats.todo || 0) + (t.stats.doing || 0) : '-';
      const gs = (g && g.docs) ? (g.docs || 0) + ' 文档 / ' + (g.links || 0) + ' 链接' : '-';
      const hbTxt = hb ? (hb.enabled ? '心跳开' : '心跳关') : '';
      statusDot.className = ok ? 'ok' : 'bad';
      statusDot.title = ok ? '服务运行中' : '服务异常';
      const parts = [
        ok ? '服务运行中' : '服务异常',
        '模型 ' + model, 'KB ' + kb, '待审 ' + pend, '任务 ' + todo, '图谱 ' + gs,
      ];
      if (hbTxt) parts.push(hbTxt);
      statusText.textContent = parts.join(' · ');
      let warn = '';
      if (hb && hb.audit && (hb.audit.orphans || hb.audit.dangling || hb.audit.duplicates || hb.audit.mentions)) {
        warn = '⚠ 审计' +
          (hb.audit.orphans ? '孤立' + hb.audit.orphans : '') +
          (hb.audit.dangling ? '悬空' + hb.audit.dangling : '') +
          (hb.audit.duplicates ? '重复' + hb.audit.duplicates : '');
      }
      statusWarn.textContent = warn;
      statusWarn.style.display = warn ? '' : 'none';
    }).catch(() => {});
  }
  refreshStatus();
  setInterval(refreshStatus, 8000);

  // 启动时注入 L1（类 CLAUDE.md：规范 / 记忆 / 索引层）
  (async function loadL1() {
    try {
      const res = await fetch('/api/l1?full=1');
      const b = await res.json();
      if (b.l1 && b.l1.length) {
        L1_TEXT = b.l1.map((f) => '【' + f.name + '】\n' + f.content).join('\n\n');
        term.writeln(
          '\x1b[90m(L1 已注入 ' + b.l1.length + ' 个文件: ' + b.l1.map((f) => f.name).join(' ') + ')\x1b[0m'
        );
      } else {
        term.writeln('\x1b[33m警告: L1 层为空，规范/记忆未注入\x1b[0m');
      }
    } catch (e) {
      term.writeln('\x1b[33mL1 加载失败: ' + e.message + '\x1b[0m');
    }
    await bannerDone;
    cmdInput.focus();
  })();

  // 终端确认机制：confirm(msg) 挂起等待 y/n 按键（写操作人审闭环的基础设施）
  // 焦点从输入框转到终端收 y/n，结束后回输入框
  let confirmCb = null;
  function confirm(msg) {
    return new Promise((resolve) => {
      confirmCb = (ok) => resolve(ok);
      term.writeln('\x1b[33m' + msg + ' \x1b[1m(y/N)\x1b[0m');
      term.focus();
    });
  }

  // 终端键盘只处理 confirm（y/n）；普通输入走 DOM 输入框（cmdInput）
  term.onData((data) => {
    if (!confirmCb) return;
    const code = data.charCodeAt(0);
    const c = data.trim().toLowerCase();
    const cb = confirmCb;
    if (c === 'y') { confirmCb = null; term.writeln('\x1b[32m✓ 已确认\x1b[0m'); cb(true); cmdInput.focus(); }
    else if (c === 'n' || data === '\r' || code === 27) { confirmCb = null; term.writeln('\x1b[90m已取消\x1b[0m'); cb(false); cmdInput.focus(); }
    else { term.write('\b \b'); } // 忽略其他键
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

  async function api(path, opts) {
    const res = await fetch(path, opts);
    const body = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error((body && body.error) || 'HTTP ' + res.status);
    return body;
  }

  async function run(cmd) {
    const [head, ...rest] = cmd.split(/\s+/);
    switch (head) {
      case '/help': help(); break;
      case '/search': await search(rest.join(' ')); break;
      case 'open': await openFile(rest[0]); break;
      case '/l1': await l1(); break;
      case '/sync': await sync(); break;
      case '/config': await cfg(); break;
      case '/remember': await remember(rest); break;
      case '/digest': await digest(rest.join(' ')); break;
      case '/clear': history = []; saveHistory(); term.writeln('多轮记忆已清空'); break;
      case '/link-all': await linkAll(); break;
      case '/fetch': await fetchCmd(rest); break;
      case '/page': await pageCmd(rest); break;
      case '/task': await taskCmd(rest); break;
      case '/graph': await graph(rest.join(' ')); break;
      case '/orphans': await orphans(); break;
      case '/projects': await projects(); break;
      case '/tags': await tags(); break;
      case '/rescan': await rescan(); break;
      case '/pending': await pendingList(); break;
      case '/preview': await previewPending(rest[0]); break;
      case '/approve': await pendingAct('approve', rest[0]); break;
      case '/reject': await pendingAct('reject', rest[0]); break;
      case '/view': await viewCmd(rest[0]); break;
      case '/audit': await auditCmd(); break;
      case '/conflicts': await conflicts(); break;
      case '/diff': await diffCmd(rest[0], rest[1]); break;
      case '/link': await linkCmd(rest[0], rest[1]); break;
      case '/suggest': await suggest(rest.join(' ')); break;
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
    term.writeln('命令列表：');
    term.writeln('  直接输入问题          知识库问答（流式输出 + 多轮记忆 + 自动沉淀）');
    term.writeln('  /search <关键词>       检索双层库（多关键词任一命中，显示所属小节）');
    term.writeln('  open <路径>            查看 KB 内 MD 文件，如 open notes/rag/xxx.md');
    term.writeln('  /l1                    查看 L1 规范/索引/记忆层');
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
    term.writeln('  /audit                知识库健康审计（盲区/冲突/补链接建议）');
    term.writeln('  /conflicts             冲突检查（重复标题/悬空链接）   /diff <A> <B> 行级对比');
    term.writeln('  /link <源> <目标>      补链接（在源文档追加 [[目标]]，人工确认）');
    term.writeln('  /link-all              一键应用 /audit 的全部补链接建议');
    term.writeln('  /suggest <主题>        LLM 补全缺失主题的新文档（进待审）');
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

  // 关键词提取：ASCII 单词直接提取；中文按多字功能词 + 语气助词切分，
  // 切出的 CJK 片段(≥2)取「整段 + 滑动双字」，避免单字停用字切断实体词（如 向量/配置）。
  const FW = [
    '为什么','怎么样','怎么能','是不是','会不会','有没有','能不能','需要','帮忙',
    '什么','怎么','如何','请问','怎样','为啥','咋','一下','这个','那个','这些','那些',
    '因为','所以','如果','但是','然后','可以','就是','不是','一个','我们','你们','他们',
  ].sort((a, b) => b.length - a.length); // 长的先切，如 为什么 先于 什么
  const PARTICLES = new Set('吗呢啊呀吧么的得地了着是吗哦嘛嗯');
  function extractKeywords(q) {
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
  }

  async function ask(question) {
    term.writeln('\x1b[90m(Agent: 提取关键词 → 检索 L2 → 调用 LLM)\x1b[0m');
    const kws = extractKeywords(question);
    const query = kws.length ? kws.join(' ') : question;
    term.writeln('\x1b[90m关键词: ' + (kws.length ? query : '(无，用原文)') + '\x1b[0m');

    // 1. 检索 L2（L1 已在 system 里，不重复检索）
    let sr;
    try {
      sr = await api('/api/search?q=' + encodeURIComponent(query) + '&layer=notes&ctx=1');
    } catch (e) {
      term.writeln('\x1b[31m检索失败: ' + e.message + '\x1b[0m');
      return;
    }
    const top = sr.hits.slice(0, 8);
    if (!top.length) {
      term.writeln('\x1b[33m知识库无相关片段（仅靠 L1 规范与模型自身知识回答）\x1b[0m');
    } else {
      term.writeln('\x1b[90m命中 ' + sr.file_count + ' 文件 / ' + sr.hit_count + ' 处，注入前 ' + top.length + ' 条\x1b[0m');
    }

    // 2. 组装 Prompt（system + 多轮历史 + 当前问题）
    const frag = top
      .map(
        (h) =>
          '[来源 ' + h.file + ':' + h.line + ' 小节:' + (h.section || '(frontmatter)') + ']\n' +
          (h.context || h.text)
      )
      .join('\n\n');
    const system = [
      '你是本地双层 MD 知识库的检索问答助手。',
      '以下是知识库 L1 规范/记忆/索引层（权威约定，需遵循）：',
      L1_TEXT || '(L1 未加载)',
      '',
      '回答规则：',
      '1. 优先依据用户消息中给出的检索片段回答，引用格式 [文件:行号]；',
      '2. 片段不足时如实说明，不要编造；',
      '3. 用中文简洁回答；',
      '4. 多轮对话中注意保持与上文一致（引用只需标注本轮片段来源）；',
      '5. 今天是 ' + localToday() + '。若本次问答产生了值得沉淀的知识（新事实、已定决策、用户纠正、新规范），在回答末尾单独附写回块（不要放进代码块）：',
      '   <!-- md-agent-save -->',
      '   {"path":"相对KB根路径","mode":"append|new","content":"markdown正文"}',
      '   - 新知识：path 指向 notes/ 下的 L2 文件，mode=new，正文含 # 标题；',
      '   - 追加/决策/纠正：path=MEMORY.md，mode=append；',
      '   - 没有可沉淀内容时不要输出该块。',
    ].join('\n');
    const userMsg = [
      '问题：' + question,
      '',
      '知识库检索片段（L2）：',
      frag || '(无片段)',
    ].join('\n');
    const messages = [
      { role: 'system', content: system },
      ...history,
      { role: 'user', content: userMsg },
    ];

    // 3. 经后端代理流式调用 LLM（Ctrl+C 可中断）
    term.writeln('\x1b[90m(回答中...)\x1b[0m');
    let full = '';
    let saveSeen = false;
    currentAbort = new AbortController();
    try {
      const res = await fetch('/api/llm', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ messages, stream: true }),
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
        full = (body.choices && body.choices[0] && body.choices[0].message && body.choices[0].message.content) || '';
        term.writeln('\x1b[1;32m──── 回答 ────\x1b[0m');
        term.write(renderMdFile(full));
      } else {
        term.writeln('\x1b[1;32m──── 回答 ────\x1b[0m');
        const reader = res.body.getReader();
        const dec = new TextDecoder();
        let buf = '';
        // 流式 markdown：按完整行渲染（保留打字机效果，且行内样式正确）
        const md = createMdRenderer();
        let lineBuf = '';
        let held = []; // 含 <!-- 的行可能是写回块前奏，暂缓显示（避免露出一小段标记）
        const feedDelta = (d) => {
          lineBuf += d;
          let nl;
          while ((nl = lineBuf.indexOf('\n')) !== -1) {
            const line = lineBuf.slice(0, nl);
            lineBuf = lineBuf.slice(nl + 1);
            if (line.includes('<!--')) held.push(line);
            else for (const l of md.feed(line)) term.writeln(l);
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
              const delta = j.choices && j.choices[0] && j.choices[0].delta && j.choices[0].delta.content;
              if (!delta) continue;
              full += delta;
              // 写回块起就不再展示（继续收集用于落盘）
              if (!saveSeen) {
                if (full.includes('<!-- md-agent-save -->')) {
                  saveSeen = true;
                  term.write('\n');
                  continue;
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
        if (lineBuf.length && !saveSeen) {
          for (const l of md.feed(lineBuf)) term.writeln(l);
        }
        for (const l of md.flush()) term.writeln(l);
        // 写回块未触发（如正文含普通 HTML 注释）→ 补显暂缓行
        if (!saveSeen && held.length) {
          for (const line of held) {
            for (const l of md.feed(line)) term.writeln(l);
          }
          for (const l of md.flush()) term.writeln(l);
        }
      }
    } catch (e) {
      if (e && e.name === 'AbortError') {
        term.writeln('\x1b[33m(回答已中断)\x1b[0m');
      } else {
        term.writeln('\x1b[31mLLM 调用失败: ' + e.message + '\x1b[0m');
        term.writeln('\x1b[33m提示: 配置页 http://127.0.0.1:8756/config.html（endpoint/model/api_key）\x1b[0m');
      }
      return;
    } finally {
      currentAbort = null;
    }
    term.writeln('');

    const cleanFull = full.replace(/\n?<!--\s*md-agent-save\s*-->[\s\S]*$/, '').trim();

    // 4. 写回沉淀（解析回答末尾的 md-agent-save 块 → 进待审）
    const save = parseSaveBlock(full);
    if (save) {
      try {
        const savedPath = await applySave(save);
        term.writeln('\x1b[33m💾 已进入待审: \x1b[0m' + savedPath + '  （/view pending 图形审核 · /approve 确认 · /reject 丢弃）');
      } catch (e) {
        term.writeln('\x1b[31m写回失败: ' + e.message + '\x1b[0m');
      }
    }

    // 5. 多轮记忆（存问答正文，不含检索片段；localStorage 持久化）
    history.push({ role: 'user', content: question });
    if (cleanFull) history.push({ role: 'assistant', content: cleanFull });
    if (history.length > MAX_HISTORY) history = history.slice(-MAX_HISTORY);
    saveHistory();

    if (top.length) {
      term.writeln('\x1b[1;32m──── 引用来源 ────\x1b[0m');
      const seen = new Set();
      for (const h of top) {
        const k = h.file + ':' + h.line;
        if (seen.has(k)) continue;
        seen.add(k);
        term.writeln('\x1b[90m' + k + '  ' + (h.section || '') + '\x1b[0m');
      }
    }
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
    term.writeln('\x1b[33m💾 已进入待审: \x1b[0m' + saved + '  （/view pending 图形审核 · /approve 确认 · /reject 丢弃）');
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
    term.writeln('\x1b[33m💾 已进入待审: \x1b[0m' + saved + '  （/view pending 图形审核 · /approve 确认 · /reject 丢弃）');
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
    const kws = extractKeywords(topic);
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
    term.writeln('\x1b[32m✓ 笔记已进入待审: \x1b[0m' + saved + '  （/view pending 图形审核 · /approve 确认 · /reject 丢弃）');
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
        '  ' + (p.kind === 'memory' ? '\x1b[33m[记忆]\x1b[0m' : '\x1b[36m[笔记]\x1b[0m') +
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
  const viewFrame = document.getElementById('view-frame');
  const viewTitle = document.getElementById('view-title');

  // 注入视图的桥脚本：window.hostApi(path, opts) → postMessage 给宿主 → 宿主调 /api/* 后回传；
  // 焦点在 sandbox iframe 内时 Esc 不冒泡出 frame，iframe 内监听并转发给宿主
  const BRIDGE = '<script>' +
    'window.hostApi=function(path,opts){return new Promise(function(res,rej){' +
    'var id=Math.random().toString(36).slice(2);' +
    'function h(ev){if(ev.data&&ev.data.id===id){window.removeEventListener("message",h);ev.data.ok?res(ev.data.data):rej(new Error(ev.data.error||"host error"));}}' +
    'window.addEventListener("message",h);' +
    'window.parent.postMessage({type:"api",id:id,method:(opts&&opts.method)||"GET",path:path,body:opts&&opts.body},"*");' +
    '});};' +
    'window.addEventListener("keydown",function(e){if(e.key==="Escape"){window.parent.postMessage({type:"escape"},"*");}});' +
    '<\/script>';

  function closeView() {
    viewOverlay.classList.add('hidden');
    viewFrame.srcdoc = '';
  }

  function openView(title, html) {
    viewTitle.textContent = title;
    viewFrame.srcdoc = BRIDGE + html;
    viewOverlay.classList.remove('hidden');
    // 焦点移出 xterm textarea：聚焦时 xterm 拦截 Esc（stopPropagation），父页监听收不到
    if (document.activeElement && document.activeElement.blur) document.activeElement.blur();
  }

  document.getElementById('view-close').addEventListener('click', closeView);
  window.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape' && !viewOverlay.classList.contains('hidden')) closeView();
  });

  // postMessage 桥：iframe 视图 → 宿主 API（只允许 /api/ 前缀）；escape 事件关闭视图
  window.addEventListener('message', async (ev) => {
    if (ev.source !== viewFrame.contentWindow) return;
    const msg = ev.data;
    if (!msg) return;
    if (msg.type === 'escape') { closeView(); return; }
    if (msg.type !== 'api') return;
    if (!msg.path || !msg.path.startsWith('/api/')) {
      viewFrame.contentWindow.postMessage({ id: msg.id, ok: false, error: '仅允许 /api/ 接口' }, '*');
      return;
    }
    try {
      const res = await fetch(msg.path, {
        method: msg.method || 'GET',
        headers: { 'Content-Type': 'application/json' },
        body: msg.body ? JSON.stringify(msg.body) : undefined,
      });
      const data = await res.json().catch(() => ({}));
      viewFrame.contentWindow.postMessage({ id: msg.id, ok: res.ok, data }, '*');
    } catch (e) {
      viewFrame.contentWindow.postMessage({ id: msg.id, ok: false, error: String(e) }, '*');
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
    if (arg === 'graph' || arg === 'board' || arg === 'pending' || arg === 'audit') {
      const name = arg + '.html';
      const r = await fetch('/views/' + name);
      if (!r.ok) throw new Error('内置视图加载失败: HTTP ' + r.status);
      const titles = { graph: '知识库结构导航', board: '任务看板', pending: '待审审核', audit: '知识库健康审计' };
      openView(titles[arg], await r.text());
      return;
    }
    const r = await api('/api/file?path=' + encodeURIComponent(arg));
    openView(r.path, r.content);
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

  // LCS 行级 diff（O(n*m)，小文档够用）
  function diffLines(a, b) {
    const n = a.length, m = b.length;
    if (n * m > 4_000_000) {
      // 大文档退化：逐行对比（只标差异）
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
    const d = diffLines(la, lb);
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
      term.writeln('\x1b[33m💾 已进入待审: \x1b[0m' + saved + '  （/view pending 图形审核 · /approve 确认 · /reject 丢弃）');
      return;
    }
    term.writeln('\x1b[90m(补全: LLM 生成「' + topic + '」新文档 → 待审)\x1b[0m');
    const kws = extractKeywords(topic);
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
    term.writeln('\x1b[33m💾 已进入待审: \x1b[0m' + saved + '  （/view pending 图形审核 · /approve 确认 · /reject 丢弃）');
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
