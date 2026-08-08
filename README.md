# md-agent — 本地 Agent Harness：MD 持久记忆 + 工具链 + 人审闭环

本地常驻的 Agent Harness：托盘常驻底座 + 网页终端交互 + LLM 代理 + 检索/图谱/网页/任务工具链；记忆与写回以 Markdown 纯文本落盘，可审计、可追溯、永不锁库。

> 一句话定位：**知识人写人读，AI 只负责整理、关联、推演、生成。**
>
> 本质：本系统是 Agent 的运行层（harness）——底座 + 终端 + LLM 代理 + 工具链；其中 **双层 MD 记忆是核心子系统**：正式记忆（人工审核固化）+ 待审草稿（Agent 候选，必经人审）+ 自组织整理器。它把 Agent 的长期记忆固化为纯文本文件，突破单次上下文窗口的容量限制；检索只是记忆的读取手段，知识正文永远是 Markdown。

## 核心哲学

1. **自组织而非自进化**：系统只负责组织 Agent 已有的资源——记忆（链接/去重/索引/巩固）、行为（技能/上下文组装）、评估（什么用得好）；不声称提升模型能力——能力来自模型训练与检索获取。
2. **记忆即文件**：原始知识永远是 `.md`，可用任意编辑器打开/修改/同步，永不锁库、永不黑盒；文件系统即持久记忆。
3. **不用向量，用显性人类知识结构做关联**：`[[双向链接]]` + Frontmatter 元数据 + 目录项目层级。关联可审计、可追溯。
4. **双检索互补（不是碾压，是各管一段）**：
   - **ripgrep 全文检索** → 找片段、找关键词（已实现）
   - **SQLite 结构化图谱检索** → 找关联、找脉络、找项目体系（已实现）

## 当前能力（Phase 1–3 已完成 ✅）

### 记忆核心

- **双层 MD 持久记忆**：记忆子系统：L1 规范/记忆/索引层（CLAUDE.md 模式，启动注入）+ L2 内容层（grep 检索）；`INDEX.md` 自动生成；写回一律走待审，人工审核固化
- **待审机制**：LLM 生成的新笔记/记忆条目先进 `pending/`（不直接污染知识库）：图形审核在「自动化」面板的审核栏目（`/view automation`，待审清单批量勾选 / 目标文档上下文+绿色 diff / 可编辑内容；支持编辑后批准与批量批准/拒绝）；终端 `/pending` `/approve` `/reject` 保留；落地自动重建 INDEX+图谱；待审文件不进检索与图谱
- **多轮对话记忆**：会话内保留最近 4 轮，localStorage 持久化（刷新页面不丢，`/clear` 清空）
- **写回沉淀**：LLM 回答附 `<!-- md-agent-save -->` 块自动落盘：新知识写 L2（自动补 frontmatter）、决策写 L1 MEMORY；`/remember` 手动沉淀

### 检索与图谱

- **全文检索**：内嵌 ripgrep 内核（grep + ignore crate），多关键词任一命中、智能大小写、小节上下文（`section`/`context`）
- **知识图谱**：SQLite `documents`/`links` 两表：`[[双向链接]]` 解析、反向链接、孤立文档检测、标签/项目维度统计；首次调用自动建库，`/sync`、`/rescan` 或心跳自动重建
- **图谱可视化（/view graph）**：打开即**全库大图**（Obsidian 式，默认全屏，可切「双栏」树+图）；度数最高文档为 hub 中心展开全部连通节点、孤立文档补最外环；hover 邻域高亮+其余淡化（画布↔树行双向）、节点大小∝连接数、缩放文字淡出、入场动画、节点拖拽固定、选中光环+相机平滑聚焦、搜索候选下拉带类型色点（命中即聚焦）、边按类型渐变着色、深度滑块 1-3

### 终端交互

