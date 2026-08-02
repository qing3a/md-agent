## 终端壳 DOM/CSS 化 + UI 增强（补全/历史/按钮/图标）实施计划

**目标**：把输入框/状态栏迁出终端流改为 DOM，新增命令补全面板、输入历史、快捷按钮、状态图标，为后续更多 UI 打基础。终端内核 xterm.js 不动，只重构 chrome（业界标准：UI 放 DOM、终端流只放内容）。

**默认产品决策**（AskUserQuestion 未答，取推荐项，可随时改）：
- 消息块：保留流内 ANSI 整行背景块（现状视觉，resize 拉窄小瑕疵接受）
- 输入框：textarea 单行视觉，Enter 提交、Shift+Enter 换行，IME/光标浏览器原生
- 回答期间：输入框可编辑但禁提交（防串流），回答结束自动恢复
- 快捷按钮默认组：/sync /rescan /pending /digest /clear /help

### 布局（web/index.html，现 53 行全量重写 body）
```
<body>
  <div id="app" class="shell">            <!-- flex column, height:100% -->
    <div id="terminal"></div>             <!-- flex:1 min-height:0，xterm 挂载，去掉 10px padding -->
    <div id="input-bar">                  <!-- 快捷按钮行 + 输入条 -->
      <div id="quick-btns">6 个按钮</div>
      <div id="input-wrap">               <!-- position:relative -->
        <span id="prompt-tag">❯</span>
        <textarea id="cmd-input" rows="1" spellcheck="false"></textarea>
        <div id="autocomplete" class="hidden"></div>  <!-- absolute 悬浮输入框上方 -->
      </div>
    </div>
    <div id="status-bar"><span id="status-icons"></span><span id="status-text"></span></div>
  </div>
  <div id="view-overlay" class="hidden">…现状不变…</div>
</body>
```
CSS：`.shell{display:flex;flex-direction:column;height:100%}`；`#terminal` 去 padding（fit 在父级测宽的坑，见调研 #1283）；输入/状态条固定行不随终端滚动；`#autocomplete{position:absolute;bottom:100%;z-index:50}`；状态文本 `white-space:nowrap;ellipsis`；主题延续 #1e1e2e/#313244。

### app.js 重构（web/app.js，1560 行）
**状态变量**：删 `line`（input.value 取代）、`statusLine`（DOM 取代）、`atPrompt`（改 `busy`=回答进行中）；保留 `trueCols()`（消息块铺背景用）。

**输入链路**（替换 onData 输入分支）：
- `term.onData` 只保留 confirmCb 分支（y/n/Esc）；confirm 时 `term.focus()` 收键盘，结束焦点回输入框
- `cmd-input` 事件：keydown——Enter(无Shift)提交（busy 忽略）、Shift+Enter 换行、ArrowUp/Down 翻输入历史（补全打开时先选补全项）、Tab 确认补全、Esc 关补全面板、Ctrl+C 空输入时中断当前回答（fetch abort，新能力）；input——触发补全过滤 + textarea 自适应高度(1~3行)

**提交**（submitMsg 重构）：删清边框逻辑；写流内消息块 `\x1b[48;2;49;50;68m + ' '.repeat(trueCols()) + '\r' + PROMPT + text + '\x1b[0m\r\n'`；`busy=true` → 清空输入框 → `run(text)` → `.finally{busy=false; 焦点回输入框; refreshStatus()}`

**状态条 DOM 化**（refreshStatus 重构）：fetch 逻辑不变（health/config/pending/tasks/graph/heartbeat/audit），改为更新 `#status-icons`/`#status-text` DOM；删 `drawStatusRow`（光标舞蹈）、`statusLine`；8s 轮询保留

**输入历史**：`history[]`+指针，提交入队（去重、上限 100），ArrowUp/Down 翻动

**命令补全面板**：内置命令表（/help /search /sync /rescan /pending /preview /approve /reject /digest /remember /graph /orphans /projects /tags /audit /conflicts /diff /link /link-all /suggest /fetch /page /task /view /config /health /heartbeat /clear 等）；`/` 开头时过滤，渲染 ≤8 项高亮选中；键盘上下/Tab/Enter/Esc + 鼠标点击；选中填入输入框（保留 rest）

**快捷按钮**：6 个按钮点击=直接执行（写消息块+run）；样式小圆角 hover 高亮

**状态图标**：DOM span 显示 ●服务(绿/红)、模型、KB、待审、任务、图谱文档/链接、心跳开关、⚠审计警告（信息与现状 statusLine 一致）

### 不动的东西
`run()`/`ask()`/全部命令 handler/`createMdRenderer`（markdown→ANSI）/`printBanner`/`/view` iframe overlay/回答流式输出/引用来源块/`<!-- md-agent-save -->` 写回抑制。

### 实施步骤
1. index.html 新布局+CSS（骨架元素）
2. 输入链路迁移（keydown/input 事件、Enter 提交、confirm 焦点、busy 状态）
3. submitMsg 重构+消息块/回答链路验证（含回答期间禁提交）
4. 状态条 DOM 化
5. 输入历史
6. 命令补全面板
7. 快捷按钮
8. 状态图标
9. 清理死代码（dispW/visOnly/truncateW/showPrompt/hline/超长保护——检查引用后删）
10. README 更新

### 验证
中文 IME（输入/候选/回车上屏）；resize 各宽度（输入/状态/补全面板零变形，消息块小瑕疵确认可接受）；回答期间可输入禁提交、结束恢复；历史翻页/补全键盘鼠标；confirm 流程（/approve y/n）；/view 桥；回答中途 Ctrl+C 中断。