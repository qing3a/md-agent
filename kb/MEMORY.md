---
type: memory
scope: global
tags: [记忆]
updated: 2026-08-02
---

# 记忆

> 记录用户偏好、已做决策与踩坑。增量追加，越新越靠上。

## 2026-08-02 项目启动
- 定案：双层结构 = L1 规范/索引/记忆层（CLAUDE.md 模式）+ L2 内容层（grep 检索）
- 检索引擎：内嵌 ripgrep（grep + ignore crate），无向量库
- LLM：暂不接入，后续走后端代理（防 CORS / 密钥暴露）

## 2026-08-03
- 编辑后内容 v2

## 2026-08-03
- 测试记忆条目一

## 2026-08-07
- 经验提案

- 类型：memory
- 信号：correction
- 问题：测试隔离时错误使用MD_AGENT_KB环境变量，被纠正应使用MD_AGENT_CONFIG
- 改进：牢记测试隔离配置变量为MD_AGENT_CONFIG，而非MD_AGENT_KB，避免再次用错导致隔离失效。
- 上下文：不对，测试隔离应该用 MD_AGENT_CONFIG 而不是 MD_AGENT_KB
