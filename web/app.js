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
  window.addEventListener('resize', () => fit.fit());

  const PROMPT = '\x1b[1;34mmd-agent>\x1b[0m ';
  let line = '';
  let L1_TEXT = ''; // 启动时注入的 L1 层全文
  let history = []; // 多轮对话记忆（不含检索片段，只存问答正文）
  const MAX_HISTORY = 8; // 最近 4 轮

  term.writeln('\x1b[1m本地双层 MD 知识库 Agent\x1b[0m');
  term.writeln('  直接输入问题 → 知识库问答（流式）；/help 查看命令；配置页 /config.html');
  term.write(PROMPT);

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
    if (!line) term.write(PROMPT);
  })();

  term.onData((data) => {
    const code = data.charCodeAt(0);
    if (data === '\r') {
      const cmd = line.trim();
      term.write('\r\n');
      line = '';
      if (cmd) {
        run(cmd)
          .catch((e) => term.writeln('\x1b[31m' + ((e && e.message) || e) + '\x1b[0m'))
          .finally(() => term.write(PROMPT));
      } else {
        term.write(PROMPT);
      }
    } else if (data === '\x7f') {
      if (line.length) {
        line = line.slice(0, -1);
        term.write('\b \b');
      }
    } else if (code >= 32) {
      line += data;
      term.write(data);
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
      case '/clear': history = []; term.writeln('多轮记忆已清空'); break;
      case '/graph': await graph(rest.join(' ')); break;
      case '/orphans': await orphans(); break;
      case '/projects': await projects(); break;
      case '/tags': await tags(); break;
      case '/rescan': await rescan(); break;
      case '/pending': await pendingList(); break;
      case '/approve': await pendingAct('approve', rest[0]); break;
      case '/reject': await pendingAct('reject', rest[0]); break;
      case '/view': await viewCmd(rest[0]); break;
      case '/audit': await auditCmd(); break;
      case '/link': await linkCmd(rest[0], rest[1]); break;
      case '/suggest': await suggest(rest.join(' ')); break;
      case '/health': await health(); break;
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
    term.writeln('  /approve <路径|all>    批准待审 → 写入知识库   /reject 丢弃');
    term.writeln('  /view graph|<html>|off  面板渲染层：内置图谱可视化 / 本地 HTML 视图（Esc 关闭）');
    term.writeln('  /audit                知识库健康审计（盲区/冲突/补链接建议）');
    term.writeln('  /link <源> <目标>      补链接（在源文档追加 [[目标]]，人工确认）');
    term.writeln('  /suggest <主题>        LLM 补全缺失主题的新文档（进待审）');
    term.writeln('  /clear                 清空多轮对话记忆');
    term.writeln('  /config                查看本地配置（掩码）  配置页: /config.html');
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

    // 3. 经后端代理流式调用 LLM
    term.writeln('\x1b[90m(回答中...)\x1b[0m');
    let full = '';
    let saveSeen = false;
    try {
      const res = await fetch('/api/llm', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ messages, stream: true }),
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
      term.writeln('\x1b[31mLLM 调用失败: ' + e.message + '\x1b[0m');
      term.writeln('\x1b[33m提示: 配置页 http://127.0.0.1:8756/config.html（endpoint/model/api_key）\x1b[0m');
      return;
    }
    term.writeln('');

    const cleanFull = full.replace(/\n?<!--\s*md-agent-save\s*-->[\s\S]*$/, '').trim();

    // 4. 写回沉淀（解析回答末尾的 md-agent-save 块 → 进待审）
    const save = parseSaveBlock(full);
    if (save) {
      try {
        const savedPath = await applySave(save);
        term.writeln('\x1b[32m✓ 已进入待审: \x1b[0m' + savedPath + '  （/approve 确认生效，/reject 丢弃）');
      } catch (e) {
        term.writeln('\x1b[31m写回失败: ' + e.message + '\x1b[0m');
      }
    }

    // 5. 多轮记忆（存问答正文，不含检索片段）
    history.push({ role: 'user', content: question });
    if (cleanFull) history.push({ role: 'assistant', content: cleanFull });
    if (history.length > MAX_HISTORY) history = history.slice(-MAX_HISTORY);

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
    term.writeln('\x1b[32m✓ 笔记已进入待审: \x1b[0m' + saved + '  （/approve 确认写入）');
  }

  // ---------- 知识图谱命令 ----------

  async function graph(path) {
    if (!path) {
      term.writeln('\x1b[33m用法：/graph <路径或文件名> —— 显示出链/入链/关联簇\x1b[0m');
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

  // ---------- /view 面板渲染层（iframe 沙箱 + postMessage 桥） ----------

  const viewOverlay = document.getElementById('view-overlay');
  const viewFrame = document.getElementById('view-frame');
  const viewTitle = document.getElementById('view-title');

  // 注入视图的桥脚本：window.hostApi(path, opts) → postMessage 给宿主 → 宿主调 /api/* 后回传
  const BRIDGE = '<script>' +
    'window.hostApi=function(path,opts){return new Promise(function(res,rej){' +
    'var id=Math.random().toString(36).slice(2);' +
    'function h(ev){if(ev.data&&ev.data.id===id){window.removeEventListener("message",h);ev.data.ok?res(ev.data.data):rej(new Error(ev.data.error||"host error"));}}' +
    'window.addEventListener("message",h);' +
    'window.parent.postMessage({type:"api",id:id,method:(opts&&opts.method)||"GET",path:path,body:opts&&opts.body},"*");' +
    '});};' +
    '<\/script>';

  function closeView() {
    viewOverlay.classList.add('hidden');
    viewFrame.srcdoc = '';
  }

  function openView(title, html) {
    viewTitle.textContent = title;
    viewFrame.srcdoc = BRIDGE + html;
    viewOverlay.classList.remove('hidden');
  }

  document.getElementById('view-close').addEventListener('click', closeView);
  window.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape' && !viewOverlay.classList.contains('hidden')) closeView();
  });

  // postMessage 桥：iframe 视图 → 宿主 API（只允许 /api/ 前缀）
  window.addEventListener('message', async (ev) => {
    if (ev.source !== viewFrame.contentWindow) return;
    const msg = ev.data;
    if (!msg || msg.type !== 'api') return;
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
    if (arg === 'graph') {
      const r = await fetch('/views/graph.html');
      if (!r.ok) throw new Error('内置视图加载失败: HTTP ' + r.status);
      openView('知识图谱可视化', await r.text());
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

  // /suggest <主题>：LLM 补全知识库缺失主题的新文档 → 进待审
  async function suggest(topic) {
    if (!topic) {
      term.writeln('\x1b[33m用法：/suggest <主题> —— LLM 生成知识库缺失主题的新文档（进待审）\x1b[0m');
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
    term.writeln('\x1b[32m✓ 已进入待审: \x1b[0m' + saved + '  （/approve 确认写入）');
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
})();
