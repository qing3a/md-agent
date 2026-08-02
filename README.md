# md-agent — 本地 Agent Harness：MD 持久记忆 + 工具链 + 人审闭环

本地常驻的 Agent Harness：托盘常驻底座 + 网页终端交互 + LLM 代理 + 检索/图谱/网页/任务工具链；记忆与写回以 Markdown 纯文本落盘，可审计、可追溯、永不锁库。

> 一句话定位：**知识人写人读，AI 只负责整理、关联、推演、生成。**

> 本质：本系统是 Agent 的运行层（harness）——底座 + 终端 + LLM 代理 + 工具链；其中 **双层 MD 记忆是核心子系统**：正式记忆（人工审核固化）+ 待审草稿（Agent 候选，必经人审）+ 自组织整理器。它把 Agent 的长期记忆固化为纯文本文件，突破单次上下文窗口的容量限制；检索只是记忆的读取手段，知识正文永远是 Markdown。

## 核心哲学

1. **自组织而非自进化**：系统只负责组织 Agent 已有的资源——记忆（链接/去重/索引/巩固）、行为（技能/上下文组装）、评估（什么用得好）；不声称提升模型能力——能力来自模型训练与检索获取。
2. **记忆即文件**：原始知识永远是 `.md`，可用任意编辑器打开/修改/同步，永不锁库、永不黑盒；文件系统即持久记忆。
3. **不用向量，用显性人类知识结构做关联**：`[[双向链接]]` + Frontmatter 元数据 + 目录项目层级。关联可审计、可追溯。
4. **双检索互补（不是碾压，是各管一段）**：
   - **ripgrep 全文检索** → 找片段、找关键词（已实现）
   - **SQLite 结构化图谱检索** → 找关联、找脉络、找项目体系（已实现）

## 当前能力（Phase 1–3 已完成 ✅）

