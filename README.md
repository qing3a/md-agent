# md-agent — 本地双层 MD 知识库 Agent

以 **Markdown 为原生知识载体**、无向量库、纯文本检索、本地常驻托盘、网页终端交互的轻量自组织 AI 知识 Agent。

> 一句话定位：**知识人写人读，AI 只负责整理、关联、推演、生成。**

## 核心哲学

1. **知识人写人读**：原始知识永远是 `.md`，可用任意编辑器打开/修改/同步，永不锁库、永不黑盒。
2. **不用向量，用显性人类知识结构做关联**：`[[双向链接]]` + Frontmatter 元数据 + 目录项目层级。关联可审计、可追溯。
3. **双检索互补（不是碾压，是各管一段）**：
   - **ripgrep 全文检索** → 找片段、找关键词（已实现）
   - **SQLite 结构化图谱检索** → 找关联、找脉络、找项目体系（路线图 Phase 2）

## 当前能力（Phase 1 已完成 ✅）

| 能力 | 说明 |
|---|---|
| 双层知识库 | L1 规范/记忆/索引层（CLAUDE.md 模式，启动注入）+ L2 内容层（grep 检索）；`INDEX.md` 自动生成 |
| 全文检索 | 内嵌 ripgrep 内核（grep + ignore crate），多关键词任一命中、智能大小写、小节上下文（`section`/`context`） |
| **知识图谱** | SQLite `documents`/`links` 两表：`[[双向链接]]` 解析、反向链接、孤立文档检测、标签/项目维度统计；首次调用自动建库，`/rescan` 或托盘"同步索引"重建 |
| **伪命令行 Markdown 渲染** | 终端内 ANSI 富渲染（零依赖）：标题加粗、行内代码/加粗/链接/`[[双链]]` 着色、列表/引用/代码围栏；**表格按 markdown 行显示**（保留 `|` 结构，复制不失真）；frontmatter 变暗；流式回答按完整行渲染 |
| LLM 代理 | OpenAI 兼容（Ollama/DeepSeek 等），后端代理防 CORS 与密钥暴露；**SSE 流式透传** |
| Agent 问答回路 | 启动注入 L1 → 提取关键词 → 检索 L2 → 拼 Prompt → 流式回答 → `[文件:行号]` 引用 |
| 多轮对话记忆 | 会话内保留最近 4 轮（`/clear` 清空） |
| 写回沉淀 | LLM 回答附 `<!-- md-agent-save -->` 块自动落盘：新知识写 L2（自动补 frontmatter）、决策写 L1 MEMORY；`/remember` 手动沉淀 |
| `/digest` | 检索结果交给 LLM 整理成结构化笔记写入 `notes/` |
| 可视化配置页 | `/config.html`：endpoint / model / api_key（掩码显示）+ 测试连接 |
| 托盘常驻 | tray-icon + winit，右键菜单：打开终端 / 同步索引 / 退出；release 单 exe 隐藏控制台 |

## 架构（四层）

```
常驻底座层    Rust 托盘常驻 + Axum 本地服务（127.0.0.1，同源托管前端免 CORS）
知识内核层    双层 MD 布局 / frontmatter 解析 / INDEX 自动生成 / 路径安全
检索引擎层    ignore 遍历 + grep crate（内嵌 ripgrep），行级小节切分
交互层        xterm.js 网页终端（Agent 回路 + 管理命令 + 配置页）
```

## 双层结构

