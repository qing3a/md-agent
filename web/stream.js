// web/stream.js — 终端呈现层（demo 结构重写）：DOM 消息流 + ANSI→HTML + 输入路由
// 替换 xterm：保持 term.writeln/write/clear/focus/onData/attachCustomKeyEventHandler 接口，
// 内部把 ANSI 转成 HTML 行追加到 #stream（#inputblock 之前），输入走 #cmd-input。
// 无 cell 模型/光标数学——行级渲染，提示块由 DOM 输入块承担（app.js 重写）。

(function () {
  'use strict';

  var SGR = { '30': 'c-30', '31': 'c-31', '32': 'c-32', '33': 'c-33', '34': 'c-34', '35': 'c-35', '36': 'c-36', '37': 'c-37', '90': 'c-90' };
  var CSS = [
    '.stream-row{white-space:pre-wrap;word-break:break-word;margin-bottom:6px;font-size:13.5px;line-height:1.75;}',
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
  // SGR 状态累积（真终端语义）：属性/前景各自增量
  function sgrToCls(params, prev) {
    if (!params.length || params[0] === '0' || params[0] === '') return '';
    var attrs = new Set((prev.match(/\b(?:b|d|i|u)\b/g) || []));
    var parts = new Set(params);
    var isTc = params[0] === '48' && params[1] === '2'; // 48;2;R;G;B 的 2 是色彩空间
    if (parts.has('1')) attrs.add('b');
    if (parts.has('2') && !isTc) attrs.add('d');
    if (parts.has('3')) attrs.add('i');
    if (parts.has('4')) attrs.add('u');
    if (parts.has('22')) attrs.delete('b');
    if (parts.has('23')) attrs.delete('i');
    if (parts.has('24')) attrs.delete('u');
    var fg = '';
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
      this._inputBlock = document.getElementById('inputblock');
      this._onDataCbs = [];
      this._keyHandlers = [];
      this._nearBottom = true;
      this.cols = 100;

      // 输入路由：input 事件 → onData；keydown → attachCustomKeyEventHandler（未消费才默认处理）
      this._input.addEventListener('input', () => {
        const v = this._input.value;
        if (v) { this._emit(v); this._input.value = ''; }
      });
      this._input.addEventListener('keydown', (ev) => {
        for (const h of this._keyHandlers) {
          const r = h(ev);
          if (r === false) { ev.preventDefault(); ev.stopPropagation(); return; }
        }
        if (ev.key === 'Enter') { ev.preventDefault(); this._emit('\r'); }
        else if (ev.key === 'Backspace') { ev.preventDefault(); this._emit('\x7f'); }
      });
      // 滚动：近底才自动跟随；输入块随滚动淡出（demo）
      const onScroll = () => {
        const el = this.element;
        this._nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
        if (this._inputBlock) this._inputBlock.style.opacity = this._nearBottom ? '1' : '0.25';
      };
      this.element.addEventListener('scroll', onScroll);
      window.addEventListener('resize', () => {
        this.cols = Math.max(40, Math.floor((this.element.clientWidth - 32) / 8));
      });
      this.cols = Math.max(40, Math.floor((this.element.clientWidth - 32) / 8));
      this._measureTimer = null;
    }

    // ---- 输入路由 ----
    onData(cb) { this._onDataCbs.push(cb); return cb; }
    attachCustomKeyEventHandler(h) { this._keyHandlers.push(h); return () => {}; }
    _emit(data) { this._onDataCbs.forEach((cb) => cb(data)); }
    focus() { this._input.focus({ preventScroll: true }); }

    // ---- 输出（行级 ANSI→HTML） ----
    write(text) {
      const t = String(text).replace(/\r/g, '');
      if (!t) return;
      const lines = t.split('\n');
      for (const l of lines) this._appendRow(ansiToHtml(l));
    }
    writeln(text) { this.write(text); }
    clear() {
      // 清空消息行（保留输入块）
      let el = this.element.firstChild;
      while (el && el !== this._inputBlock) {
        const next = el.nextSibling;
        this.element.removeChild(el);
        el = next;
      }
      this.element.scrollTop = 0;
    }
    scrollToBottom() { this.element.scrollTop = this.element.scrollHeight; this._nearBottom = true; }

    // ---- DOM 行 ----
    _appendRow(html) {
      const d = document.createElement('div');
      d.className = 'stream-row';
      d.innerHTML = html;
      this.element.insertBefore(d, this._inputBlock);
      if (this._nearBottom) this.element.scrollTop = this.element.scrollHeight;
      return d;
    }
    // 富内容行（工具卡/欢迎横幅/用户消息区块）：直接 appendCard 到流（在输入块之前）
    appendCard(html, cls) {
      const d = document.createElement('div');
      d.className = 'row ' + (cls || '');
      d.innerHTML = html;
      this.element.insertBefore(d, this._inputBlock);
      if (this._nearBottom) this.element.scrollTop = this.element.scrollHeight;
      return d;
    }
    // 输入块状态行
    setStatus(text) {
      const el = document.getElementById('statusline');
      if (el) el.textContent = text;
    }
  }

  window.StreamTerm = StreamTerm;
})();