| 能力 | 说明 |
|---|---|
| 双层 MD 持久记忆 | 记忆子系统：L1 规范/记忆/索引层（CLAUDE.md 模式，启动注入）+ L2 内容层（grep 检索）；`INDEX.md` 自动生成；写回一律走待审，人工审核固化 |
| 全文检索 | 内嵌 ripgrep 内核（grep + ignore crate），多关键词任一命中、智能大小写、小节上下文（`section`/`context`） |
| **知识图谱** | SQLite `documents`/`links` 两表：`[[双向链接]]` 解析、反向链接、孤立文档检测、标签/项目维度统计；首次调用自动建库，`/rescan`、托盘「立即同步」或心跳自动重建 |
| **伪命令行 Markdown 渲染** | 终端内 ANSI 富渲染（零依赖）：标题加粗、行内代码/加粗/链接/`[[双链]]` 着色、列表/引用/代码围栏；**表格按 markdown 行显示**（保留 `|` 结构，复制不失真）；frontmatter 变暗；流式回答按完整行渲染 |
| **/view 面板渲染层** | iframe 沙箱 + postMessage 桥（视图经宿主调 `/api/*`，仅允许 api 前缀，真机验证通过）：`/view graph` **知识库结构导航**（目录层级缩进树 + 项目色点 + 孤立/悬空标记，点击文档右侧显示出链/入链并可跳转——全图环形布局发散已弃用）、`/view board` 任务看板、`/view pending` 待审审核面板、`/view audit` 审计面板、`/view <html>` 渲染 kb 内本地 HTML、`/view off` 或 Esc 关闭；**多标签页并存**（各视图可同时打开、点击标签切换、标签 × / Esc 关单个、`/view off` 全关）；**视图健壮性**（10s 未加载 tab 标黄、沙箱脚本错误上报宿主标红 + 终端提示、桥请求 20s 超时兜底） |
| **心跳自动同步（自组织自动发现）** | 默认关闭；开启后每 60s（可调）指纹比对知识库（路径+mtime+大小，排除 pending/），变化自动重建 INDEX+图谱并跑本地审计，状态行提示「心跳开 + ⚠审计发现」；托盘勾选 / `/heartbeat` / 配置页三入口；`sync_lock` 与手动写端点防并发 |
| **终端壳体验** | 启动欢迎横幅 + 状态汇总（版本/KB/图谱/模型/待审/进行中任务）；**输入框回流内**——4 行结构贴内容末尾（上边框/输入行/下边框/状态行），resize 自动重画；↑↓ 输入历史、Tab 命令循环补全（`/` 命令 + `@` 文件提及）、Ctrl+C 中断、**Esc 停止**、**Ctrl+K 速览**均经 `attachCustomKeyEventHandler`；**推理思考折叠**（流式 `reasoning_content` → 推理期灰色「🧠 思考中…」→ 首个内容到达时清除并出「──── 回答 ────」标题，回答后「Thought · N 秒」折叠行）；**本次回答 token 用量**（`stream_options.include_usage`，引用来源前）；**@ 文件提及**（`@xxx`+Tab 补全 KB 文档路径，提交时指定文档全文注入检索目标）；**输入草稿 + 命令历史 localStorage 持久化**（刷新恢复未提交输入，上限 100）；**速览侧边栏**（`/side` / 快捷按钮 / Ctrl+K 唤出左侧抽屉：任务/待审/图谱/审计速览，点卡片直达对应面板）；快捷按钮行（同步/图谱/待审/整理/**速览**/清记忆/帮助，DOM）；状态行（● 服务/模型/KB/待审/任务/图谱/心跳 + ⚠审计警告，8s 轮询）；提交消息块保留流内整行背景色；回答期间输入框可编辑但禁提交 |
| **审计面板** | `/view audit`：审计结果卡片化——补链建议一键 [应用]、悬空/孤立/重复分组展示（`/audit` 终端版保留） |
| **记忆自组织（Phase 3-A 基础）** | `/audit` 本地规则健康审计（孤立/无出链/重复标题/悬空链接/提及未链接建议，零 LLM 快速确定）；`/link` 人工补链接（文件名双链、去重、自动重建图谱）；`/link-all` 一键应用建议；`/suggest` 补全缺失主题（带主题名）或**无参盲区模式**（先审计后让 LLM 分析知识盲区生成新文档，进待审）；`/diff`/`/conflicts` 行级对比与冲突检查 |
| **待审行级预览** | `/preview <待审路径>` 只读展示批准后将写入的内容（记忆条目按当日小节合并规则计算，不落盘） |
| **网页读取与操作** | `/fetch <url> [标题]` 静态抓取；`/page <url> [标题]` 动态读取（headless 等 JS 渲染）；`/page act <url> <json 动作数组>` **写侧**（click/fill/select/scroll，动作清单**人工确认后执行**，返回页面结果） |
| **任务引擎（Phase 3-B）** | `kb/.tasks.db` 独立 SQLite：目标/状态机（待办·进行中·完成·放弃）/依赖/推进日志；`/task` 终端文字看板 + `/task board` HTML 看板；**依赖就绪校验**（进入进行中/完成时依赖必须已完成）；`/task plan <目标>` LLM 拆解串行子任务链 |
| LLM 代理 | OpenAI 兼容（Ollama/DeepSeek 等），后端代理防 CORS 与密钥暴露；**SSE 流式透传** |
| Agent 问答回路 | 启动注入 L1 → 提取关键词 → 检索 L2 → 拼 Prompt → 流式回答 → `[文件:行号]` 引用（检索词当前为前端启发式提取；LLM 显式 Tool Use 见路线图 Phase 3-C P1） |
| 多轮对话记忆 | 会话内保留最近 4 轮，**localStorage 持久化**（刷新页面不丢，`/clear` 清空） |
| 写回沉淀 | LLM 回答附 `<!-- md-agent-save -->` 块自动落盘：新知识写 L2（自动补 frontmatter）、决策写 L1 MEMORY；`/remember` 手动沉淀 |
| **待审机制** | LLM 生成的新笔记/记忆条目先进 `pending/`（不直接污染知识库）：`/view pending` **图形审核面板**（三栏：待审清单批量勾选 / 目标文档上下文+绿色 diff / 可编辑内容；支持**编辑后批准**与批量批准/拒绝）；终端 `/pending` `/approve` `/reject` 保留；落地自动重建 INDEX+图谱；待审文件不进检索与图谱 |
| `/digest` | 检索结果交给 LLM 整理成结构化笔记写入 `notes/` |
| 可视化配置页 | `/config.html`：endpoint / model / api_key（掩码显示）+ 测试连接 |
| 托盘常驻 | tray-icon + winit，右键菜单：打开终端 / **心跳同步（可勾选开关）** / 立即同步 / 退出；release 单 exe 隐藏控制台 |

