---
type: rule
scope: global
tags: [规范, frontmatter]
updated: 2026-08-02
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
