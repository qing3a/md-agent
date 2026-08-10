# md-agent — 本地 Agent Harness：双层 MD 记忆 + 工具链 + 人审闭环 + MCP 出口

本地常驻的 Agent Harness：托盘底座 + 网页终端 + LLM 代理 + 检索/图谱/网页/任务工具链。记忆以 Markdown 纯文本落盘——可审计、可追溯、永不锁库。

> 一句话：**知识人写人读，AI 只负责整理、关联、推演、生成。**
> 本质：Agent 的运行层（harness）——双层 MD 记忆是核心子系统（正式记忆人审固化 + 待审草稿必经人审）；检索是读取手段，知识正文永远是 Markdown。

## 核心亮点

1. **双层 MD 持久记忆（非向量）**：L1 规范/记忆/索引 + L2 正文检索（ripgrep），可读可审计；会话归档检索化、记忆写入人审闭环——不是黑盒向量库
2. **自组织迭代**：应用/系统基于使用反馈自己提改进提案 → 人审 → 自动应用（备份/构建/回滚）——完整的人审守门闭环
3. **MCP 薄壳（`--mcp`）**：stdio JSON-RPC 暴露 10 个纯本地工具（检索/记忆/图谱/风控/待审/任务），Claude Code / DeepSeek Harness / Cursor 一行配置接入——md-agent 当「知识/记忆/风控层」，推理由调用方负责（零 LLM 依赖）
4. **应用平台**：3 个真实行业应用（猎头/相亲评估/招聘工作台）+ 应用空间（私有知识层）+ 应用×Agent 协作（agent:ask 通道，context 结构化入参 + 结果 JSON 回推）

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

### 检索与图谱

- **全文检索**：内嵌 ripgrep，多关键词任一命中、智能大小写、小节上下文
- **知识图谱**：SQLite 双链（documents/links），反链/孤立/标签/项目维度；`/sync`、`/rescan` 或心跳自动重建
- **图谱可视化（/view graph）**：沉浸画布 + 情境抽屉——树抽屉（可关）+ 详情抽屉（详情/路径/健康标签页）+ 底部过滤条（渲染级过滤不丢布局）；单击聚焦、双击重布局、⛶ 全图；hover 邻域高亮、拖拽固定、路径链橙色；**思源式增强**：目录节点（结构边虚线）+ 标签节点（标签边紫色）进图；UI 状态持久化；canvas 颜色随主题
- **关系探索**：A→B 最短路径（≤6 跳）——图谱面板 + 对话流路径卡（`graph.paths` 工具）

### Agent 与工具

- **LLM 代理**：OpenAI 兼容（Ollama/DeepSeek 等），后端代理防 CORS 与密钥暴露；SSE 流式透传；联网通道（web_search，触发词或检索 0 命中自动开）
- **Agent 问答回路**：LLM 显式调工具（声明式清单：search/read_l1/memory_search/graph/risk.check/fetch/page/file/tasks/pending.list/dev 工具链）→ 宿主执行回填 → 循环（上限 8 轮，强制回答轮兜底）→ 流式回答 → `[文件:行号]` 引用可点击跳图谱
- **交互卡片**：工具结果旁路渲染对话流卡片（风控/待审/路径链/任务/链接卡，按钮直接操作，不污染 LLM 上下文）
- **自我开发工具链**：dev.read/status/diff/patch/apply——AI 读自己代码、生成改进提案（进待审人审）、应用（备份/构建/回滚）
- **任务引擎**：`kb/.tasks.db` 独立 SQLite，状态机/依赖校验/推进日志；`/task` 终端看板 + `/task board` HTML 看板；`/task plan` LLM 目标拆解

### 应用平台（工作台）

- **工作台/应用市场**：`kb/apps/<id>/` 单文件 HTML 应用，沙箱 iframe + manifest 权限白名单（llm/storage/agent/search/graph/file/write…）；安装走 `/market import|install`（dry_run 人审）；SkillHub 索引连接；侧边栏「工作台」子菜单展示已装应用
- **应用 × Agent 协作**：应用委托宿主 agent 全回路（agent:ask：context 结构化入参、结果 JSON 约定标记回推、应用任务授权）；三应用已接入（猎头助手 L2 / 相亲评估 L2 / 招聘工作台 L0）
- **应用空间**：每应用私有知识层 `kb/apps/<id>/notes/`（agent:ask space:true 注入摘要）；桥写文件限定自己目录（防越权）；排除出主库检索/图谱/心跳
- **AI 升级应用代码**：应用内「改进应用」→ agent 读自己代码 → dev.patch 提案（限定自己目录）→ 人审 → dev.apply（纯应用文件跳过构建）

### MCP 出口

- **`md-agent --mcp`**：stdio JSON-RPC（MCP 协议），10 个纯本地工具——MCP 客户端一行配置接入（Claude Code：`claude mcp add md-agent -- md-agent --mcp`）；md-agent 当知识/记忆/风控层，推理归调用方（零 LLM 依赖，进程在即可用）

### 常驻与运维

- **托盘常驻**：右键菜单（打开终端/应用市场/同步/已安装应用/面板导航/心跳开关/Key 设置/退出）；release 单 exe 隐藏控制台
- **心跳自动同步**：默认关；开启后周期指纹比对，变化自动重建 INDEX+图谱并跑本地审计（孤立/悬空/重复）
- **网页能力**：`/fetch` 静态抓取；`/page` 动态读取（headless Edge/Chrome）；`/page act` 写侧（click/fill/select，人工确认后执行）
- **文档摄入**：PDF/DOCX/PPT/XLS/CSV/EPUB → anydoc 本地转 Markdown（dry-run 预览 → 确认）→ 落 `notes/` 自动重建索引
- **项目空间**：多项目硬隔离（`kb/projects/<id>/` 独立迷你知识库，检索/会话/记忆/图谱绝不串用）；新建向导三模板（空白/律师案件/猎头项目）

