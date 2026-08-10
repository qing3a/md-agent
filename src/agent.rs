//! Agent 回路（Phase 4 M2：把 Agent Loop 下沉到 Rust 侧）。
//! 参考 ZCode `runMemoryAgentLoop` 的结构与 md-agent 现有前端回路语义：
//! - 主回路 run_loop：LLM 生成 → 工具调用 → 宿主执行回填 → 循环（上限轮次，强制回答轮兜底）；
//! - 子 agent 复用同一 run_loop：独立 messages 上下文 + 受限工具白名单（ToolPolicy）；
//! - LLM 调用与工具执行通过 trait 抽象（LlmPort / ToolExecutor）——mock 可测、零网络依赖。
//!
//! 工具调用协议沿用现有前端「软工具调用」：LLM 在回答中输出一行 JSON `{"tool": "...", "args": {...}}`，
//! 宿主解析后执行；解析逻辑由调用方提供（复用 Core.extractJsonObjects 同构的 Rust 解析）。

use futures_util::future::BoxFuture;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 会话消息（与前端 messages 同构）
#[derive(Debug, Clone, PartialEq)]
pub struct AgentMessage {
    pub role: String,
    pub content: String,
}

impl AgentMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: content.into() }
    }
}

/// LLM 一轮回复：answer 为空 = 工具轮（tool 有值）；answer 非空 = 最终回答
#[derive(Debug, Clone)]
pub struct LlmReply {
    /// 文本回答（工具轮可为空；reasoning 归入 answer 由调用方决定）
    pub answer: String,
    /// 软工具调用（LLM 输出的 JSON，如 {"tool":"search","args":{"q":"..."}}）
    pub tool: Option<ToolCall>,
}

/// 软工具调用（LLM 声明式，与 /api/tools 契约一致）
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub args: Value,
}

/// LLM 端口抽象：调用方实现（server 路由接 crate::llm；测试接 mock）
pub trait LlmPort {
    fn call<'a>(&'a self, messages: &'a [AgentMessage]) -> BoxFuture<'a, Result<LlmReply, String>>;
}

/// 工具执行端口抽象：调用方实现（server 路由接 crate::search/kb/graph 等；测试接 mock）
pub trait ToolExecutor {
    fn exec<'a>(&'a self, name: &'a str, args: &'a Value) -> BoxFuture<'a, Result<String, String>>;
}

/// 回路结果
#[derive(Debug, Clone)]
pub struct AgentResult {
    pub answer: String,
    /// 实际执行的工具调用次数（被策略拒绝的不计入）
    pub tool_calls: usize,
    /// 执行过的工具名（审计/交互卡片用）
    pub tools_used: Vec<String>,
}

// ---------- 工具策略（子 agent 受限白名单，对齐 ZCode evaluateMemoryAgentToolPolicy） ----------

/// 子 agent 只读工具白名单（全部映射到 KB 内只读端点）
pub const READ_ONLY_TOOLS: &[&str] = &[
    "search",
    "memory_search",
    "read_l1",
    "read_file",
    "graph.linked",
    "graph.backlinks",
    "graph.paths",
    "risk.check",
    "tasks",
    "pending.list",
];

/// 受限写工具（仅 .md 且 resolve 后仍在 KB 根内）
pub const WRITE_MD_TOOLS: &[&str] = &["write_file", "edit_file"];

/// 工具策略：硬白名单校验（防越权/防递归/防路径穿越）。
/// 拒绝一律返回理由（回填给 LLM 让它调整），不静默。
#[derive(Debug, Clone)]
pub struct ToolPolicy {
    /// KB 根（路径校验基准；子 agent 只允许操作本库内）
    pub root: PathBuf,
}

