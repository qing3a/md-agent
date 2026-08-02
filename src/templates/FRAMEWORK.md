---
type: rule
scope: global
tags: [框架, 目录]
updated: 2026-08-02
---

# 框架

## 目录结构
- `KB.md` L1 入口，功能类似 CLAUDE.md：全局约定与文件指引
- `FRAMEWORK.md` / `RULES.md` / `MEMORY.md` L1 规范与记忆
- `INDEX.md` L1 索引（自动生成）
- `notes/` L2 内容层，按主题分子目录

## 双链
- 使用 `[[文件名]]` 在 MD 之间建立链接；链接目标是 L2 的文件名（不含 .md）

## 分层原则
- L1 只放「位置 + 要点」，正文进 L2；L1 保持小体积（几十 KB 内）
- 记忆与决策沉淀到 MEMORY.md，规范变更进 RULES.md