- **终端壳体验**：启动欢迎横幅 + 状态汇总（版本/KB/图谱/模型/待审/进行中任务）；**输入框回流内**——4 行结构贴内容末尾（上边框/输入行/下边框/状态行），resize 自动重画；↑↓ 输入历史、Tab 命令循环补全（`/` 命令 + `@` 文件提及）、Ctrl+C 中断、Esc 停止、Ctrl+K 速览均经 `attachCustomKeyEventHandler`；推理思考折叠（流式 `reasoning_content` → 推理期灰色「🧠 思考中…」→ 首个内容到达时清除并出「──── 回答 ────」标题，回答后「Thought · N 秒」折叠行）；本次回答 token 用量（`stream_options.include_usage`，引用来源前）；@ 文件提及（`@xxx`+Tab 补全 KB 文档路径，提交时指定文档全文注入检索目标）；输入草稿 + 命令历史 localStorage 持久化（刷新恢复未提交输入，上限 100）；命令面板 + 状态中心（`/side` / 速览按钮 / Ctrl+K 唤出左侧抽屉：上部模糊搜索直达视图/`/` 命令/`@` 文档/已装应用，↑↓ 选择回车执行；下部任务/审核/图谱速览卡 8s 自动刷新，点卡片直达面板）；侧边栏功能菜单（功能/系统两组：图谱 / 自动化（红徽标=待审数，无待审时黄徽标=审计发现数）/ 市场 / 设置 / 命令速览，点击直达对应面板，徽标与状态行同源 8s 轮询）；状态行（● 服务/模型/KB/待审/任务/图谱/心跳 + ⚠审计警告，8s 轮询）；提交消息块保留流内整行背景色；回答期间只放行导航命令（/view /side /help open），其余命令与输入拦截，按钮行导航可随时点、不打断回答流
- **伪命令行 Markdown 渲染**：终端内 ANSI 富渲染（零依赖）：标题加粗、行内代码/加粗/链接/`[[双链]]` 着色、列表/引用/代码围栏；**表格按 markdown 行显示**（保留 `|` 结构，复制不失真）；frontmatter 变暗；流式回答按完整行渲染

### 面板视图

- **/view 面板渲染层**：iframe 沙箱 + postMessage 桥（视图经宿主调 `/api/*`，仅允许 api 前缀，真机验证通过）：`/view graph` 知识图谱可视化（**打开即全库大图**：hover 邻域高亮/度数定大小/文字淡出/树图双向联动/搜索聚焦/全屏切换，详见「检索与图谱」）、`/view board` 任务看板、`/view automation` 自动化面板（控制/审核/运营数据三栏目）、`/view <html>` 渲染 kb 内本地 HTML、`/view off` 或 Esc 关闭；**单视图**（同一时刻只开一个详情页，新开替换旧的，`/view ops`/`/view pending`/`/view audit` 兼容映射到自动化面板）；分屏参照（header「分屏/全屏」切换：终端占左 40% 保持可见可参照 + 视图右 60%，选择记忆、关闭视图自动回全屏）；沙箱脚本错误上报宿主写终端提示、桥请求 20s 超时兜底；面板写操作（批准待审/改任务/补链/卸载）经桥回传后主界面状态栏即时刷新；关闭视图后焦点自动归还终端输入框（面板往返无需点终端）
- **自动化面板**：`/view automation`（兼容 `/view ops`/`/view pending`/`/view audit`），手风琴三栏目——① 自动化控制（心跳开关/周期/自动补链/巩固器/经验闭环/会话归档说明）② 审核（待审三栏：清单批量勾选/目标上下文+绿色 diff/可编辑内容，支持编辑后批准与批量批准/拒绝；下叠健康审计折叠区：补链建议一键 [应用]、悬空/孤立/重复分组展示，`/audit` 终端版保留）③ 运营数据（记忆热度 / token 用量与缓存命中 / 近 7 日检索·工具·待审趋势 / 活动时间线与未读通知，3s 轮询）
- **待审行级预览**：`/preview <待审路径>` 只读展示批准后将写入的内容（记忆条目按当日小节合并规则计算，不落盘）
- **可视化配置页**：`/config.html`：endpoint / model / api_key（掩码显示）+ 测试连接

### 自组织与审核闭环

- **心跳自动同步（自组织自动发现）**：默认关闭；开启后每 60s（可调）指纹比对知识库（路径+mtime+大小，排除 pending/），变化自动重建 INDEX+图谱并跑本地审计，状态行提示「心跳开 + ⚠审计发现」；托盘勾选 / `/heartbeat` / 配置页三入口；`sync_lock` 与手动写端点防并发
- **记忆自组织（Phase 3-A 基础）**：`/audit` 本地规则健康审计（孤立/无出链/重复标题/悬空链接/提及未链接建议，零 LLM 快速确定）；`/link` 人工补链接（文件名双链、去重、自动重建图谱）；`/link-all` 一键应用建议；`/suggest` 补全缺失主题（带主题名）或无参盲区模式（先审计后让 LLM 分析知识盲区生成新文档，进待审）；`/diff`/`/conflicts` 行级对比与冲突检查
- **/digest**：检索结果交给 LLM 整理成结构化笔记写入 `notes/`