impl ToolPolicy {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn evaluate(&self, name: &str, args: &Value) -> Result<(), String> {
        // 防递归 + 防转手 MCP：子 agent 内禁止再派生 agent / 调用 MCP 工具
        if name == "agent.spawn" || name == "Agent" || name.starts_with("mcp__") {
            return Err(format!(
                "工具 {name} 被策略拒绝：子 agent 内禁止派生子 agent 或转手 MCP 工具（防递归）"
            ));
        }
        if READ_ONLY_TOOLS.contains(&name) {
            // 含路径参数的只读工具做路径校验（防穿越读）
            if name == "read_file" {
                let p = args.get("path").and_then(Value::as_str).unwrap_or("");
                if self.resolve_md(p).is_none() {
                    return Err(format!(
                        "工具 read_file 被策略拒绝：路径越界或非 .md：{p}"
                    ));
                }
            }
            return Ok(());
        }
        if WRITE_MD_TOOLS.contains(&name) {
            let p = args.get("file_path").and_then(Value::as_str).unwrap_or("");
            if self.resolve_md(p).is_none() {
                return Err(format!(
                    "工具 {name} 被策略拒绝：仅允许 KB 根内 .md 文件（路径越界：{p}）"
                ));
            }
            return Ok(());
        }
        Err(format!(
            "工具 {name} 被策略拒绝：不在子 agent 白名单（仅只读工具 + 受限 .md 写）"
        ))
    }

    /// 路径必须能以 .md 结尾且 resolve 后仍在 KB 根内（复用 kb::resolve_in_kb 防穿越）
    fn resolve_md(&self, rel: &str) -> Option<PathBuf> {
        if !rel.ends_with(".md") {
            return None;
        }
        crate::kb::resolve_in_kb(&self.root, rel)
    }
}

// ---------- 主回路 ----------

/// 回填工具结果的 user 消息（与前端 3014 行语义一致，方向 4 摘要注入）
fn tool_result_message(name: &str, result: &str) -> AgentMessage {
    let body: String = result.chars().take(3000).collect();
    AgentMessage::user(format!(
        "工具 {name} 返回（基于它直接回答；仍缺关键信息才可再调用工具，引用标注 [工具:{name}]）：\n{body}"
    ))
}

/// 工具被策略拒绝时的回填消息（LLM 据此调整策略）
fn tool_rejected_message(name: &str, reason: &str) -> AgentMessage {
    AgentMessage::user(format!(
        "工具 {name} 被策略拒绝：{reason}\n请基于已获取信息回答，或改用白名单内工具（只读：search/memory_search/read_l1/read_file/graph.*/risk.check/tasks/pending.list；受限写：write_file/edit_file 仅 .md）。"
    ))
}

/// 达上限后的强制回答轮（去掉工具调用指令，防 LLM 无限探索不收敛）
fn forced_answer_message() -> AgentMessage {
    AgentMessage::user("请基于上述工具返回直接给出最终回答，不要调用任何工具。")
}

/// Agent 主回路：LLM 生成 → 工具调用 → 策略校验 → 执行回填 → 循环。
/// - 无工具调用 → 返回最终回答；
/// - 工具调用被策略拒绝 → 拒绝理由回填（不执行），继续循环；
/// - 达到 max_turns → 强制回答轮兜底；
/// - 子 agent 与主 agent 共用本函数（子 agent 传受限 policy + 独立 messages）。
pub async fn run_loop<L: LlmPort + ?Sized, E: ToolExecutor + ?Sized>(
    llm: &L,
    exec: &E,
    policy: &ToolPolicy,
    mut messages: Vec<AgentMessage>,
    max_turns: usize,
) -> Result<AgentResult, String> {
    let mut tool_calls = 0usize;
    let mut tools_used: Vec<String> = Vec::new();
    for _ in 0..max_turns {
        let reply = llm.call(&messages).await?;
        let Some(tc) = reply.tool else {
            return Ok(AgentResult {
                answer: reply.answer,
                tool_calls,
                tools_used,
            });
        };
        match policy.evaluate(&tc.name, &tc.args) {
            Ok(()) => {
                let result = exec.exec(&tc.name, &tc.args).await?;
                messages.push(AgentMessage::assistant(reply.answer));
                messages.push(tool_result_message(&tc.name, &result));
                tool_calls += 1;
                tools_used.push(tc.name.clone());
            }
            Err(reason) => {
                messages.push(AgentMessage::assistant(reply.answer));
                messages.push(tool_rejected_message(&tc.name, &reason));
            }
        }
    }
    // 达上限：强制回答轮（不调用工具）
    messages.push(forced_answer_message());
    let reply = llm.call(&messages).await?;
    Ok(AgentResult {
        answer: reply.answer,
        tool_calls,
        tools_used,
    })
}

