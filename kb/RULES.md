---
type: rule
scope: global
tags: [规范, frontmatter]
updated: 2026-08-09
---

# 规范

## Frontmatter
所有 MD 文件以 `---` 开头，字段：
```yaml
---
type: note | rule | memory | index
title: 标题
tags: [a, b]
updated: YYYY-MM-DD
---
```

## 命名
- 文件名：kebab-case，中文可用，空格用 `-` 代替
- 目录：按主题聚合，如 `notes/rag/`

## 写作
- 用标题分层（`#`/`##`），每篇先写一句话摘要
- 代码块标语言
- 外部链接与双链并用

## 记忆写入公理（四条）
1. **No Execution, No Memory**：未经验证的猜测禁止入记忆；执行/验证后才可写
2. **已验证数据神圣不可删**：重构时可压缩/迁移层级，禁止丢弃
3. **禁存易变状态**：时间戳/PID/临时路径等不写记忆
4. **最小充分指针**：索引层只放定位词与反直觉触发词，细节留给正文

分类决策树：环境特异事实 → `MEMORY.md`/`notes/`（L2）；通用规律 → 本文件 RULES（L1，一句）；特定任务技术 → `skills/`（L3 SOP）；通用常识 → 丢弃。
L1 同步规则：L2/L3 新增场景 → `MEMORY.md` 索引加行；删除 → 删行；索引只写关键词。