### 工具链

- **网页读取与操作**：`/fetch <url> [标题]` 静态抓取；`/page <url> [标题]` 动态读取（headless 等 JS 渲染）；`/page act <url> <json 动作数组>` 写侧（click/fill/select/scroll，动作清单**人工确认后执行**，返回页面结果）
- **文档摄入**：附件按钮「＋」选 PDF/DOCX/PPT/XLS/CSV/EPUB 等 → anydoc 本地转 Markdown（dry-run 预览 → 软弹窗确认）→ 落 `notes/` 自动重建 INDEX+图谱；扫描件/加密文档明确报错
- **任务引擎（Phase 3-B）**：`kb/.tasks.db` 独立 SQLite：目标/状态机（待办·进行中·完成·放弃）/依赖/推进日志；`/task` 终端文字看板 + `/task board` HTML 看板；**依赖就绪校验**（进入进行中/完成时依赖必须已完成）；`/task plan <目标>` LLM 拆解串行子任务链

### Agent 与 LLM

- **LLM 代理**：OpenAI 兼容（Ollama/DeepSeek 等），后端代理防 CORS 与密钥暴露；SSE 流式透传
- **Agent 问答回路**：启动注入 L1 → 提取关键词 → 检索 L2 → 拼 Prompt → 流式回答 → `[文件:行号]` 引用（检索词当前为前端启发式提取；LLM 显式 Tool Use 见路线图 Phase 3-C P1）

### 常驻与运维

- **托盘常驻**：tray-icon + winit，右键菜单分组：打开终端 / 应用市场 / 同步（INDEX+图谱全量重建）/ 已安装应用子菜单（动态）+ 面板导航子菜单（待审/看板/审计/图谱）/ 心跳同步（复选框 + 文字「开/关」双信号，点击即切换）/ Key 设置（直达 /config.html API Key 输入）/ 退出；release 单 exe 隐藏控制台

### 项目空间（多项目硬隔离）

- **项目制**：每个项目 = `kb/projects/<id>/` 下的独立迷你知识库（自己的 L1 规范、笔记、会话、图谱/活动/任务三库）；项目之间**完全隔离**——检索、会话、记忆、图谱绝不串用（隔离由 root 指向 + 遍历排除双层保证，非约定）
- **个人空间**：现有全局 KB 自动成为「个人空间」默认项目，零迁移
- **切换与新建**：顶栏项目切换器（chip + 菜单）；新建向导三模板：空白 / 律师案件（案件总览·证据清单·时间线·法律研究）/ 猎头项目（职位需求·候选人台账·客户·沟通记录）
- **新手引导**：首次启动未配置连接时自动弹出三步向导（连接 DeepSeek 预填默认值 → 创建第一个项目 → 开始使用）；`/spaces` 列出项目空间
- **隔离语义**：全局（个人空间）检索与图谱不含任何项目内容；项目内 API 经请求头 `X-Project` 解析到项目根

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

配置统一存储在 `%APPDATA%\md-agent\config.json`（Windows；其他平台 `$XDG_CONFIG_HOME` 或 `~/.config/md-agent/config.json`）——debug/dist 共享一份不再漂移；旧 exe 旁 config.json 首次启动自动迁移（先到先得）；`MD_AGENT_CONFIG` 可覆盖路径（测试隔离用）。

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

### 项目空间

顶栏左侧项目 chip 显示当前项目，点击弹出项目菜单（个人空间 + 全部项目 + 新建入口）；也可在功能首页「项目空间」区切换。

- 每个项目独立知识空间，切换即隔离：会话、笔记、检索、记忆各自一套
- 新建项目：项目菜单 →「＋ 新建项目」→ 选模板（空白 / 律师案件 / 猎头项目）+ 命名 → 自动进入
- 首次启动未配置连接时自动弹出三步引导（连接 DeepSeek 预填默认值 → 创建第一个项目）；跳过可随时从顶栏新建
- `/spaces` 列出全部项目空间；`/newproject` 打开新建向导