## 架构（四层）

```
常驻底座层    Rust 托盘常驻 + Axum 本地服务（127.0.0.1，同源托管前端免 CORS）
记忆内核层    双层 MD 记忆布局（L1 规范/记忆/索引 + L2 内容）/ frontmatter 解析 / INDEX 自动生成 / 待审机制 / 路径安全
工具链层      检索（ignore + grep crate）、图谱（SQLite 双链）、网页（/fetch /page act）、任务（tasks.db）——宿主 API 统一鉴权，LLM 不直连
交互层        xterm.js 网页终端（Agent 回路 + 管理命令 + 配置页）+ /view iframe 面板渲染层（App 系统地基）
```

## 双层记忆结构

| 层 | 内容 | 记忆角色 | 进入上下文方式 |
|---|---|---|---|
| L1（kb 根目录） | `KB.md` / `FRAMEWORK.md` / `RULES.md` / `MEMORY.md` / `INDEX.md` —— 规范、记忆、索引 | 引导记忆（bootstrap memory，类 CLAUDE.md） | 启动时注入 Agent（常驻，只放"位置+要点"，正文进 L2） |
| L2（`kb/notes/`） | 知识正文 | 内容记忆（retrievable memory） | 按需检索（grep），命中片段注入 Prompt |

## 使用

### 运行

```bash
cargo run                # 托盘模式（tray-icon + winit）
cargo run -- --no-tray   # 纯服务模式（调试用）
cargo run -- --port 9000 # 自定义端口（默认 8756）
```

环境变量：`MD_AGENT_KB`（KB 根目录）、`MD_AGENT_PORT`、`MD_AGENT_CONFIG`（config.json 路径）、`MD_AGENT_NO_TRAY`（=1 等同 --no-tray）。

### 配置 LLM

浏览器打开 `http://127.0.0.1:8756/config.html` 可视化配置，或：

```bash
curl -X POST http://127.0.0.1:8756/api/config -H "Content-Type: application/json" \
  --data-binary @- <<'JSON'
{"llm":{"endpoint":"https://api.deepseek.com/v1","model":"deepseek-v4-flash","api_key":"sk-xxx"}}
JSON
```

- `endpoint`：Ollama 基址（`http://127.0.0.1:11434`）或 OpenAI 兼容基址（`https://api.deepseek.com/v1`），代理自动补 `/v1/chat/completions`
- `api_key` 以掩码显示（`sk-****1249`）；POST 传 `****` 保留旧 key
- 浏览器不直连 LLM，一律经 `/api/llm` 后端代理
- config.json 另有 `heartbeat` 字段（`{enabled: false, interval_secs: 60}`），旧配置缺省自动兼容

### 终端命令

直接输入问题走 Agent 问答；`/help` 查看全部：

