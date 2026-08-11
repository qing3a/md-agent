# md-agent — 本地常驻通用 Agent Harness：MCP 双模式 · 可审计 MD 记忆 · 行业应用生态

纯本地 Rust 构建的通用 Agent 运行底座：托盘常驻 + 网页终端 + LLM 代理 + 检索/图谱/任务工具链 + 沙箱应用生态。数据全程留存本机，记忆以 Markdown 纯文本落盘——可审计、可追溯、永不锁库。

> 一句话：**知识人写人读，AI 只负责整理、关联、推演、生成。**
> 本质：Agent 的运行层（harness）——双层 MD 记忆是核心子系统（正式记忆人审固化 + 待审草稿必经人审）；检索是读取手段，知识正文永远是 Markdown。

## 三大核心价值

1. **MCP 双模式（生态互联）**：本地 stdio MCP 薄壳已实现（10 个纯本地工具，任意 MCP 客户端一行接入）；远程 MCP 客户端规划中（主动聚合第三方 MCP 服务与自研行业服务——唯一远程交互方式，不开放自身为远程服务）
2. **可审计双层 MD 记忆（合规底座）**：L1 规范/记忆层 + L2 内容层，全部知识可读 Markdown；AI 生成内容一律进待审，人工确认后才写入正式知识库——明文可审计、操作可追溯，区别于黑盒向量库
3. **行业应用生态（垂直落地）**：沙箱应用平台（单文件 HTML + 权限白名单 + 桥通信）+ 完整猎头业务资产（21 屏三角色工作台、60+ 业务 API 数据模型、本地+远程协作同步方案）——通用底座之上的可拆卸行业套件

## 生态能力全览（Agent 基建六大赛道）

| 赛道 | md-agent 对应能力 |
|---|---|
| MCP 服务 | 标准 stdio MCP 薄壳（`--mcp`，10 工具）；远程 MCP 客户端（规划中） |
| Skill 技能 | 技能注册表 + SkillHub 商店（一键安装/卸载、沙箱隔离、trigger 注入） |
| Agent 编排 | 自研多轮工具回路 + 子 Agent（受限白名单/防递归）+ 任务引擎（拆解/状态机）+ 人审 Gate 闭环 |
| 聚合 | 远程 MCP 客户端聚合多服务（首个真实对接目标 = 自研猎头 ERP API hub） |
| 本地 UI | Rust 托盘常驻 + 网页终端（知识图谱可视化/自动化工作台/技能市场/审批面板） |
| 行业插件 | 猎头全流程工作台（21 屏三角色）+ 律师/猎头项目模板 + 规则风控预警 |

## MCP 双模式

架构明确：**md-agent 只做 MCP 客户端，不对外暴露远程 MCP 服务**（本地 stdio 薄壳是唯一服务形态）。两条链路各司其职：

### 模式一：本地 Stdio MCP 薄壳（已实现）

`md-agent --mcp`：stdio JSON-RPC 暴露 10 个纯本地工具（检索/记忆/图谱/风控/待审/任务），任意 MCP 客户端一行配置接入：

```bash
# Claude Code 示例（DeepSeek Harness / Cursor 等 MCP 客户端同理）
claude mcp add md-agent -- md-agent --mcp
```

md-agent 当「知识/记忆/风控层」，推理由调用方负责——零 LLM 依赖，进程在即可用，数据不出本机。

### 模式二：远程 MCP 客户端（规划中，触发条件已满足）

主动连接第三方 MCP 服务与**自研行业服务**，实现本地节点聚合远程业务能力：

- 首个真实对接目标：自研猎头 ERP（60 端点 API hub，JWT + API Key 双鉴权），封装为标准 MCP 服务后由本地节点消费
- 远程工具走 `mcp__<服务>.<工具>` 命名空间，默认拒绝、人工审批放行（与记忆人审同一哲学）
- 不开放自身为远程服务：单用户节点无暴露面，认证/运维负担为零

## 快速开始

```bash
cargo run                # 托盘模式（默认 8756 端口）
```

浏览器打开 `http://127.0.0.1:8756` → 首次启动三步引导（配置 DeepSeek → 创建项目 → 开始使用）。

其他运行方式：`--no-tray` 纯服务 / `--port 9000` 自定义端口 / `--mcp` MCP 薄壳模式。

环境变量：`MD_AGENT_KB`（KB 根）、`MD_AGENT_PORT`、`MD_AGENT_CONFIG`（config.json 路径）、`MD_AGENT_NO_TRAY`。

