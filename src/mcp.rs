//! MCP 薄壳（Model Context Protocol server over stdio）：
//! `md-agent --mcp` 启动 —— 进程内跑 HTTP 服务 + stdio JSON-RPC 循环，
//! 把 md-agent 的纯本地能力（检索/记忆/图谱/风控/待审/任务）暴露为标准 MCP 工具，
//! 供 Claude Code / DeepSeek Harness / Cursor 等 MCP 客户端一行配置接入。
//! 设计：只暴露零 LLM 依赖的工具——md-agent 作为「知识/数据层」，推理由调用方负责。
use std::io::{BufRead, Write};

use serde_json::{json, Value};

struct McpTool {
    name: &'static str,
    desc: &'static str,
    schema: Value,
    /// 构造本地 HTTP 请求：(path, method, body)
    call: fn(&Value) -> (String, &'static str, Option<Value>),
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn tools() -> Vec<McpTool> {
    vec![
    McpTool {
        name: "search",
        desc: "检索知识库 L2 内容层（全文 ripgrep，多关键词任一命中，返回命中片段与所属小节）",
        schema: json!({
            "type": "object",
            "properties": {
                "q": {"type": "string", "description": "检索关键词，空格分隔"},
                "layer": {"type": "string", "description": "notes（默认）| all（含 L1 规范/记忆）"},
                "ctx": {"type": "string", "description": "传 1 返回命中行前后上下文"}
            },
            "required": ["q"]
        }),
        call: |a| {
            let q = a.get("q").and_then(Value::as_str).unwrap_or("");
            let layer = a.get("layer").and_then(Value::as_str).unwrap_or("notes");
            let ctx = a.get("ctx").and_then(Value::as_str).unwrap_or("");
            (
                format!(
                    "/api/search?q={}&layer={}&ctx={}",
                    urlencode(q),
                    urlencode(layer),
                    urlencode(ctx)
                ),
                "GET",
                None,
            )
        },
    },
    McpTool {
        name: "read_l1",
        desc: "读取知识库 L1 规范/记忆/索引层（KB/FRAMEWORK/RULES/MEMORY/INDEX/记忆摘要）——回答涉及规范约定、历史决策时先查",
        schema: json!({
            "type": "object",
            "properties": {
                "file": {"type": "string", "description": "L1 文件名：KB.md|FRAMEWORK.md|RULES.md|MEMORY.md|INDEX.md|memory_summary.md"},
                "q": {"type": "string", "description": "定位词：空=文件头+章节清单；非空=第一个含词的 ## 小节"}
            },
            "required": ["file"]
        }),
        call: |a| {
            let file = a.get("file").and_then(Value::as_str).unwrap_or("");
            let q = a.get("q").and_then(Value::as_str).unwrap_or("");
            (
                format!("/api/l1/read?file={}&q={}", urlencode(file), urlencode(q)),
                "GET",
                None,
            )
        },
    },
    McpTool {
        name: "memory_search",
        desc: "记忆检索：检索整个持久记忆系统（L1 规范/记忆/索引 + L2 内容层），返回 top 片段",
        schema: json!({
            "type": "object",
            "properties": {"q": {"type": "string", "description": "检索关键词，空格分隔"}},
            "required": ["q"]
        }),
        call: |a| {
            let q = a.get("q").and_then(Value::as_str).unwrap_or("");
            (
                format!("/api/search?q={}&layer=all&ctx=1", urlencode(q)),
                "GET",
                None,
            )
        },
    },
    McpTool {
        name: "graph.linked",
        desc: "查文档的出链（[[双链]] 指向谁，含悬空检测）",
        schema: json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "文档相对 KB 根路径"}},
            "required": ["path"]
        }),
        call: |a| {
            let p = a.get("path").and_then(Value::as_str).unwrap_or("");
            (format!("/api/graph/linked?path={}", urlencode(p)), "GET", None)
        },
    },
    McpTool {
        name: "graph.backlinks",
        desc: "查文档的入链（谁链向该文档）",
        schema: json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "文档相对 KB 根路径"}},
            "required": ["path"]
        }),
        call: |a| {
            let p = a.get("path").and_then(Value::as_str).unwrap_or("");
            (
                format!("/api/graph/backlinks?path={}", urlencode(p)),
                "GET",
                None,
            )
        },
    },
    McpTool {
        name: "graph.paths",
        desc: "查两篇文档之间的最短关联路径（双链关系链，最多 6 跳）",
        schema: json!({
            "type": "object",
            "properties": {
                "from": {"type": "string", "description": "起点文档相对 KB 根路径"},
                "to": {"type": "string", "description": "终点文档相对 KB 根路径"}
            },
            "required": ["from", "to"]
        }),
        call: |a| {
            let from = a.get("from").and_then(Value::as_str).unwrap_or("");
            let to = a.get("to").and_then(Value::as_str).unwrap_or("");
            (
                format!(
                    "/api/graph/paths?from={}&to={}&max_depth=6",
                    urlencode(from),
                    urlencode(to)
                ),
                "GET",
                None,
            )
        },
    },
    McpTool {
        name: "risk.check",
        desc: "风控预警扫描（律师案件：诉讼时效到期/证据待补/案件信息缺失，纯规则零 LLM）",
        schema: json!({ "type": "object", "properties": {} }),
        call: |_| ("/api/risk".to_string(), "GET", None),
    },
    McpTool {
        name: "file_read",
        desc: "读取 KB 内 Markdown 文件全文（L1 或 L2）",
        schema: json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "文件相对 KB 根路径"}},
            "required": ["path"]
        }),
        call: |a| {
            let p = a.get("path").and_then(Value::as_str).unwrap_or("");
            (format!("/api/file?path={}", urlencode(p)), "GET", None)
        },
    },
    McpTool {
        name: "pending.list",
        desc: "列出待审提案（写操作人审队列：记忆/技能/巩固/笔记提案）",
        schema: json!({ "type": "object", "properties": {} }),
        call: |_| ("/api/kb/pending".to_string(), "GET", None),
    },
    McpTool {
        name: "tasks",
        desc: "列出任务引擎的当前任务（状态机：待办/进行中/完成/放弃）",
        schema: json!({ "type": "object", "properties": {} }),
        call: |_| ("/api/tasks".to_string(), "GET", None),
    },
    ]
}


