---
type: experience
signal: correction
date: 2026-08-05
---

# 经验提案

- 类型：memory
- 信号：correction
- 问题：测试隔离时错误使用MD_AGENT_KB环境变量，被纠正应使用MD_AGENT_CONFIG
- 改进：牢记测试隔离配置变量为MD_AGENT_CONFIG，而非MD_AGENT_KB，避免再次用错导致隔离失效。
- 上下文：不对，测试隔离应该用 MD_AGENT_CONFIG 而不是 MD_AGENT_KB