```
/search <关键词>      检索双层库（显示所属小节）
open <路径>           查看 KB 内 MD
/l1                   查看 L1 规范/记忆/索引层
/sync                 重建 INDEX.md
/digest <主题>        检索并把结果整理成新笔记写入 notes/
/remember [路径] 内容  手动沉淀（默认 MEMORY.md）
/graph <路径>         知识图谱：出链/入链/关联簇
/orphans              孤立文档（无入链也无出链）
/projects             项目维度统计    /tags 标签统计
/rescan               重建知识图谱（SQLite）
/pending              查看待审（LLM 写回/生成笔记先进这里）
/preview <待审路径>    行级预览：批准后将写入的内容（只读）
/approve <路径|all>    批准待审 → 写入知识库（自动重建 INDEX+图谱）
/reject <路径|all>     丢弃待审
/view graph|board|pending|audit|<html>|off  面板渲染层（多标签并存，Esc 关闭当前）
/side                速览侧边栏（任务/待审/图谱/审计，Ctrl+K 或快捷按钮同样唤出）
/audit                知识库健康审计（盲区/冲突/补链接建议）
/conflicts            冲突检查（重复标题/悬空链接）   /diff <A> <B> 行级对比
/link <源> <目标>      补链接（在源文档追加 [[目标]]，人工确认）
/link-all              一键应用 /audit 的全部补链接建议
/suggest [主题]        LLM 补全缺失主题（无参 = 盲区分析模式，均进待审）
/fetch <url> [标题]    静态抓取网页：阅读视图 / 带标题则沉淀为待审笔记
/page <url> [标题]     动态网页读取（headless Edge/Chrome，等 JS 渲染）
/task                  任务看板：new/start/done/drop/note/dep/rm/plan/board
/clear                清空多轮对话记忆
/config               查看配置（掩码）
/heartbeat [on|off|interval <秒>|status]  心跳自动同步开关/周期/状态（变化自动重建+审计提示）
/health               服务健康检查
```
`/page act <url> <json 动作数组>`：动态页**写侧**——click/fill/select/scroll，动作清单打印后 `y/N` 人工确认才执行（例：`/page act https://example.com [{"kind":"fill","selector":"#q","value":"hello"},{"kind":"click","selector":"#btn"}]`）。

### 打包发布

```bash
cargo build --release
mkdir -p dist && cp target/release/md-agent.exe dist/
cp -r web dist/web && cp -r kb dist/kb
cp config.json dist/config.json   # 可选：携带已有 LLM 配置
```

`dist/` 即成品目录，双击 `md-agent.exe` 常驻托盘。首次运行若 kb 缺失会自动用内嵌模板重建。