| 层 | 内容 | 进入上下文方式 |
|---|---|---|
| L1（kb 根目录） | `KB.md` / `FRAMEWORK.md` / `RULES.md` / `MEMORY.md` / `INDEX.md` —— 规范、记忆、索引 | 启动时注入 Agent（常驻，只放"位置+要点"，正文进 L2） |
| L2（`kb/notes/`） | 知识正文 | 按需检索（grep），命中片段注入 Prompt |

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
/clear                清空多轮记忆
/config               查看配置（掩码）
```

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
| POST | `/api/graph/sync` | 重建知识图谱（SQLite） |
| GET | `/api/graph/stats` | 图谱统计（文档/链接/解析/悬空/孤立/项目） |
| GET | `/api/graph/backlinks?path=` | 反向链接（谁链向该文档） |
| GET | `/api/graph/linked?path=` | 出链（该文档链向谁，含悬空标记） |
| GET | `/api/graph/related?path=` | 关联簇（出链+入链去重） |
| GET | `/api/graph/orphans` | 孤立文档（无入链也无出链） |
| GET | `/api/graph/tags` | 标签统计 |
| GET | `/api/graph/projects` | 项目维度统计 |
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
Phase 3  ▶ 自组织工作流（必须带人工审核闭环）
         ├─ Agent 自动补链接 / 补文档 / 盲区与冲突检测
         ├─ 知识生成走「生成 → 预览 → 确认」或待审目录，防 LLM 污染知识库
         └─ 延伸：Agent 可动态生成 HTML 面板 App（人审通过后安装）——自组织的应用层表现
可选前置：面板渲染层（不依赖自组织，可提前单独做）
         ├─ /view <html/目录>：iframe 沙箱渲染本地 HTML，postMessage 桥调宿主 API
         ├─ 通信复用现有 HTTP/SSE，不引入 WebSocket
         └─ 首个内置视图：知识图谱可视化（/graph 文本输出升级为可视化面板）
Phase 4  生态化（可选，与"轻量"定位有张力，个人场景可长期搁置）
         ├─ MCP 客户端（Stdio/SSE）；兼容标准 MCP App 渲染（复用统一 iframe 渲染层）
         ├─ App 系统：manifest + 安装/启用/运行/关闭/卸载 + 权限声明（本地导入即可）
         └─ WASM 计算后端（仅当出现"本地运行不可信计算"的真实需求）
```

**设计决策**（含 2026-08 对「Host App / WASM 插件」方案评审结论）：

- **概念界定：App ≠ MCP App**。本项目的"应用"= 一体化应用包（HTML 界面 + 业务逻辑，可安装/启用/运行/关闭/卸载、可上架插件市场）；MCP App 只是依附外部 MCP 进程的 UI 片段，无独立生命周期。混淆两者会把 MCP 误当插件系统来设计。
- **统一 iframe 渲染层，双源复用**：只维护一套 iframe 沙箱组件，同时渲染「本地 HTML 面板」与「未来兼容的标准 MCP App」；数据请求一律 postMessage 转发给宿主鉴权，前端不直连数据。
- **App 逻辑不强上 WASM**：检索/图谱/文件逻辑已在 Rust 宿主，App 的"深度接入"= 调宿主 API（manifest 声明权限即可）；HTML + 宿主 API 代理覆盖 90% 场景，WASM 仅保留为可选计算后端，等出现真实需求再引入。
- **通信复用现有传输**：iframe 用 postMessage 与父页面通信，父页面走现有 HTTP/SSE；不引入 WebSocket。
- **应用包 = 文件夹 / zip**，不造自定义归档格式（.hax 之类）；市场只做本地导入（拖拽离线安装），不做在线市场——个人场景无多应用供给，在线市场属生态叙事。
- **自组织必须带人工审核**：LLM 幻觉/错误关联会污染图谱，"可审计"是这套架构的立身之本；Agent 动态生成 App 同理，人审后安装，不绕开审核闭环。

## 已知短板与边界

- 检索无语义召回：换种说法问可能搜不到（Phase 2 图谱缓解，但语义召回仍需向量，当前定位明确不做）
- 多轮记忆仅会话内，重启即清（如需跨会话持久化，可把历史写入 kb）
- `config.json` 的 `kb_root` 字段为装饰性，服务实际按 `MD_AGENT_KB` > `./kb` > exe 旁 `kb` 解析
- 写回默认自动落盘、无人工确认（Phase 3 待审机制的前置）
- 托盘图标为代码生成的占位方块（正式图标待换）
- 关键词提取为启发式（无真分词），英文/数字效果好于中文长句
- 终端表格不做对齐（避免 CJK 宽度计算与流式缓冲），需要整齐表格可 `open` 后用支持表格的编辑器查看原 markdown
- `/view` 面板渲染与 App 系统尚未实现（见路线图「可选前置」与 Phase 4，当前先聚焦 Phase 3 自组织）

## 与同类产品的定位差异

| 对比 | 差异（互补，非碾压） |
|---|---|
| 向量 RAG | 本架构可审计、可追溯、文件即库；代价是无语义召回 |
| Obsidian/Notion | 本架构可编程 Agent、可工具调用、可自动化；缺其插件生态 |
| ClaudeCode/Cursor | 本架构专注文档体系治理、轻量无 IDE 绑定；无代码执行能力 |
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
src/kb.rs     双层布局 / frontmatter 解析 / INDEX 自动生成 / 路径安全
src/config.rs 本地配置
web/          xterm.js 终端前端（Agent 回路 + 管理命令）+ config.html 配置页
kb/           L1 规范层 + L2 内容层（首次运行自动补齐模板）
scripts/      mock_llm.py 开发测试工具
dist/         release 打包产物（双击即用）
```