/// stdio JSON-RPC 循环：MCP stdio transport = 换行分隔的 JSON-RPC 消息
pub fn run_stdio(port: u16) {
    let base = format!("http://127.0.0.1:{port}");
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let result: Result<Value, (i64, String)> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2025-03-26",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "md-agent", "version": "0.1.0" }
            })),
            "ping" => Ok(json!({})),
            "tools/list" => {
                let tools: Vec<Value> = tools()
                    .iter()
                    .map(|t| {
                        json!({
                            "name": t.name,
                            "description": t.desc,
                            "inputSchema": t.schema
                        })
                    })
                    .collect();
                Ok(json!({ "tools": tools }))
            }
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                match tools().iter().find(|t| t.name == name) {
                    Some(t) => {
                        let (path, method, body) = (t.call)(&args);
                        match call_local(&base, &path, method, body.as_ref()) {
                            Ok(text) => Ok(json!({
                                "content": [{"type": "text", "text": text}]
                            })),
                            Err(e) => Err((-32603, format!("工具执行失败: {e}"))),
                        }
                    }
                    None => Err((-32602, format!("未知工具: {name}"))),
                }
            }
            "notifications/initialized" | "notifications/cancelled" => continue,
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        match result {
            Ok(r) => {
                let mut resp = json!({ "jsonrpc": "2.0", "result": r });
                if let Some(i) = id {
                    resp["id"] = i;
                }
                let _ = writeln!(out, "{}", resp);
            }
            Err((code, msg)) => {
                let mut resp = json!({
                    "jsonrpc": "2.0",
                    "error": { "code": code, "message": msg }
                });
                if let Some(i) = id {
                    resp["id"] = i;
                }
                let _ = writeln!(out, "{}", resp);
            }
        }
        let _ = out.flush();
    }
}

/// 转发到本地 HTTP（复用现有 API handler；X-Project 空 = 个人空间）
fn call_local(base: &str, path: &str, method: &str, body: Option<&Value>) -> Result<String, String> {
    let url = format!("{base}{path}");
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.request(
        reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?,
        &url,
    );
    if let Some(b) = body {
        req = req.json(b);
    }
    let resp = req.send().map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().map_err(|e| e.to_string())?;
    if status.is_success() {
        Ok(text)
    } else {
        Err(format!("HTTP {status}: {}", text.chars().take(300).collect::<String>()))
    }
}
