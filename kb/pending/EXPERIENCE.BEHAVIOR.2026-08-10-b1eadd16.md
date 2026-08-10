---
type: experience
signal: tool_failed
date: 2026-08-10
---

# 经验提案

- 类型：behavior
- 信号：tool_failed
- 问题：工具 dev.read 因路径不在白名单内而失败（白名单：src/web/scripts、Cargo.toml、README.md、.zcode、plans），导致读取目标文件受阻。
- 改进：调用 dev.read 前先确认目标路径在白名单中；仅对白名单内路径发起读取，若需读取其他路径应改用白名单外工具（如通用 read），或先扩展白名单配置。
- 上下文：工具 dev.read 失败: 路径不在 dev 白名单内（src/web/scripts/Cargo.toml/README.md/.zcode/plans）
