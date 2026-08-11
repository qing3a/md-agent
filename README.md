# md-agent｜本地优先通用Agent运行底座

[![Rust](https://img.shields.io/badge/Rust-%E2%9C%93-orange?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![version](https://img.shields.io/badge/version-0.1.0-blue.svg)](https://github.com/qing3a/md-agent)

> 全本机数据存储、明文可审计双层Markdown知识库，原生兼容标准MCP协议，可无缝对接DeepSeek及各类主流推理服务，内置技能市场、任务编排、可视化网页终端，附带完整猎头行业私有化工作台落地方案

本地优先的通用Agent运行底座，三大重点：

- **明文可审计记忆**：知识库、会话、业务文件全部Markdown纯文本落盘，无黑盒向量存储；AI生成内容一律进待审，人工确认后才写入正式知识库
- **双向MCP生态**：stdio服务端向任意MCP客户端开放记忆/检索/图谱能力（已落地），客户端聚合第三方MCP服务与自研业务中台（stdio已落地，HTTP规划中）
- **模型零绑定**：标准OpenAI协议，DeepSeek、Ollama、各类商用推理端点随配随用

本地常驻托盘后台，数据不出本机。

## ✨ 生态能力总览（覆盖通用Agent六大核心赛道）

1. **MCP**：双向生态——本地stdio服务端（10类纯本地工具）+ 远程客户端（stdio聚合外部服务，HTTP传输规划中），可对接DeepSeek及全系列MCP兼容运行时
2. **Skill**：内置SkillHub技能市场，通用工具+猎头垂直可分发技能包
3. **Orchestrator**：带人审闭环的长任务编排引擎，长目标拆解、多轮工具循环
4. **Aggregator**：MCP客户端统一聚合多台远程MCP服务（第三方工具/自研猎头ERP）
5. **UI**：Rust托盘常驻+全套可视化网页终端（图谱/审批/技能/任务面板）
6. **行业垂直插件**：完整猎头HR私有化落地业务资产，企业级真实场景支撑

## 核心差异化优势

### 1. 明文双层MD持久记忆（独有底层设计）

摒弃黑盒向量库，L1规范/记忆索引层+L2业务正文分层存储；AI生成内容强制进入待审队列人工确认后入库，全文件原生可读、变更完整追溯，适配人力、政企敏感数据本地合规需求。

可独立作为本地记忆底座挂载至DeepSeek Harness，补齐原生知识库审计短板。

### 2. 成熟猎头行业完整落地资产（稀缺商用场景）

沉淀三套自研猎头SaaS完整业务设计底稿：21屏PM/猎头/候选人三角色界面、60+标准业务API、本地离线+多人远程同步冲突解决机制。

可快速搭建对接DeepSeek推理的私有化招聘工作台，提供真实长业务、多角色、跨服务生产测试负载。

## MCP双向完整能力

### 已完成：本地Stdio MCP薄壳（当前可用）

```bash
cargo run -- --mcp
```

stdio JSON-RPC向外暴露10类本地原生工具（检索/图谱/风控/任务/文档处理等），一行配置即可接入DeepSeek Harness、Cursor、各类遵循MCP标准的Agent客户端。

通用接入配置模板：

```json
"mcpServers": {
  "md-local-knowledge": {
    "command": "md-agent",
    "args": ["--mcp"]
  }
}
```

定位：作为独立本地记忆与工具层，推理逻辑交由DeepSeek或其他上层运行时负责，零模型绑定。

### 已完成：远程MCP客户端（stdio，Phase 5）

主动连接外部MCP服务，远程工具以`mcp__<服务>.<工具>`进Agent工具注册表，与本地工具同一生态：

- config.json `mcp_servers` 段注册外部服务（id/启动命令/参数/启停开关），懒启动连接（spawn+握手+工具清单），进程随宿主退出自动回收
- 远程工具由LLM声明调用、宿主统一执行并记审计，与本地工具同口径；服务连不上自动降级跳过，不阻塞整体
- 网页「远程MCP」面板（/view mcp）：服务添加/启停/测试连接/移除，连接状态与失败原因可视化

```json
"mcp_servers": [
  {"id": "erp", "name": "猎头ERP", "transport": "stdio",
   "command": "node", "args": ["mcp-server.js"], "enabled": true}
]
```

首个真实对接目标：自研猎头ERP（60端点API hub）。架构边界：**只消费远程、不对外暴露自身为远程服务**（单用户节点无暴露面，认证/运维负担为零）。

### 规划中：远程MCP客户端第二阶段（HTTP/SSE传输）

- 连接远端HTTP MCP服务（通用数据库/文档工具、团队协作中台），统一聚合多源工具集
- 远程工具权限管控强化：外部调用前置人工授权放行，延续人审安全设计

落地价值：

1. 单机可独立使用，团队协作时对接自研业务中台同步候选人、客户数据；
2. 聚合DeepSeek配套工具生态，扩展本地运行时能力边界；
3. 提供多人协作、跨服务调用、离线冲突等真实业务场景，可供DeepSeek生态做生产负载测试。

## 快速开始

### 1. 编译启动本地托盘服务

```bash
git clone https://github.com/qing3a/md-agent
cd md-agent
cargo build --release
./target/release/md-agent
```

浏览器访问`http://127.0.0.1:8756`进入可视化终端。

### 2. 接入推理服务

配置页支持填入DeepSeek API密钥、Ollama本地地址、其他商用推理接口，标准OpenAI协议全兼容。

首次启动三步引导可优先选择DeepSeek推理快速完成初始化。

### 3. 启用MCP本地能力

启动追加`--mcp`参数，即可在DeepSeek Harness等MCP客户端调用本机可审计知识库与全套本地工具链。

### 附加运行参数

`--no-tray`纯后台服务 / `--port`自定义端口 / `--mcp`开启MCP stdio出口

环境变量：`MD_AGENT_KB`、`MD_AGENT_PORT`、`MD_AGENT_CONFIG`

## 四大核心系统架构

```
常驻底座层 Rust托盘+Axum本地网页服务（127.0.0.1同源隔离）
记忆内核层 双层MD文件库/待审人审机制/自动索引图谱
工具编排层 检索/图谱/网页抓取/任务引擎/风控校验，统一宿主权限管控
交互生态层 网页可视化面板+内置iframe行业工作台+双向MCP（stdio服务端+客户端，HTTP规划中）
```

## 能力总览（精简，底层命令/目录/测试见文末折叠区）

### 记忆与知识

- 双层MD分层存储：L1规范/记忆/索引层+L2内容层，AI生成内容一律进待审、人工确认才落地，变更全程记录
- 会话自动归档检索（notes/会话归档/）；跨会话自动召回（grep+可选语义RRF双路）
- 全文检索（内嵌ripgrep）+SQLite双链知识图谱（实体类型化/路径探索/激活扩散），图谱可视化沉浸画布

### Agent任务编排

- 兼容DeepSeek等各类LLM流式输出，8轮上限标准ReAct工具循环，工具结果以交互卡片渲染、不污染上下文
- 子Agent（独立上下文+只读工具白名单+防递归）；任务引擎（目标自动拆解/状态机/依赖校验/任务看板）
- 自我开发工具链（AI读代码→改进提案→人审→应用/回滚）；文档批量摄入（PDF/Word/Excel→本地转MD）

### 应用工作台（行业沙箱）

- iframe隔离应用市场，原生搭载猎头/相亲/招聘三套行业应用，应用独立私有知识库
- 结构化通道与本地Agent交互（L0/L1/L2分级），与MCP能力完全解耦
- 单机无需联网、无需对接DeepSeek也可完整运行

### 常驻运维

- 系统托盘后台常驻，心跳自动索引同步+本地审计（孤立/悬空/重复）
- 多项目知识库硬隔离（kb/projects/），互不串用
- 网页抓取（/fetch静态、/page动态读取，人工确认后执行）

## 定位差异对比

| 产品 | md-agent | 通用向量RAG | IDE内置Agent | 云端对话WebUI |
|------|----------|-------------|--------------|---------------|
| 记忆存储 | 明文MD可审计，AI写入强制人审，可挂载DeepSeek做本地知识库 | 黑盒向量无人工管控 | 仅代码文件存储 | 数据存远端无本地库 |
| MCP生态 | 本地服务端+远程聚合客户端（stdio已落地），原生适配DeepSeek Harness | 极少MCP协议支持 | 私有协议绑定编辑器 | 无原生MCP互通 |
| 任务编排 | 带人审闭环长任务引擎，多轮工具循环，适配DeepSeek复杂业务推理 | 无完整工具循环 | 仅代码短流程 | 无本地持久调度 |
| 行业方案 | 完整猎头私有化工作台，可搭配DeepSeek搭建企业招聘AI | 无行业业务层 | 仅面向开发 | 无本地私有化方案 |
| 部署 | 100%本地优先，可选远程团队协同 | 本地/云端二选一 | 依赖IDE | 强制联网云端 |

## 界面预览

> 待补：知识图谱沉浸画布 / 技能市场 / 远程MCP管理面板 / 猎头工作台截图

## Roadmap 迭代规划

### 已完成

双层MD记忆（人审闭环）、网页可视化终端、任务编排引擎、应用沙箱平台、本地Stdio MCP服务端、远程MCP客户端（stdio）、全平台托盘打包。

### 规划中（生态拓展核心）

1. HTTP/SSE传输的远程MCP客户端：聚合第三方通用MCP服务、自研猎头ERP协作中台
2. 远程工具权限管控强化：外部调用前置人工授权
3. 多人团队协作场景：本地节点连接中央业务服务，提供长任务、跨服务调用、数据冲突等真实生产测试负载
4. 可选：任务断点持久化/多模型路由/行业技能包标准分发/团队协作MCP中枢

<details>
<summary>底层细节：双层记忆结构/终端命令速查/源码目录/开发测试/详细阶段（点击展开）</summary>

### 双层记忆结构

| 层 | 内容 | 记忆角色 | 进入上下文方式 |
|---|---|---|---|
| L1（kb根目录） | `KB.md`/`FRAMEWORK.md`/`RULES.md`/`MEMORY.md`/`INDEX.md` | 引导记忆（bootstrap memory，类CLAUDE.md） | 启动时注入Agent（只放"位置+要点"，正文进L2） |
| L2（`kb/notes/`） | 知识正文 | 内容记忆（retrievable memory） | 按需检索（grep），命中片段注入Prompt |

### 终端命令速查

```
# 检索与阅读
/search <关键词>         全文检索L2；/l1 <文件> 读L1规范/记忆
/graph <路径>            单篇图谱（出链/入链/相关）；/view graph 可视化
open <路径>              用系统编辑器打开文件
# 记忆写回与索引
/remember <内容>         手动沉淀；/digest <主题> 检索结果LLM整理成笔记
/sync /syncall /rescan   重建INDEX+图谱
# 待审与预览
/pending                 列出待审；/preview <路径> 行级预览；/approve /reject
# 面板与速览
/view graph|automation|market|board|home|sessions|config|mcp|off
# 自组织
/audit                   本地规则健康审计；/link-all 一键应用补链建议；/suggest 补全缺失主题
# 网页/文档摄入/任务
/fetch <url> /page <url> [/page act <json>]；附件按钮「＋」摄入文档
/task new|start|done|drop|plan <目标>；/task board
# 系统
/heartbeat 心跳开关 /config 配置 /clear 清空多轮记忆 /spaces 项目空间
```

### 开发测试

测试=harness代码层的"人审闭环"（人审保护记忆不被LLM污染，测试保护harness不被改动破坏）。**隔离铁律：所有测试用临时目录，绝不碰主kb。**

```bash
cargo test                 # Rust单测（177项：kb/graph/task/search/heartbeat/consolidate/agent/mcp_client…）
node --test tests/web/     # 前端node:test套件（core 20组+skills 6组）
python scripts/e2e.py      # E2E（37项断言）：隔离kb起服务，待审审批链路+Agent回路+语义召回+MCP客户端（mock stdio server）
```

`scripts/mock_llm.py`：OpenAI兼容mock（默认11434端口），支持流式，便于验证沉淀链路。
`scripts/mock_mcp.py`：stdio MCP mock服务，E2E验证远程客户端链路。

### 详细阶段

```
Phase 1-2  ✅ 底座+终端+LLM代理+Agent回路+知识图谱
Phase 3    ✅ 自组织工作流：审计/补链/巩固/技能/任务引擎/应用平台（人审闭环贯穿）
Phase 3-C  ✅ Harness深化：工具注册表+Agent Loop+记忆组装器（C半步评测input -47%/cache 74%）+Skills+App系统+SkillHub接入
Phase A    ✅ 应用×Agent协作（context/JSON回推/提提案）+应用空间+AI升级应用代码+MCP server出口
Phase 4    ✅ 三短板补齐：语义召回（grep+向量RRF）+子Agent（run_loop/ToolPolicy/spawn）+跨会话记忆（recall/extract/dream）
Phase 5    ✅ MCP客户端：主动连接外部MCP服务（stdio），远程工具mcp__进Agent生态+管理面板；HTTP/SSE传输第二阶段规划
```

设计决策（App≠MCP App、统一iframe双源复用、SkillHub信任分级、工具调用走宿主代理、自组织必须带人审、只消费远程MCP不暴露自身等）见git历史与代码注释。

### 源码目录

```
src/main.rs       托盘+服务线程+--mcp入口
src/server.rs     Axum路由与接口
src/mcp.rs        MCP薄壳（--mcp：stdio JSON-RPC+工具映射）
src/mcp_client.rs MCP客户端（Phase 5：stdio连接外部服务，mcp__工具合并注册表）
src/agent.rs      Agent回路+子Agent（ToolPolicy白名单）
src/search.rs     检索（ignore遍历+grep crate+语义RRF融合）
src/graph.rs      知识图谱（SQLite documents/links、双链、路径BFS、激活扩散）
src/llm.rs        LLM代理（非流式JSON+流式SSE透传+联网通道）
src/risk.rs       律师风控预警（时效/证据/信息缺失，纯规则）
src/kb.rs         双层记忆布局/frontmatter/INDEX/待审机制
src/memory.rs     跨会话记忆（recall/extract/dream）
src/embed.rs      向量索引（可选通道，OpenAI兼容embeddings）
src/task.rs       任务引擎（kb/.tasks.db）
src/ingest.rs/fetch.rs/page.rs/heartbeat.rs/consolidate.rs/hub.rs/market.rs
web/              网页终端（Agent回路+交互卡片）+config.html
web/views/        内置面板（graph沉浸画布/automation三栏目/market工作台/board看板/home首页/mcp远程服务管理）
kb/               L1规范层+L2内容层（首次运行自动补齐）
tests/web/        前端node:test纯逻辑测试
scripts/          mock_llm.py/mock_mcp.py/frontend-test.js/e2e.py/test.sh/verify-*.py
dist/             release打包产物（双击即用）
```

</details>

## 开源说明

MIT开源协议，完全遵循标准MCP、OpenAI兼容协议；既可以作为独立本地Agent运行，也可作为记忆/工具底座配套DeepSeek Harness及其他主流推理生态使用，无私有强制绑定。