## 能力总览

### 记忆与知识

- **双层 MD 持久记忆**：L1 规范/记忆/索引层（启动注入）+ L2 内容层（grep 检索）；`INDEX.md` 自动生成；写回一律走待审，人工审核固化
- **待审机制**：LLM 生成的新笔记/记忆条目先进 `pending/`——图形审核（自动化面板：批量勾选/目标上下文绿色 diff/编辑后批准）、终端 `/pending` `/approve` `/reject` 保留；落地自动重建 INDEX+图谱；待审文件不进检索与图谱
- **写回沉淀**：回答附 `<!-- md-agent-save -->` 块自动落盘（新知识写 L2、决策写 L1 MEMORY）；`/remember` 手动沉淀
- **会话归档检索化**：会话归档（/clear 收尾 LLM 摘要、侧边栏 × 轻归档规则摘要）落 `notes/会话归档/` 进可检索层
- **跨会话记忆**：提问前自动召回（grep + 可选语义双路 RRF 融合）；会话收尾 LLM 提炼 + 后台巩固（产物一律进待审，人审后合并进 MEMORY.md）
- **语义召回（可选通道）**：配置 `llm.embedding` 后 `/api/embed/sync` 建向量索引，检索叠加语义命中（RRF k=60）；未配置自动降级纯 grep，零破坏

### 检索与图谱

- **全文检索**：内嵌 ripgrep，多关键词任一命中、智能大小写、小节上下文
- **知识图谱**：SQLite 双链（documents/links），实体类型化 + 反链/孤立/标签/项目维度；路径 BFS（≤6 跳）；激活扩散检索（命中沿图边联想）；心跳自动重建
- **图谱可视化（/view graph）**：沉浸画布 + 情境抽屉（树抽屉/详情抽屉/过滤条）、双骨架放射树、思源式目录/标签节点、hover 邻域高亮、路径链橙色、UI 状态持久化

### Agent 与工具

- **LLM 代理**：OpenAI 兼容（Ollama/DeepSeek 等），后端代理防 CORS 与密钥暴露；SSE 流式透传；联网通道（web_search，触发词或检索 0 命中自动开）
- **Agent 问答回路**：LLM 显式调工具（声明式清单：search/read_l1/memory_search/graph/risk.check/fetch/page/file/tasks/pending.list/dev 工具链）→ 宿主执行回填 → 循环（上限 8 轮，强制回答轮兜底）→ 流式回答 → `[文件:行号]` 引用可点击跳图谱；可选后端回路（Rust run_loop + ToolPolicy）
- **子 Agent**：独立上下文 + 只读工具白名单 + 防递归/防越权（`/api/agent spawn=true` 或 MCP `agent.spawn`）
- **交互卡片**：工具结果旁路渲染对话流卡片（风控/待审/路径链/任务/链接卡，按钮直接操作，不污染 LLM 上下文）
- **自我开发工具链**：dev.read/status/diff/patch/apply——AI 读自己代码、生成改进提案（进待审人审）、应用（备份/构建/回滚）
- **任务引擎**：`kb/.tasks.db` 独立 SQLite，状态机/依赖校验/推进日志；`/task` 终端看板 + `/task board` HTML 看板；`/task plan` LLM 目标拆解

### 应用平台（工作台）

- **工作台/应用市场**：`kb/apps/<id>/` 单文件 HTML 应用，沙箱 iframe + manifest 权限白名单（llm/storage/agent/search/graph/file/write…）；安装走 `/market import|install`（dry_run 人审）；SkillHub 索引连接；侧边栏「工作台」子菜单展示已装应用
- **应用 × Agent 协作**：应用委托宿主 agent 全回路（agent:ask：context 结构化入参、结果 JSON 约定标记回推、应用任务授权）；三应用已接入（猎头助手 L2 / 相亲评估 L2 / 招聘工作台 L0）
- **应用空间**：每应用私有知识层 `kb/apps/<id>/notes/`（agent:ask space:true 注入摘要）；桥写文件限定自己目录（防越权）；排除出主库检索/图谱/心跳
- **应用状态持久化**：桥层 localStorage 代理（storage 权限）→ 防抖落盘 `kb/apps/<id>/data/localstorage.json`（沙箱无 allow-same-origin 的替代通道）
- **AI 升级应用代码**：应用内「改进应用」→ agent 读自己代码 → dev.patch 提案（限定自己目录）→ 人审 → dev.apply（纯应用文件跳过构建）

### MCP 出口