// ---------- 子 agent（独立上下文 + 受限策略，防递归） ----------

/// 子 agent 规格：独立上下文（seed + 指令），与父 agent 完全隔离。
#[derive(Debug, Clone)]
pub struct SubagentSpec {
    /// 子任务指令（作为 user 消息追加到独立上下文末尾）
    pub prompt: String,
    /// 独立上下文 seed（如 L1 规范/上下文摘要注入；父 agent 消息绝不透传）
    pub seed: Vec<AgentMessage>,
    /// 子循环轮次上限（默认 8）
    pub max_turns: usize,
}

impl Default for SubagentSpec {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            seed: Vec::new(),
            max_turns: 8,
        }
    }
}

/// 子 agent 执行：独立 messages + 受限 ToolPolicy（只读 + 受限 .md 写）。
/// 防递归由策略层硬保证：子 agent 的 policy 拒绝 agent.spawn/Agent/mcp__*，
/// 子 agent 内无法再派生 agent；结果以文本返回父级（父级按需解析）。
pub async fn spawn_subagent<L: LlmPort + ?Sized, E: ToolExecutor + ?Sized>(
    llm: &L,
    exec: &E,
    root: &Path,
    spec: SubagentSpec,
) -> Result<AgentResult, String> {
    let policy = ToolPolicy::new(root.to_path_buf());
    let mut messages = spec.seed;
    messages.push(AgentMessage::user(spec.prompt));
    run_loop(llm, exec, &policy, messages, spec.max_turns).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn tmp_kb(name: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("md-agent-agent-test-{name}-{n}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("notes")).unwrap();
        d
    }

    // 可编程 mock：按序返回预设回复；记录执行过的工具
    struct MockLlm {
        replies: Mutex<VecDeque<Result<LlmReply, String>>>,
        calls: Mutex<Vec<usize>>, // 每次调用收到的消息数（审计用）
    }
    impl MockLlm {
        fn new(replies: Vec<Result<LlmReply, String>>) -> Self {
            Self {
                replies: Mutex::new(replies.into()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }
    impl LlmPort for MockLlm {
        fn call<'a>(&'a self, messages: &'a [AgentMessage]) -> BoxFuture<'a, Result<LlmReply, String>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(messages.len());
                self.replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(Ok(LlmReply {
                        answer: "(mock 回复耗尽)".into(),
                        tool: None,
                    }))
            })
        }
    }

    #[derive(Default)]
    struct MockExec {
        called: Mutex<Vec<(String, Value)>>,
    }
    impl ToolExecutor for MockExec {
        fn exec<'a>(&'a self, name: &'a str, args: &'a Value) -> BoxFuture<'a, Result<String, String>> {
            Box::pin(async move {
                self.called.lock().unwrap().push((name.to_string(), args.clone()));
                Ok(format!("mock 结果 for {name}"))
            })
        }
    }

    fn tool(name: &str, args: Value) -> LlmReply {
        LlmReply {
            answer: String::new(),
            tool: Some(ToolCall {
                name: name.to_string(),
                args,
            }),
        }
    }

    #[test]
    fn direct_answer_no_tools() {
        let llm = MockLlm::new(vec![Ok(LlmReply {
            answer: "直接回答".into(),
            tool: None,
        })]);
        let exec = MockExec::default();
        let policy = ToolPolicy::new(tmp_kb("direct"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(run_loop(
            &llm,
            &exec,
            &policy,
            vec![AgentMessage::user("你好")],
            8,
        )).unwrap();
        assert_eq!(r.answer, "直接回答");
        assert_eq!(r.tool_calls, 0);
        assert!(exec.called.lock().unwrap().is_empty());
    }

    #[test]
    fn one_tool_then_answer() {
        let llm = MockLlm::new(vec![
            Ok(tool("search", json!({"q": "记忆 分片"}))),
            Ok(LlmReply { answer: "查到了".into(), tool: None }),
        ]);
        let exec = MockExec::default();
        let policy = ToolPolicy::new(tmp_kb("onetool"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(run_loop(
            &llm,
            &exec,
            &policy,
            vec![AgentMessage::user("查一下")],
            8,
        )).unwrap();
        assert_eq!(r.answer, "查到了");
        assert_eq!(r.tool_calls, 1);
        assert_eq!(r.tools_used, vec!["search"]);
        let called = exec.called.lock().unwrap();
        assert_eq!(called[0].0, "search");
        assert_eq!(called[0].1, json!({"q": "记忆 分片"}));
    }

    #[test]
    fn policy_rejects_unknown_tool_with_reason() {
        // 非白名单工具（fetch=网络侧效应）→ 拒绝回填，不执行，继续循环后正常回答
        let llm = MockLlm::new(vec![
            Ok(tool("fetch", json!({"url": "https://example.com"}))),
            Ok(LlmReply { answer: "被拒绝后回答".into(), tool: None }),
        ]);
        let exec = MockExec::default();
        let policy = ToolPolicy::new(tmp_kb("reject"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(run_loop(
            &llm,
            &exec,
            &policy,
            vec![AgentMessage::user("抓网页")],
            8,
        )).unwrap();
        assert_eq!(r.answer, "被拒绝后回答");
        assert_eq!(r.tool_calls, 0, "被拒绝的工具不计数");
        assert!(exec.called.lock().unwrap().is_empty(), "拒绝的工具不得执行");
        // 回填消息里带拒绝理由（LLM 可见）
        let calls = llm.calls.lock().unwrap();
        assert!(calls.len() >= 2);
    }

    #[test]
    fn policy_rejects_spawn_recursion() {
        let llm = MockLlm::new(vec![
            Ok(tool("agent.spawn", json!({"prompt": "x"}))),
            Ok(LlmReply { answer: "ok".into(), tool: None }),
        ]);
        let exec = MockExec::default();
        let policy = ToolPolicy::new(tmp_kb("spawn"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(run_loop(
            &llm,
            &exec,
            &policy,
            vec![AgentMessage::user("再派一个")],
            8,
        )).unwrap();
        assert_eq!(r.tool_calls, 0);
        assert!(exec.called.lock().unwrap().is_empty());
    }

    #[test]
    fn policy_rejects_path_traversal_write() {
        // write_file 穿越根外 → 拒绝（resolve_in_kb 防穿越）
        let d = tmp_kb("traversal");
        let policy = ToolPolicy::new(d.clone());
        let ok = policy.evaluate("write_file", &json!({"file_path": "notes/好.md"}));
        assert!(ok.is_ok());
        let bad = policy.evaluate("write_file", &json!({"file_path": "../outside.md"}));
        assert!(bad.is_err(), "穿越路径必须拒绝: {bad:?}");
        let not_md = policy.evaluate("write_file", &json!({"file_path": "notes/坏.txt"}));
        assert!(not_md.is_err(), "非 .md 必须拒绝");
        let no_arg = policy.evaluate("write_file", &json!({}));
        assert!(no_arg.is_err());
    }

    #[test]
    fn policy_read_file_requires_md_in_kb() {
        let d = tmp_kb("readfile");
        let policy = ToolPolicy::new(d.clone());
        assert!(policy.evaluate("read_file", &json!({"path": "notes/甲.md"})).is_ok());
        assert!(policy.evaluate("read_file", &json!({"path": "../逃.md"})).is_err());
        assert!(policy.evaluate("read_file", &json!({"path": "notes/乙.txt"})).is_err());
    }

    #[test]
    fn max_turns_forces_answer_round() {
        // 每轮都坚持调工具 → 达上限后强制回答轮（不再执行工具）
        let llm = MockLlm::new(vec![
            Ok(tool("search", json!({"q": "a"}))),
            Ok(tool("search", json!({"q": "b"}))),
            Ok(LlmReply { answer: "被迫收敛".into(), tool: None }),
        ]);
        let exec = MockExec::default();
        let policy = ToolPolicy::new(tmp_kb("maxturns"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(run_loop(
            &llm,
            &exec,
            &policy,
            vec![AgentMessage::user("跑起来")],
            2, // 上限 2 轮：2 次工具轮后强制回答
        )).unwrap();
        assert_eq!(r.answer, "被迫收敛");
        assert_eq!(r.tool_calls, 2);
        assert_eq!(exec.called.lock().unwrap().len(), 2);
    }

    #[test]
    fn llm_error_propagates() {
        let llm = MockLlm::new(vec![Err("上游挂了".into())]);
        let exec = MockExec::default();
        let policy = ToolPolicy::new(tmp_kb("llmerr"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt.block_on(run_loop(
            &llm,
            &exec,
            &policy,
            vec![AgentMessage::user("hi")],
            8,
        )).unwrap_err();
        assert!(err.contains("上游挂了"));
    }

    // ---------- spawn_subagent（独立上下文 + 防递归） ----------

    #[test]
    fn spawn_uses_isolated_context_with_seed() {
        // seed 2 条 + prompt 1 条 = 独立上下文 3 条（父 agent 消息绝不透传）
        let llm = MockLlm::new(vec![Ok(LlmReply {
            answer: "子任务完成".into(),
            tool: None,
        })]);
        let exec = MockExec::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(spawn_subagent(
            &llm,
            &exec,
            &tmp_kb("spawnctx"),
            SubagentSpec {
                prompt: "整理这段知识".into(),
                seed: vec![
                    AgentMessage::assistant("系统：你是子 agent，只读知识库"),
                    AgentMessage::user("背景：父任务上下文"),
                ],
                max_turns: 4,
            },
        )).unwrap();
        assert_eq!(r.answer, "子任务完成");
        let calls = llm.calls.lock().unwrap();
        assert_eq!(*calls.last().unwrap(), 3, "独立上下文 = seed 2 + prompt 1");
    }

    #[test]
    fn spawn_cannot_nest_spawn() {
        // 子 agent 内尝试再 spawn（agent.spawn）→ 策略拒绝，不执行，收敛为回答
        let llm = MockLlm::new(vec![
            Ok(tool("agent.spawn", json!({"prompt": "再来一个"}))),
            Ok(LlmReply { answer: "子任务收敛".into(), tool: None }),
        ]);
        let exec = MockExec::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(spawn_subagent(
            &llm,
            &exec,
            &tmp_kb("nospawn"),
            SubagentSpec::default(),
        )).unwrap();
        assert_eq!(r.answer, "子任务收敛");
        assert_eq!(r.tool_calls, 0, "递归 spawn 被策略拒绝不计数");
        assert!(exec.called.lock().unwrap().is_empty());
    }

    #[test]
    fn spawn_result_tools_visible_to_parent() {
        let llm = MockLlm::new(vec![
            Ok(tool("search", json!({"q": "记忆"}))),
            Ok(LlmReply { answer: "结论".into(), tool: None }),
        ]);
        let exec = MockExec::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(spawn_subagent(
            &llm,
            &exec,
            &tmp_kb("spawntools"),
            SubagentSpec::default(),
        )).unwrap();
        assert_eq!(r.answer, "结论");
        assert_eq!(r.tools_used, vec!["search"]);
    }
}