## 接口

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/health` | 健康检查 |
| GET | `/api/search?q=&layer=&ctx=` | 检索（layer: `all`/`notes`/`l1`；`ctx=1` 附小节标题与上下文） |
| GET | `/api/l1?full=` | L1 层文件（`full=1` 附完整内容，Agent 启动注入用） |
| GET/POST | `/api/file` | 读/写 KB 内 MD（路径越权防护） |
| POST | `/api/kb/sync` | 重建 INDEX.md |
| GET | `/api/kb/pending` | 待审文件列表 |
| GET | `/api/kb/pending/preview?path=` | 待审行级预览（记忆条目按合并规则计算，只读） |
| POST | `/api/kb/pending/approve` | 批准待审（body: `{path}` 或 `all`，可选 `content` 覆盖内容=编辑后批准）→ 落地 + 重建 INDEX/图谱 |
| POST | `/api/kb/pending/reject` | 丢弃待审（body: `{path}` 或 `all`） |
| POST | `/api/graph/sync` | 重建知识图谱（SQLite） |
| GET | `/api/graph/stats` | 图谱统计（文档/链接/解析/悬空/孤立/项目） |
| GET | `/api/graph/graph` | 全量图谱数据（nodes 含度数 / edges，供可视化视图） |
| GET | `/api/graph/backlinks?path=` | 反向链接（谁链向该文档） |
| GET | `/api/graph/linked?path=` | 出链（该文档链向谁，含悬空标记） |
| GET | `/api/graph/related?path=` | 关联簇（出链+入链去重） |
| GET | `/api/graph/orphans` | 孤立文档（无入链也无出链） |
| GET | `/api/graph/tags` | 标签统计 |
| GET | `/api/graph/projects` | 项目维度统计 |
| GET | `/api/heartbeat` | 心跳状态（开关/周期/上次同步/审计摘要） |
| POST | `/api/heartbeat` | 改心跳配置（body: `{enabled?, interval_secs?}`，落盘） |
| GET | `/api/audit` | 知识库健康审计（孤立/无出链/重复标题/悬空/提及未链接建议） |
| POST | `/api/link` | 补链接（body: `{src, dst}`；文件名双链 + 去重 + 重建图谱） |
| GET | `/api/fetch?url=` | 静态网页抓取（HTTP + HTML 文本提取，零浏览器依赖） |
| GET | `/api/page?url=` | 动态网页读取（chromiumoxide + 系统 Edge/Chrome headless） |
| POST | `/api/page/act` | 动作执行（body: `{url, actions: [{kind: click\|fill\|select\|scroll, selector, value?}]}`；前端人审确认后调用） |
| GET | `/api/tasks` | 任务列表 + 看板统计（`kb/.tasks.db` 独立库） |
| POST | `/api/tasks` | 新建任务（body: `{goal, title?}`） |
| PATCH | `/api/tasks/{id}` | 任务更新（`{status?, note?, deps?}`，note 追加带时间戳日志） |
| DELETE | `/api/tasks/{id}` | 删除任务 |
| GET/POST | `/api/config` | 本地配置（GET 掩码 api_key） |
| POST | `/api/llm` | LLM 代理（`stream=true` 走 SSE 流式，否则 JSON 透传） |

## 迭代路线

```
Phase 1  ✅ 已完成（托盘 + Axum + xterm + ripgrep + LLM 代理 + Agent 回路 + 写回 + 流式/多轮/digest/配置页）
Phase 2  ✅ 已完成（知识图谱）
         ├─ SQLite 元数据图谱（documents / links 两表，kb/.md-graph.db）
         ├─ [[双向链接]] 解析 + frontmatter 入库（标题/标签/摘要/更新时间）
         ├─ 反向链接 / 孤立文档 / 标签 / 项目维度检索（/api/graph/*）
         ├─ 多项目隔离（project = 相对 kb 根的第一段目录）
         └─ /digest 升级：沿知识链路（关联簇）生成体系化文案，输出自动带 [[链接]]
Phase 3  ▶ 自组织工作流（双主线，人审闭环贯穿）
Phase 3-A 记忆自组织（✅ 已完成基础 + 深化）
         ├─ ✅ /audit 本地规则审计（孤立/无出链/重复标题/悬空链接/提及未链接建议；自动生成的 INDEX.md 不参与建议）
         ├─ ✅ /link 人工补链接（文件名双链、去重、自动重建图谱）+ /link-all 一键应用建议
         ├─ ✅ /suggest 补全缺失主题（带主题名 / 无参盲区分析两种模式，进待审通道）
         ├─ ✅ /diff 行级对比（LCS，大文件自动降级）+ /conflicts 冲突检查（重复标题/悬空链接）
         ├─ ✅ 知识生成走「生成 → 预览（/preview 行级）→ /approve 确认」待审通道，防 LLM 污染知识库
         └─ 延伸：Agent 可动态生成 HTML 面板 App（人审通过后安装）——自组织的应用层表现
Phase 3-B 规划引擎（✅ 基础已实现）
         ├─ ✅ 任务实体持久化：目标/状态机/依赖/推进日志入 SQLite（kb/.tasks.db 独立库，避开 graph 全量重建）
         ├─ ✅ /task 终端看板：new / start / done / drop / note / dep / rm
         ├─ ✅ 规划可视化：/task board（/view board）HTML 看板（四列泳道 + 拖按钮流转）
         ├─ ✅ 依赖就绪校验（进入 doing/done 前依赖必须完成，后端拒绝并提示）
         ├─ ✅ /task plan LLM 目标拆解（串行子任务链，依赖联动）
         └─ ▶ 深化：动态重规划；执行约束延续人审闭环
Phase 3-C Harness 深化（▶ 待开工——工具 → 记忆 → 程序性自组织 → 生态）
         ├─ ▶ P1 工具注册表 + Agent Loop：GET /api/tools 声明式工具清单（name/desc/params）；LLM 决策调工具 → 宿主执行 → 结果回填，Agent 回路从「启发式关键词 → grep」升级为 LLM 显式 Tool Use
         ├─ ▶ P2 记忆生命周期：任务感知上下文组装（L1 常驻 → 按任务动态注入）+ 巩固/遗忘（自动压缩与降级，走待审通道）
         ├─ ▶ P3 Skills / 程序性自组织：/suggest、/audit 产出从知识笔记升级为程序性技能（技能格式 + 注册表 + 自动触发）
         └─ ▶ P4 App 系统（原 Phase 4 提前）：manifest + 安装/启停 + 权限声明 + 人审安装（/view 渲染层为地基；本地市场最后）
Phase 4  生态化（可选，与"轻量"定位有张力，个人场景可长期搁置）
         ├─ MCP 客户端（Stdio/SSE）；兼容标准 MCP App 渲染（复用统一 iframe 渲染层）
         └─ WASM 计算后端（仅当出现"本地运行不可信计算"的真实需求）
```

**设计决策**（含 2026-08 对「Host App / WASM 插件」方案评审结论）：

- **概念界定：App ≠ MCP App**。本项目的"应用"= 一体化应用包（HTML 界面 + 业务逻辑，可安装/启用/运行/关闭/卸载、可上架插件市场）；MCP App 只是依附外部 MCP 进程的 UI 片段，无独立生命周期。混淆两者会把 MCP 误当插件系统来设计。
- **统一 iframe 渲染层，双源复用**：只维护一套 iframe 沙箱组件，同时渲染「本地 HTML 面板」与「未来兼容的标准 MCP App」；数据请求一律 postMessage 转发给宿主鉴权，前端不直连数据。
- **App 逻辑不强上 WASM**：检索/图谱/文件逻辑已在 Rust 宿主，App 的"深度接入"= 调宿主 API（manifest 声明权限即可）；HTML + 宿主 API 代理覆盖 90% 场景，WASM 仅保留为可选计算后端，等出现真实需求再引入。
- **通信复用现有传输**：iframe 用 postMessage 与父页面通信，父页面走现有 HTTP/SSE；不引入 WebSocket。
- **应用包 = 文件夹 / zip**，不造自定义归档格式（.hax 之类）；市场只做本地导入（拖拽离线安装），不做在线市场——个人场景无多应用供给，在线市场属生态叙事。
- **自组织必须带人工审核**：LLM 幻觉/错误关联会污染图谱，"可审计"是这套架构的立身之本；Agent 动态生成 App 同理，人审后安装，不绕开审核闭环。
- **工具调用走宿主代理、LLM 不直连**：工具（检索/图谱/网页/任务/文件）一律经 `/api/*` 宿主鉴权执行，浏览器不直连 LLM 与本地文件——为 LLM 显式 Tool Use（Phase 3-C P1）预留同一安全边界，工具权限即宿主 API 权限，新工具=新端点而非放开直连。
- **网页能力：bsk 外挂，Page 内化**。md-agent 不内置 browser-skill 形态（驱动真实浏览器 + 扩展，要求浏览器在线、会话纪律，与"后台自主"冲突，只作外部会话的外挂工具）。系统内置的"读/操作页面"能力 = Page 抽象（open/click/fill/extract/screenshot）+ 本地无头引擎：静态读取用 HTTP + HTML 解析（零浏览器依赖），动态/操作用 chromiumoxide 连系统 Edge/Chrome 的 headless 模式（零下载、自管登录态、Agent 可自主调度）；写操作（点击/提交）必须人审确认。

## 已知短板与边界

- 工具调用尚未 LLM 显式决策：Agent 回路的检索词由前端启发式提取，LLM 不发工具调用（Tool Use 显式化在 Phase 3-C P1）
- 无 Subagent / Multi-Agent：单 Agent 模型；`/task plan` 拆解由 LLM 一次性生成、宿主顺序执行
- 记忆无巩固/遗忘：L1/L2 只增不改（MEMORY 按日合并追加），自动压缩与降级未实现（Phase 3-C P2）
- 无 Skills 注册表：自组织产出是知识笔记，尚无程序性技能格式与自动触发（Phase 3-C P3）
- 检索无语义召回：换种说法问可能搜不到（Phase 2 图谱缓解，但语义召回仍需向量，当前定位明确不做）
- 多轮记忆仅会话内（localStorage 持久化，刷新不丢；跨重启如需持久可把历史写入 kb）
- 写回审核为「待审目录 + 行级预览」（/preview 看追加内容、/approve 整篇落地），尚无逐行合并/驳回编辑
- 托盘图标为代码生成的文档图案（白色圆角文档 + 知识行 + 链接点），正式设计图标待换
- 关键词提取为启发式（无真分词），英文/数字效果好于中文长句
- 终端表格不做对齐（避免 CJK 宽度计算与流式缓冲），需要整齐表格可 `open` 后用支持表格的编辑器查看原 markdown
- `/page` 依赖本机 Edge/Chrome（headless CDP），个别站点（如被网络环境拦截的域名）可能读到空正文；写侧目前是显式 selector 动作（/page act），LLM 自主决策尚未实现

## 与同类产品的定位差异

| 对比 | 差异（互补，非碾压） |
|---|---|
| 向量 RAG | 本架构可审计、可追溯、文件即库；代价是无语义召回 |
| Obsidian/Notion | 本架构可编程 Agent、可工具调用、可自动化；缺其插件生态 |
| ClaudeCode/Cursor/oh-my-pi | 同为 Agent Harness；本架构专精「记忆 + 审核闭环」、文件即库、无向量库；缺其代码执行/IDE 深度 |
| WebUI（OpenWebUI 等） | 本架构做本地文件原生治理与多项目隔离；缺其模型管理 |

## 开发测试

`scripts/mock_llm.py`：OpenAI 兼容 mock（默认 11434 端口），支持流式；最后一条用户消息含「记住/沉淀」时返回写回块，便于验证沉淀链路。

```
python scripts/mock_llm.py          # 起 mock
python scripts/mock_llm.py 9000     # 自定义端口
```

## 目录

```
src/main.rs   托盘 + winit 事件循环 + 服务线程
src/server.rs Axum 路由与接口
src/search.rs 检索（ignore 遍历 + grep crate，多关键词/智能大小写/小节上下文）
src/graph.rs  知识图谱（SQLite documents/links、[[链接]] 解析、反链/孤立/标签/项目查询）
src/llm.rs    LLM 代理（非流式 JSON + 流式 SSE 透传）
src/fetch.rs  /fetch 静态网页抓取（HTTP + HTML 文本提取）
src/page.rs   /page 动态网页 + /page act 动作执行（chromiumoxide + 系统 Edge/Chrome headless CDP）
src/heartbeat.rs 心跳自动同步（指纹检测 / 状态结构）
src/task.rs   任务引擎（kb/.tasks.db 独立库：状态机/依赖/日志）
src/kb.rs     双层记忆布局 / frontmatter 解析 / INDEX 自动生成 / 路径安全 / 待审机制
src/config.rs 本地配置
web/          xterm.js 终端前端（Agent 回路 + 管理命令）+ config.html 配置页
web/views/    内置面板视图（graph.html 结构导航 / board.html 任务看板）
kb/           L1 规范层 + L2 内容层（首次运行自动补齐模板）
scripts/      mock_llm.py 开发测试工具
dist/         release 打包产物（双击即用）
```