### 终端命令

直接输入问题走 Agent 问答；`/help` 查看全部：

```
# 检索与阅读
/search <关键词>      检索双层库（显示所属小节）
open <路径>           查看 KB 内 MD
/l1                   查看 L1 规范/记忆/索引层
/graph <路径>         知识图谱：出链/入链/关联簇
/orphans              孤立文档（无入链也无出链）
/projects             项目维度统计    /tags 标签统计

# 记忆写回与索引
/remember [路径] 内容  手动沉淀（默认 MEMORY.md）
/digest <主题>        检索并把结果整理成新笔记写入 notes/
/sync                 重建 INDEX.md
/rescan               重建知识图谱（SQLite）

# 待审与预览
/pending              查看待审（LLM 写回/生成笔记先进这里）
/preview <待审路径>    行级预览：批准后将写入的内容（只读）
/approve <路径|all>    批准待审 → 写入知识库（自动重建 INDEX+图谱）
/reject <路径|all>     丢弃待审

# 面板与速览
/view graph|board|pending|audit|<html>|off  面板渲染层（多标签并存，Esc 关闭当前）
/side                命令面板 + 状态中心（视图/命令/@文档搜索直达 + 速览卡，Ctrl+K 或速览按钮同样唤出）

# 自组织
/audit                知识库健康审计（盲区/冲突/补链接建议）
/conflicts            冲突检查（重复标题/悬空链接）   /diff <A> <B> 行级对比
/link <源> <目标>      补链接（在源文档追加 [[目标]]，人工确认）
/link-all              一键应用 /audit 的全部补链接建议
/suggest [主题]        LLM 补全缺失主题（无参 = 盲区分析模式，均进待审）
/decide <主题> <结论>  未决决策拍板：从未决清单移除议题 → 结论落 notes/决策/已决.md + L1 MEMORY 决策区（会话收尾自动检测未决议题进待审，批准后进未决清单）

# 网页
/fetch <url> [标题]    静态抓取网页：阅读视图 / 带标题则沉淀为待审笔记
/page <url> [标题]     动态网页读取（headless Edge/Chrome，等 JS 渲染）
# 文档摄入
＋ 附件按钮             摄入 PDF/DOCX/PPT/XLS/CSV/EPUB → 预览 → 软弹窗确认进 notes/

# 任务
/task                  任务看板：new/start/done/drop/note/dep/rm/plan/board

# 系统
/clear                清空多轮对话记忆
/config               查看配置（掩码）
/heartbeat [on|off|interval <秒>|status]  心跳自动同步开关/周期/状态（变化自动重建+审计提示）
/health               服务健康检查
```
`/page act <url> <json 动作数组>`：动态页**写侧**——click/fill/select/scroll，动作清单打印后软弹窗确认才执行（例：`/page act https://example.com [{"kind":"fill","selector":"#q","value":"hello"},{"kind":"click","selector":"#btn"}]`）。

