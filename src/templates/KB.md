---
type: index
scope: global
updated: 2026-08-02
---

# KB 知识库入口

本库为**本地双层 MD 知识库**：
- **L1（本目录）**：规范 / 索引 / 记忆层。启动时注入 Agent 上下文，建立框架认知与检索指引。
- **L2（`notes/`）**：内容层。按需检索（内嵌 ripgrep），命中片段注入 Prompt。

## 文件指引
| 文件 | 职责 |
|---|---|
| [[FRAMEWORK]] | 框架：目录结构、双链约定 |
| [[RULES]] | 规范：frontmatter / 命名 / 写作 |
| [[MEMORY]] | 记忆：决策与偏好（增量追加） |
| [[INDEX]] | 内容索引（自动生成，勿手改） |

## 使用约定
- 检索：`/api/search?q=关键词`（智能大小写、多关键词任一命中）
- 同步：`POST /api/kb/sync` 重建 INDEX
- 新增知识：写入 `notes/`，Frontmatter 必须带 `type/tags/title`