## 架构（四层）

```
常驻底座层    Rust 托盘常驻 + Axum 本地服务（127.0.0.1，同源托管前端免 CORS）
记忆内核层    双层 MD 记忆布局（L1 规范/记忆/索引 + L2 内容）/ frontmatter 解析 / INDEX 自动生成 / 待审机制 / 路径安全
工具链层      检索（ignore + grep crate）、图谱（SQLite 双链）、网页（/fetch /page act）、任务（tasks.db）——宿主 API 统一鉴权，LLM 不直连
交互层        网页终端（Agent 回路 + 管理命令 + 配置页）+ /view iframe 面板渲染层 + 对话流交互卡片 + MCP stdio 出口
```

## 双层记忆结构

| 层 | 内容 | 记忆角色 | 进入上下文方式 |
|---|---|---|---|
| L1（kb 根目录） | `KB.md` / `FRAMEWORK.md` / `RULES.md` / `MEMORY.md` / `INDEX.md` | 引导记忆（bootstrap memory，类 CLAUDE.md） | 启动时注入 Agent（只放"位置+要点"，正文进 L2） |
| L2（`kb/notes/`） | 知识正文 | 内容记忆（retrievable memory） | 按需检索（grep），命中片段注入 Prompt |

## 定位差异

| 对比 | 差异（互补，非碾压） |
|---|---|
| 向量 RAG | 本架构可审计、可追溯、文件即库；代价是无语义召回 |
| Obsidian/Notion | 本架构可编程 Agent、可工具调用、可自动化；缺其插件生态 |
| ClaudeCode/Cursor | 同为 Agent Harness；本架构专精「记忆 + 审核闭环」+ MCP 记忆层出口；缺其代码执行/IDE 深度 |
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
cargo test                 # Rust 单测（kb/graph/task/search/heartbeat/consolidate/…）
node --test tests/web/     # 前端 node:test 套件（core 20 组 + market 2 组）
python scripts/e2e.py      # E2E：隔离 kb 起服务，跑待审四型审批链路
```

`scripts/mock_llm.py`：OpenAI 兼容 mock（默认 11434 端口），支持流式，便于验证沉淀链路。

## 迭代路线（当前状态）

```
Phase 1-2  ✅ 底座 + 终端 + LLM 代理 + Agent 回路 + 知识图谱
Phase 3    ✅ 自组织工作流：审计/补链/巩固/技能/任务引擎/应用平台（人审闭环贯穿）
Phase 3-C  ✅ Harness 深化：工具注册表 + Agent Loop + 记忆组装器（C 半步评测 input -47%/cache 74%）+ Skills + App 系统 + SkillHub 接入
Phase A    ✅ 应用×Agent 协作（context/JSON 回推/提提案）+ 应用空间 + AI 升级应用代码 + MCP server 出口
Phase 4    ✅ 三短板补齐：语义召回（embed.rs/vector.rs，grep+向量 RRF k=60）+ 子 Agent（agent.rs run_loop/ToolPolicy/spawn，/api/agent + MCP agent.spawn）+ 跨会话记忆（memory.rs recall/extract/dream，提问自动召回 + 收尾提炼，写回全走 pending 人审）
Phase 4    ▶ 剩余可选项：MCP 客户端（接第三方工具）/ 兼容标准 MCP App 渲染 / WASM 计算后端 / 向量 ANN 索引 / /api/agent SSE 流式化
```

设计决策（App≠MCP App、统一 iframe 双源复用、SkillHub 信任分级、工具调用走宿主代理、自组织必须带人审等）见 git 历史与代码注释。
原已知短板（无语义召回、无子 agent、多轮记忆仅会话内）已在 Phase 4 补齐：语义召回为可选通道（llm.embedding 未配置自动降级纯 grep，零破坏）；子 agent 由 ToolPolicy 硬白名单防越权/防递归；跨会话记忆的 extract/dream 产物一律进 pending 人审后落地（recall 只读）。

## 目录

```
src/main.rs       托盘 + 服务线程 + --mcp 入口
src/server.rs     Axum 路由与接口
src/mcp.rs        MCP 薄壳（--mcp：stdio JSON-RPC + 工具映射）
src/search.rs     检索（ignore 遍历 + grep crate）
src/graph.rs      知识图谱（SQLite documents/links、双链、路径 BFS）
src/llm.rs        LLM 代理（非流式 JSON + 流式 SSE 透传 + 联网通道）
src/risk.rs       律师风控预警（时效/证据/信息缺失，纯规则）
src/kb.rs         双层记忆布局 / frontmatter / INDEX / 待审机制
src/task.rs       任务引擎（kb/.tasks.db）
src/ingest.rs / fetch.rs / page.rs / heartbeat.rs / consolidate.rs / hub.rs / market.rs
web/              网页终端（Agent 回路 + 交互卡片）+ config.html
web/views/        内置面板（graph 沉浸画布 / automation 三栏目 / market 工作台 / board 看板 / home 首页）
kb/               L1 规范层 + L2 内容层（首次运行自动补齐）
tests/web/        前端 node:test 纯逻辑测试
scripts/          mock_llm.py / frontend-test.js / e2e.py / test.sh
dist/             release 打包产物（双击即用）
```
