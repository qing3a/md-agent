// web/stream.js — 终端呈现层（DeepSeek 网页版结构）：DOM 消息流 + ANSI→HTML + 原生 textarea 输入路由
// 保持 term.writeln/write/clear/focus/onData/attachCustomKeyEventHandler 接口；
// 消息按「气泡容器」组织：beginMsg() 后写入进入当前容器（.msg.user / .msg.assistant / .msg.tool），
// 无容器时写入 #stream 根部（系统行/横幅）。无 cell 模型/光标数学——行级渲染，输入走 #cmd-input。

(function () {
  'use strict';

  var SGR = { '30': 'c-30', '31': 'c-31', '32': 'c-32', '33': 'c-33', '34': 'c-34', '35': 'c-35', '36': 'c-36', '37': 'c-37', '90': 'c-90' };
  var CSS = [
    '.stream-row{white-space:pre-wrap;word-break:break-word;margin-bottom:6px;font-size:13.5px;line-height:1.75;}',
    '.stream-row.think{color:#7f849c;font-size:12.5px;}',
    '.stream-row .c-30{color:#4c4f69}.stream-row .c-31{color:#f38ba8}.stream-row .c-32{color:#a6e3a1}',
    '.stream-row .c-33{color:#f9e2af}.stream-row .c-34{color:#89b4fa}.stream-row .c-35{color:#cba6f7}',
    '.stream-row .c-36{color:#94e2d5}.stream-row .c-37{color:#cdd6f4}.stream-row .c-90{color:#7f849c}',
    '.stream-row .b{font-weight:700}.stream-row .d{opacity:.7}.stream-row .i{font-style:italic}.stream-row .u{text-decoration:underline}',
  ].join('');

  function injectCss() {
    if (document.getElementById('stream-css')) return;
    var st = document.createElement('style');
    st.id = 'stream-css';
    st.textContent = CSS;
    document.head.appendChild(st);
  }
  function esc(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }
  // SGR 状态累积（真终端语义）：属性/前景各自增量；\x1b[0;32m 这类"重置+上色"组合正确生效（E7）
  function sgrToCls(params, prev) {
    if (!params.length) return '';
    var attrs = new Set((prev.match(/\b(?:b|d|i|u)\b/g) || []));
    var parts = new Set(params);
    var isTc = params[0] === '48' && params[1] === '2'; // 48;2;R;G;B 的 2 是色彩空间
    var fg = '';
    if (parts.has('0')) attrs = new Set(); // 重置属性（fg 本就按本轮参数重算）
    if (parts.has('1')) attrs.add('b');
    if (parts.has('2') && !isTc) attrs.add('d');
    if (parts.has('3')) attrs.add('i');
    if (parts.has('4')) attrs.add('u');
    if (parts.has('22')) attrs.delete('b');
    if (parts.has('23')) attrs.delete('i');
    if (parts.has('24')) attrs.delete('u');
    for (var i = 0; i < params.length; i++) if (SGR[params[i]]) fg = SGR[params[i]];
    var cls = Array.from(attrs);
    if (fg) cls.push(fg);
    return cls.join(' ');
  }
  // ANSI → HTML（按 SGR 切段，行级）
  function ansiToHtml(t) {
    var segs = [];
    var cls = '';
    var re = /\x1b\[([0-9;]*)m/g;
    var last = 0;
    var m;
    while ((m = re.exec(t))) {
      if (m.index > last) segs.push([t.slice(last, m.index), cls]);
      cls = sgrToCls(m[1] ? m[1].split(';') : [], cls);
      last = re.lastIndex;
    }
    if (last < t.length) segs.push([t.slice(last), cls]);
    return segs.map(function (s) {
      var txt = esc(s[0].replace(/[\r\n]+/g, ''));
      return txt ? (s[1] ? '<span class="' + s[1] + '">' + txt + '</span>' : txt) : '';
    }).join('');
  }

  class StreamTerm {
    constructor() {
      injectCss();
      this.element = document.getElementById('stream');
      this._input = document.getElementById('cmd-input');
      this._current = null;          // 当前消息容器（气泡）；null = 写 #stream 根部
      this._onDataCbs = [];
      this._keyHandlers = [];
      this._nearBottom = true;
      this.cols = 100; // 兼容遗留（app.js trueCols() 自行按容器宽度计算）

      // 原生 textarea 编辑：Enter(无 Shift)=提交；Tab 永远拦截（补全/焦点保护）；
      // Shift+Enter 换行 / 退格 / 光标移动 = 原生行为（app.js 在 input 事件同步 line）
      this._input.addEventListener('keydown', (ev) => {
        for (const h of this._keyHandlers) {
          const r = h(ev);
          if (r === false) { ev.preventDefault(); ev.stopPropagation(); return; }
        }
        if (ev.key === 'Enter' && !ev.shiftKey) { ev.preventDefault(); this._emit('\r'); }
        else if (ev.key === 'Tab') ev.preventDefault();
      });
      // 输入即自动增高（1→6 行封顶）
      this._input.addEventListener('input', () => { this.autogrow(); });
      const onScroll = () => {
        this._nearBottom = this.element.scrollHeight - this.element.scrollTop - this.element.clientHeight < 60;
      };
      this.element.addEventListener('scroll', onScroll);
      this.autogrow();
    }

    // ---- 输入路由 ----
    onData(cb) { this._onDataCbs.push(cb); return cb; }
    attachCustomKeyEventHandler(h) { this._keyHandlers.push(h); return () => {}; }
    _emit(data) { this._onDataCbs.forEach((cb) => cb(data)); }
    focus() { this._input.focus({ preventScroll: true }); }
    autogrow() {
      const el = this._input;
      el.style.height = 'auto';
      el.style.height = Math.min(200, el.scrollHeight) + 'px';
    }

    // ---- 消息容器（气泡） ----
    beginMsg(cls) {
      this._current = document.createElement('div');
      this._current.className = 'msg ' + (cls || '');
      this.element.appendChild(this._current);
      if (this._nearBottom) this.element.scrollTop = this.element.scrollHeight;
      return this._current;
    }
    endMsg(removeIfEmpty) {
      if (this._current) {
        if (removeIfEmpty && !this._current.childNodes.length) this._current.remove();
        this._current = null;
      }
    }
    currentMsg() { return this._current; }

    // ---- 输出（行级 ANSI→HTML） ----
    write(text) {
      const t = String(text).replace(/\r/g, '');
      if (!t) return;
      const lines = t.split('\n');
      for (const l of lines) this._appendRow(ansiToHtml(l));
    }
    writeln(text) { this.write(text); }
    clear() {
      // 清空全部消息（输入条在 #stream 外，不动）
      this._current = null;
      this.element.innerHTML = '';
      this.element.scrollTop = 0;
    }
    scrollToBottom() { this.element.scrollTop = this.element.scrollHeight; this._nearBottom = true; }

    // ---- DOM 行 ----
    _appendRow(html) {
      const d = document.createElement('div');
      d.className = 'stream-row';
      d.innerHTML = html;
      (this._current || this.element).appendChild(d);
      if (this._nearBottom) this.element.scrollTop = this.element.scrollHeight;
      return d;
    }
    // 纯文本行（思考中指示等）
    appendRow(text, cls) {
      const d = document.createElement('div');
      d.className = 'stream-row' + (cls ? ' ' + cls : '');
      d.textContent = text;
      (this._current || this.element).appendChild(d);
      if (this._nearBottom) this.element.scrollTop = this.element.scrollHeight;
      return d;
    }
    // 富内容行（工具卡/欢迎横幅/用户消息区块）
    appendCard(html, cls) {
      const d = document.createElement('div');
      d.className = 'row ' + (cls || '');
      d.innerHTML = html;
      (this._current || this.element).appendChild(d);
      if (this._nearBottom) this.element.scrollTop = this.element.scrollHeight;
      return d;
    }
    removeRow(el) {
      if (el && el.parentNode) el.parentNode.removeChild(el);
    }
    // 用富 HTML 行替换现有行（「思考中…」→ 深度思考折叠块）
    replaceRow(el, html, cls) {
      const d = document.createElement('div');
      d.className = 'row ' + (cls || '');
      d.innerHTML = html;
      if (el && el.parentNode) el.parentNode.replaceChild(d, el);
      else (this._current || this.element).appendChild(d);
      if (this._nearBottom) this.element.scrollTop = this.element.scrollHeight;
      return d;
    }
    // 输入条状态行
    setStatus(text) {
      const el = document.getElementById('statusline');
      if (el) el.textContent = text;
    }
  }

  window.StreamTerm = StreamTerm;
})();