- **`md-agent --mcp`**：stdio JSON-RPC（MCP 协议），10 个纯本地工具——MCP 客户端一行配置接入；md-agent 当知识/记忆/风控层，推理归调用方（零 LLM 依赖，进程在即可用）

### 常驻与运维

- **托盘常驻**：右键菜单（打开终端/应用市场/同步/已安装应用/面板导航/心跳开关/Key 设置/退出）；release 单 exe 隐藏控制台
- **心跳自动同步**：默认关；开启后周期指纹比对，变化自动重建 INDEX+图谱并跑本地审计（孤立/悬空/重复）
- **网页能力**：`/fetch` 静态抓取；`/page` 动态读取（headless Edge/Chrome）；`/page act` 写侧（click/fill/select，人工确认后执行）
- **文档摄入**：PDF/DOCX/PPT/XLS/CSV/EPUB → anydoc 本地转 Markdown（dry-run 预览 → 确认）→ 落 `notes/` 自动重建索引
- **项目空间**：多项目硬隔离（`kb/projects/<id>/` 独立迷你知识库，检索/会话/记忆/图谱绝不串用）；新建向导三模板（空白/律师案件/猎头项目）

## 行业资产（垂直差异化）

7 月沉淀的三套成熟猎头 SaaS 资产（业务模型完整、测试全绿），作为行业应用的设计蓝本与数据模型来源：

| 资产 | 内容 |
|---|---|
| 21 屏三角色工作台 | PM（漏斗/沙盘/方案对比/人才匹配）+ 猎头（候选人池/Pipeline 看板/消息/任务）+ 候选人（浏览/申请/画像/Offer 决策） |
| 60+ 业务 API 数据模型 | 候选人/职位/推荐/客户/标签/报表/AI 匹配，OpenAPI 3.0 规范 + JWT/API Key 双鉴权 |
| 本地+远程协作同步 | 离线/本地/协作三模式 + HMAC 签名 webhook + outbox + LWW 冲突解决 |

猎头工作台 = 内核之上的**可拆卸应用**（沙箱 iframe，复用宿主记忆/检索/图谱/Agent/审批）；律师平台同理，随需安装。

## 架构（四层）

```
常驻底座层    Rust 托盘常驻 + Axum 本地服务（127.0.0.1，同源托管前端免 CORS）
记忆内核层    双层 MD 记忆布局（L1 规范/记忆/索引 + L2 内容）/ frontmatter 解析 / INDEX 自动生成 / 待审机制 / 路径安全
工具链层      检索（ignore + grep crate）、图谱（SQLite 双链）、网页（/fetch /page act）、任务（tasks.db）——宿主 API 统一鉴权，LLM 不直连
交互层        网页终端（Agent 回路 + 管理命令 + 配置页）+ /view iframe 面板渲染层 + 对话流交互卡片 + 应用沙箱 + MCP stdio 出口
```

## 双层记忆结构

| 层 | 内容 | 记忆角色 | 进入上下文方式 |
|---|---|---|---|
| L1（kb 根目录） | `KB.md` / `FRAMEWORK.md` / `RULES.md` / `MEMORY.md` / `INDEX.md` | 引导记忆（bootstrap memory，类 CLAUDE.md） | 启动时注入 Agent（只放"位置+要点"，正文进 L2） |
| L2（`kb/notes/`） | 知识正文 | 内容记忆（retrievable memory） | 按需检索（grep），命中片段注入 Prompt |

## 定位差异

| 对比 | 差异（互补，非碾压） |
|---|---|
| 向量 RAG | 本架构可审计、可追溯、文件即库；语义召回为可选通道（Phase 4 已补） |
| Obsidian/Notion | 本架构可编程 Agent、可工具调用、可自动化；缺其插件生态 |
| ClaudeCode/Cursor | 同为 Agent Harness；本架构专精「记忆 + 审核闭环」+ MCP 记忆层出口；缺其代码执行/IDE 深度 |
| 通用 MCP 工具项目 | 本架构自带完整垂直行业套件（猎头 21 屏/60+ API），纯工具类项目无行业壁垒 |
| WebUI（OpenWebUI 等） | 本架构做本地文件原生治理与多项目隔离；缺其模型管理 |

## 终端命令速查