确认交互约定：**可逆操作免确认**（新对话/归档会话/断开 hub 直接执行），**不可逆或安全敏感操作走页面内软弹窗**（删除会话/文档摄入/动作执行/安装/卸载/更新等，危险操作红色按钮，Esc/遮罩/取消均可退出）——不再有终端 y/N 按键等待。

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
| GET | `/api/projects` | 项目列表（项目制：各项目独立隔离知识空间） |
| POST | `/api/projects` | 创建项目（body: `{name, template}`，template: `blank`/`lawyer`/`headhunter`） |
| GET | `/api/projects/{id}` | 项目详情 |
| PATCH | `/api/projects/{id}` | 重命名（body: `{name}`） |
| DELETE | `/api/projects/{id}` | 删除项目（个人空间不可删） |
| （项目内 API） | 检索/会话/文件/图谱/记忆/待审等 | 请求头 `X-Project: <id>` 限定到项目根；缺省回退全局「个人空间」 |
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
| POST | `/api/ingest` | 文档摄入（body: `{name, content_base64, dry_run?}`；anydoc 转 GFM，dry_run 预览 / 落 notes/ 重建 INDEX+图谱） |
| POST | `/api/decide` | 未决决策拍板（body: `{topic, conclusion}`；未决清单移除 → 已决.md + MEMORY 决策区，幂等） |
| POST | `/api/page/act` | 动作执行（body: `{url, actions: [{kind: click\|fill\|select\|scroll, selector, value?}]}`；前端人审确认后调用） |
| GET | `/api/tasks` | 任务列表 + 看板统计（`kb/.tasks.db` 独立库） |
| POST | `/api/tasks` | 新建任务（body: `{goal, title?}`） |
| PATCH | `/api/tasks/{id}` | 任务更新（`{status?, note?, deps?}`，note 追加带时间戳日志） |
| DELETE | `/api/tasks/{id}` | 删除任务 |
| GET/POST | `/api/config` | 本地配置（GET 掩码 api_key） |
| POST | `/api/llm` | LLM 代理（`stream=true` 走 SSE 流式，否则 JSON 透传） |
| GET | `/api/apps` | 已安装应用列表（manifest：id/name/version/entry/permissions/source_hub） |
| GET/POST | `/api/apps/{id}/data` | App 状态持久化（`storage` 权限）：读/写 `kb/apps/{id}/data/localstorage.json`——沙箱无 allow-same-origin → localStorage 不可用，桥层注入内存+防抖落盘代理，app 代码零改动 |
| GET | `/api/hubs` | 已连接 SkillHub 列表 |
| POST | `/api/hubs/connect` | 连接 SkillHub（body: `{url}`；拉取校验 skillhub.md 索引入库，返回 hub 与可用应用） |
| POST | `/api/hubs/refresh` | 刷新 hub 索引（body: `{name}`；失败保留旧索引，降级不丢目录） |
| POST | `/api/hubs/disconnect` | 断开 hub（body: `{name}`；不删已装应用） |
| GET | `/api/market/catalog` | 已连接 hub 合并目录（条目带 source + 来源 hub） |
| POST | `/api/market/install` | 安装（body: `{source?, path?, hub?}`；source=hub 条目下载，`dry_run=true` 人审校验返回 manifest） |
| POST | `/api/market/uninstall` | 卸载（body: `{id}`） |
| POST | `/api/market/update` | 更新（body: `{id, path}` 本地新版本目录，卸载重装） |

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
Phase 3-C Harness 深化（✅ P1/P3/P4 + SkillHub 接入已完成；P2 巩固器已落地）
         ├─ ✅ P1 工具注册表 + Agent Loop：GET /api/tools 声明式工具清单（name/desc/params）；LLM 决策调工具 → 宿主执行 → 结果回填，Agent 回路从「启发式关键词 → grep」升级为 LLM 显式 Tool Use
         ├─ ✅ P2 巩固/遗忘：consolidate 两阶段（先规则后 LLM）+ 记忆提案四型待审通道；任务感知上下文组装（CE 组装器 + memory_summary 注入；C 半步：LLM 配置时 L1 全文移出前缀、read_l1/search 按需取用——同口径评测 input/total -47%、cache 率 74%）
         ├─ ✅ P3 Skills / 程序性自组织：/suggest、/audit 产出升级为程序性技能（技能格式 + 注册表 /api/skills + trigger 命中自动注入）
         ├─ ✅ P4 App 系统（原 Phase 4 提前）：manifest + 权限白名单 + 生命周期（/api/market/*）+ 面板/托盘动态菜单 + 沙箱 iframe 渲染（/view）
         └─ ✅ SkillHub 接入：应用市场 = hub 管理端 + 客户端——**侧边栏入口已升级为「工作台」**（/view market：我的应用卡片网格 + 状态速览，纯应用启动器；应用市场收进右上角「🛒 应用市场」二级入口；项目空间/会话列表在左侧栏下方——顶部「🗂 项目空间」行点击切换 + 「进行中/会话列表」分组，目录/已安装双 Tab、打开默认「已安装」、目录已装应用排最前标注徽标）；/market connect 连接第三方 SkillHub（skillhub.md 索引协议），安装走命令行（人审确认）；本地导入兜底；market.connect 工具 + 技能触发（LLM 一句话连商店）；条目统一（应用与技能同目录，按包内容识别落点：app.json→kb/apps/，SKILL.md/裸 md→kb/skills/）
Phase 4  生态化（可选，与"轻量"定位有张力，个人场景可长期搁置）
         ├─ MCP 客户端（Stdio/SSE）；兼容标准 MCP App 渲染（复用统一 iframe 渲染层）
         └─ WASM 计算后端（仅当出现"本地运行不可信计算"的真实需求）
```

**设计决策**（含 2026-08 对「Host App / WASM 插件」方案评审结论）：

- **概念界定：App ≠ MCP App**。本项目的"应用"= 一体化应用包（HTML 界面 + 业务逻辑，可安装/启用/运行/关闭/卸载、可上架插件市场）；MCP App 只是依附外部 MCP 进程的 UI 片段，无独立生命周期。混淆两者会把 MCP 误当插件系统来设计。
- **统一 iframe 渲染层，双源复用**：只维护一套 iframe 沙箱组件，同时渲染「本地 HTML 面板」与「未来兼容的标准 MCP App」；数据请求一律 postMessage 转发给宿主鉴权，前端不直连数据。
- **App 逻辑不强上 WASM**：检索/图谱/文件逻辑已在 Rust 宿主，App 的"深度接入"= 调宿主 API（manifest 声明权限即可）；HTML + 宿主 API 代理覆盖 90% 场景，WASM 仅保留为可选计算后端，等出现真实需求再引入。
- **通信复用现有传输**：iframe 用 postMessage 与父页面通信，父页面走现有 HTTP/SSE；不引入 WebSocket。
- **应用包 = 文件夹 / zip**，不造自定义归档格式（.hax 之类）；**应用市场 = SkillHub 管理端 + 客户端**——不内置市场目录，通过 `/market connect <url>` 连接第三方 SkillHub（hub = skillhub.md 轻索引，只管"谁有什么/去哪下载"），app 包本体在任意位置（git / GitHub zip / 本地），安装落 `kb/apps/` 后由本地托管 + 沙箱渲染（市场与托管职责分离、已安装即本地权威副本）；本地手动导入永远是兜底通道。
- **SkillHub 信任分级**：连 hub 自动（拉 md 索引不执行代码，派生产物无人审）→ 安装走命令行（`/market install <id>` / `/market import <路径>`，dry_run 人审确认）；source 协议白名单（git+https / GitHub zip / 裸 md / 本地；裸 http 禁）；沙箱 iframe + manifest 权限白名单兜底。
- **自组织必须带人工审核**：LLM 幻觉/错误关联会污染图谱，"可审计"是这套架构的立身之本；Agent 动态生成 App 同理，人审后安装，不绕开审核闭环。
- **工具调用走宿主代理、LLM 不直连**：工具（检索/图谱/网页/任务/文件）一律经 `/api/*` 宿主鉴权执行，浏览器不直连 LLM 与本地文件——为 LLM 显式 Tool Use（Phase 3-C P1）预留同一安全边界，工具权限即宿主 API 权限，新工具=新端点而非放开直连。
- **网页能力：bsk 外挂，Page 内化**。md-agent 不内置 browser-skill 形态（驱动真实浏览器 + 扩展，要求浏览器在线、会话纪律，与"后台自主"冲突，只作外部会话的外挂工具）。系统内置的"读/操作页面"能力 = Page 抽象（open/click/fill/extract/screenshot）+ 本地无头引擎：静态读取用 HTTP + HTML 解析（零浏览器依赖），动态/操作用 chromiumoxide 连系统 Edge/Chrome 的 headless 模式（零下载、自管登录态、Agent 可自主调度）；写操作（点击/提交）必须人审确认。

## 已知短板与边界

- 工具调用已 LLM 显式决策（/api/tools 声明式清单 + Agent Loop），但工具集仍为宿主内建（8+1 个），外部工具/MCP 客户端（Stdio/SSE）未做（Phase 4）
- 无 Subagent / Multi-Agent：单 Agent 模型；`/task plan` 拆解由 LLM 一次性生成、宿主顺序执行
- 记忆巩固器已实现（规则+LLM 两阶段、走待审通道），自动遗忘/降级与按任务动态注入的深化未做（Phase 3-C P2 余项）
- Skills 注册表已就绪（/api/skills + trigger 触发注入），技能产物的质量收敛仍依赖人审（/approve）
- 检索无语义召回：换种说法问可能搜不到（Phase 2 图谱缓解，但语义召回仍需向量，当前定位明确不做）
- 多轮记忆仅会话内（localStorage 持久化，刷新不丢；跨重启如需持久可把历史写入 kb）
- 写回审核为「待审目录 + 行级预览」（/preview 看追加内容、/approve 整篇落地），尚无逐行合并/驳回编辑
- 托盘图标为代码生成的文档图案（白色圆角文档 + 知识行 + 链接点），正式设计图标待换
- 关键词提取为启发式（无真分词），英文/数字效果好于中文长句
- 终端表格不做对齐（避免 CJK 宽度计算与流式缓冲），需要整齐表格可 `open` 后用支持表格的编辑器查看原 markdown
- `/page` 依赖本机 Edge/Chrome（headless CDP），个别站点（如被网络环境拦截的域名）可能读到空正文；写侧目前是显式 selector 动作（/page act），LLM 自主决策尚未实现
- `/api/ingest` 文档摄入（anydoc）对扫描件/图片型 PDF 明确不支持（无 OCR）；复杂排版（分栏/表格/嵌入字体）的 PDF 可能丢部分结构，坏样本可收集后评估 pdfium/mupdf 兜底

## 与同类产品的定位差异

| 对比 | 差异（互补，非碾压） |
|---|---|
| 向量 RAG | 本架构可审计、可追溯、文件即库；代价是无语义召回 |
| Obsidian/Notion | 本架构可编程 Agent、可工具调用、可自动化；缺其插件生态 |
| ClaudeCode/Cursor/oh-my-pi | 同为 Agent Harness；本架构专精「记忆 + 审核闭环」、文件即库、无向量库；缺其代码执行/IDE 深度 |
| WebUI（OpenWebUI 等） | 本架构做本地文件原生治理与多项目隔离；缺其模型管理 |

## 开发测试

测试 = harness 代码层的"人审闭环"：人审保护记忆不被 LLM 污染，测试保护 harness 不被改动破坏（同一哲学）。**隔离铁律：所有测试用临时目录，绝不碰主 kb。**

```
bash scripts/test.sh          # 一键回归：Rust 单测 + 前端逻辑测试 + E2E 四型审批链路
cargo test                    # Rust 单测（kb/graph/task/search/heartbeat/consolidate/config/…）
node scripts/frontend-test.js # 前端纯逻辑测试（core.js，66 断言，零 DOM）
node --test tests/web/        # 前端 node:test 套件（core 15 组 + market 2 组）
python scripts/e2e.py         # E2E：隔离 kb 起服务，跑待审四型审批链路
```

`scripts/mock_llm.py`：OpenAI 兼容 mock（默认 11434 端口），支持流式；最后一条用户消息含「记住/沉淀」时返回写回块，便于验证沉淀链路。

```
python scripts/mock_llm.py          # 起 mock
python scripts/mock_llm.py 9000     # 自定义端口
```

## 目录

```
src/main.rs       托盘 + winit 事件循环 + 服务线程
src/server.rs     Axum 路由与接口
src/search.rs     检索（ignore 遍历 + grep crate，多关键词/智能大小写/小节上下文）
src/graph.rs      知识图谱（SQLite documents/links、[[链接]] 解析、反链/孤立/标签/项目查询）
src/llm.rs        LLM 代理（非流式 JSON + 流式 SSE 透传）
src/ingest.rs     文档摄入（anydoc 转 GFM：PDF/DOCX/PPT/XLS/CSV/EPUB → notes/）
src/fetch.rs      /fetch 静态网页抓取（HTTP + HTML 文本提取）
src/page.rs       /page 动态网页 + /page act 动作执行（chromiumoxide + 系统 Edge/Chrome headless CDP）
src/heartbeat.rs  心跳自动同步（指纹检测 / 状态结构）
src/task.rs       任务引擎（kb/.tasks.db 独立库：状态机/依赖/日志）
src/kb.rs         双层记忆布局 / frontmatter 解析 / INDEX 自动生成 / 路径安全 / 待审机制
src/config.rs     本地配置
web/              xterm.js 终端前端（Agent 回路 + 管理命令）+ config.html 配置页
web/views/        内置面板视图（graph.html 全库大图可视化 / automation.html 自动化三栏目 / market.html 应用市场 / board.html 任务看板）
tests/web/        前端 node:test 纯逻辑测试（core.test.js / market.test.js）
kb/               L1 规范层 + L2 内容层（首次运行自动补齐模板）
scripts/          mock_llm.py / frontend-test.js / e2e.py / test.sh
dist/             release 打包产物（双击即用）
```