```
# 检索与阅读
/search <关键词>         全文检索 L2；/l1 <文件> 读 L1 规范/记忆
/graph <路径>            单篇图谱（出链/入链/相关）；/view graph 可视化
open <路径>              用系统编辑器打开文件
# 记忆写回与索引
/remember <内容>         手动沉淀；/digest <主题> 检索结果 LLM 整理成笔记
/sync /syncall /rescan   重建 INDEX+图谱
# 待审与预览
/pending                 列出待审；/preview <路径> 行级预览；/approve /reject
# 面板与速览
/view graph|automation|market|board|home|sessions|config|off
# 自组织
/audit                   本地规则健康审计；/link-all 一键应用补链建议；/suggest 补全缺失主题
# 网页 / 文档摄入 / 任务
/fetch <url> /page <url> [/page act <json>]；附件按钮「＋」摄入文档
/task new|start|done|drop|plan <目标>；/task board
# 系统
/heartbeat 心跳开关 /config 配置 /clear 清空多轮记忆 /spaces 项目空间
```

## 开发测试

测试 = harness 代码层的"人审闭环"（人审保护记忆不被 LLM 污染，测试保护 harness 不被改动破坏）。**隔离铁律：所有测试用临时目录，绝不碰主 kb。**

```bash
cargo test                 # Rust 单测（175 项：kb/graph/task/search/heartbeat/consolidate/agent/memory…）
node --test tests/web/     # 前端 node:test 套件（core 20 组 + market 2 组）
python scripts/e2e.py      # E2E（22 项）：隔离 kb 起服务，待审审批链路 + Agent 回路 + 语义召回
```

`scripts/mock_llm.py`：OpenAI 兼容 mock（默认 11434 端口），支持流式，便于验证沉淀链路。

## 迭代路线（当前状态）

```
Phase 1-2  ✅ 底座 + 终端 + LLM 代理 + Agent 回路 + 知识图谱
Phase 3    ✅ 自组织工作流：审计/补链/巩固/技能/任务引擎/应用平台（人审闭环贯穿）
Phase 3-C  ✅ Harness 深化：工具注册表 + Agent Loop + 记忆组装器（C 半步评测 input -47%/cache 74%）+ Skills + App 系统 + SkillHub 接入
Phase A    ✅ 应用×Agent 协作（context/JSON 回推/提提案）+ 应用空间 + AI 升级应用代码 + MCP server 出口
Phase 4    ✅ 三短板补齐：语义召回（grep+向量 RRF）+ 子 Agent（run_loop/ToolPolicy/spawn）+ 跨会话记忆（recall/extract/dream）
Phase 5    ▶ MCP 客户端：主动聚合第三方 MCP 服务与自研猎头 ERP（首个真实对接）；远程工具 `mcp__` 权限管控（默认拒绝+人工审批）；前端远程 MCP 管理面板
Phase 5    ○ 可选：任务断点持久化 / 多模型路由 / 行业技能包标准分发 / 团队协作 MCP 中枢
```

设计决策（App≠MCP App、统一 iframe 双源复用、SkillHub 信任分级、工具调用走宿主代理、自组织必须带人审、只消费远程 MCP 不暴露自身等）见 git 历史与代码注释。

## 目录

```
src/main.rs       托盘 + 服务线程 + --mcp 入口
src/server.rs     Axum 路由与接口
src/mcp.rs        MCP 薄壳（--mcp：stdio JSON-RPC + 工具映射）
src/agent.rs      Agent 回路 + 子 Agent（ToolPolicy 白名单）
src/search.rs     检索（ignore 遍历 + grep crate + 语义 RRF 融合）
src/graph.rs      知识图谱（SQLite documents/links、双链、路径 BFS、激活扩散）
src/llm.rs        LLM 代理（非流式 JSON + 流式 SSE 透传 + 联网通道）
src/risk.rs       律师风控预警（时效/证据/信息缺失，纯规则）
src/kb.rs         双层记忆布局 / frontmatter / INDEX / 待审机制
src/memory.rs     跨会话记忆（recall/extract/dream）
src/embed.rs      向量索引（可选通道，OpenAI 兼容 embeddings）
src/task.rs       任务引擎（kb/.tasks.db）
src/ingest.rs / fetch.rs / page.rs / heartbeat.rs / consolidate.rs / hub.rs / market.rs
web/              网页终端（Agent 回路 + 交互卡片）+ config.html
web/views/        内置面板（graph 沉浸画布 / automation 三栏目 / market 工作台 / board 看板 / home 首页）
kb/               L1 规范层 + L2 内容层（首次运行自动补齐）
tests/web/        前端 node:test 纯逻辑测试
scripts/          mock_llm.py / frontend-test.js / e2e.py / test.sh / verify-*.py
dist/             release 打包产物（双击即用）
```
