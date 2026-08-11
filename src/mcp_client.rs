//! MCP 客户端（Phase 5）：主动连接 MCP 服务（stdio 传输），远程工具以 `mcp__<服务>.<工具>` 进入工具生态。
//! 架构边界（README）：md-agent 只做 MCP 客户端，不对外暴露远程 MCP 服务；
//! 本地 stdio 薄壳（mcp.rs）是唯一服务形态。stdio transport 镜像 mcp.rs 的线级 JSON-RPC 模式；
//! HTTP/SSE 传输预留（McpServerConfig.transport 字段，第二阶段实现）。
//!
//! 生命周期：懒启动（首次访问 spawn + initialize + tools/list），进程随宿主退出（stdio 管道断开自然回收）；
//! 单服务单连接串行调用（Mutex 保证，协议层无并发要求）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::Mutex;

/// 远程 MCP 服务配置（config.json `mcp_servers` 段；#[serde(default)] 兼容旧配置零破坏）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct McpServerConfig {
    /// 唯一 id（工具命名空间 `mcp__<id>.<tool>`；仅字母数字/下划线/短横线）
    pub id: String,
    /// 展示名（面板）
    pub name: String,
    /// 传输类型：当前仅 "stdio"（"http" 预留第二阶段）
    pub transport: String,
    /// stdio：启动命令（绝对路径或 PATH 内命令）
    pub command: String,
    /// stdio：启动参数
    pub args: Vec<String>,
    /// 启用开关（false = 不 spawn、工具不出现在注册表）
    pub enabled: bool,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            transport: "stdio".into(),
            command: String::new(),
            args: Vec::new(),
            enabled: true,
        }
    }
}

/// 服务 id 合法性（工具命名空间的一部分，防注入/防穿越）
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 远程工具描述（来自服务 tools/list，合并进 /api/tools 注册表供 Agent 回路使用）
#[derive(Debug, Clone, Serialize)]
pub struct McpRemoteTool {
    /// 注册表名：`mcp__<id>.<tool>`
    pub name: String,
    /// 服务侧原始工具名
    pub tool: String,
    pub desc: String,
    /// 前端 params 结构（与 tools_json 一致：name/type/required/desc）
    pub params: Vec<Value>,
}

/// 解析 `mcp__<id>.<tool>` → (id, tool)
pub fn split_tool_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("mcp__")?;
    let (id, tool) = rest.split_once('.')?;
    if id.is_empty() || tool.is_empty() {
        return None;
    }
    Some((id, tool))
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// inputSchema（JSON Schema）→ 前端 params 结构
fn schema_to_params(schema: &Value) -> Vec<Value> {
    let props = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::with_capacity(props.len());
    for (k, v) in props {
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("string");
        out.push(json!({
            "name": k,
            "type": ty,
            "required": required.contains(&k),
            "desc": v.get("description").and_then(Value::as_str).unwrap_or("")
        }));
    }
    out
}

/// 工具响应 content → 文本（拼接所有 text 块）
fn content_text(r: &Value) -> String {
    r.get("content")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|c| c.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// 单服务连接：子进程 + 行级 JSON-RPC（镜像 mcp.rs run_stdio 的线级模式，方向相反）
struct McpClient {
    cfg: McpServerConfig,
    child: Child,
    stdin: tokio::io::BufWriter<tokio::process::ChildStdin>,
    stdout: BufReader<tokio::process::ChildStdout>,
    next_id: u64,
}

impl McpClient {
    async fn spawn(cfg: &McpServerConfig) -> Result<Self, String> {
        let mut cmd = tokio::process::Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("启动 MCP 服务「{}」({}) 失败: {e}", cfg.name, cfg.command))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "子进程 stdin 不可用".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "子进程 stdout 不可用".to_string())?;
        let mut client = McpClient {
            cfg: cfg.clone(),
            child,
            stdin: tokio::io::BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            next_id: 1,
        };
        if let Err(e) = client.initialize().await {
            let _ = client.child.kill().await;
            return Err(e);
        }
        Ok(client)
    }

    async fn initialize(&mut self) -> Result<(), String> {
        let resp = self
            .request(
                "initialize",
                Some(json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": { "name": "md-agent", "version": env!("CARGO_PKG_VERSION") }
                })),
            )
            .await?;
        // 宽松兼容：服务可返回任意 protocolVersion（只要求成功握手）
        let _ = resp.get("protocolVersion");
        // 通知 initialized（notification 无 id，服务不回响应）
        self.write_line(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }))
        .await?;
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(p) = params {
            msg["params"] = p;
        }
        self.write_line(&msg).await?;
        // 读行直到 id 匹配（跳过服务主动发的 notification 等无 id 消息）
        loop {
            let line = self.read_line().await?;
            let v: Value = serde_json::from_str(&line).map_err(|e| {
                format!("解析服务响应失败: {e}: {}", truncate(&line, 200))
            })?;
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(err) = v.get("error") {
                    let m = err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("未知错误")
                        .to_string();
                    return Err(format!("{method} 失败: {m}"));
                }
                return Ok(v.get("result").cloned().unwrap_or(json!({})));
            }
        }
    }

    async fn write_line(&mut self, msg: &Value) -> Result<(), String> {
        let s = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        self.stdin
            .write_all(s.as_bytes())
            .await
            .map_err(|e| format!("写入服务失败: {e}"))?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|e| format!("写入服务失败: {e}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("写入服务失败: {e}"))?;
        Ok(())
    }

    async fn read_line(&mut self) -> Result<String, String> {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(60), self.stdout.read_line(&mut line))
            .await
            .map_err(|_| format!("等待服务「{}」响应超时", self.cfg.name))?
            .map_err(|e| format!("读取服务输出失败: {e}"))?;
        if line.is_empty() {
            return Err(format!("服务「{}」已退出（输出流关闭）", self.cfg.name));
        }
        Ok(line)
    }

    async fn list_tools(&mut self) -> Result<Vec<McpRemoteTool>, String> {
        let r = self.request("tools/list", Some(json!({}))).await?;
        let tools = r
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::with_capacity(tools.len());
        for t in tools {
            let tool = t
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if tool.is_empty() {
                continue;
            }
            let desc = t
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let schema = t.get("inputSchema").cloned().unwrap_or(json!({}));
            out.push(McpRemoteTool {
                name: format!("mcp__{}.{}", self.cfg.id, tool),
                tool,
                desc,
                params: schema_to_params(&schema),
            });
        }
        Ok(out)
    }

    async fn call_tool(&mut self, tool: &str, args: &Value) -> Result<String, String> {
        let r = self
            .request(
                "tools/call",
                Some(json!({ "name": tool, "arguments": args })),
            )
            .await?;
        if r.get("isError").and_then(Value::as_bool).unwrap_or(false) {
            let err = content_text(&r);
            return Err(if err.is_empty() {
                "工具执行失败（isError）".to_string()
            } else {
                err
            });
        }
        let text = content_text(&r);
        Ok(if text.is_empty() {
            "(无文本返回)".to_string()
        } else {
            text
        })
    }
}

/// 多服务注册表：懒启动 + 工具清单缓存 + 最近失败记录（面板展示）
pub struct McpRegistry {
    inner: Mutex<RegistryInner>,
}

struct RegistryInner {
    clients: HashMap<String, McpClient>,
    tools: HashMap<String, Vec<McpRemoteTool>>,
    failures: HashMap<String, String>,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RegistryInner {
                clients: HashMap::new(),
                tools: HashMap::new(),
                failures: HashMap::new(),
            }),
        }
    }

    /// 确保服务已连接（懒启动：spawn + initialize + tools/list）
    async fn ensure(&self, cfg: &McpServerConfig) -> Result<(), String> {
        if !valid_id(&cfg.id) {
            return Err("非法服务 id（仅字母数字/下划线/短横线）".to_string());
        }
        if !cfg.enabled {
            return Err(format!("服务「{}」已停用", cfg.name));
        }
        let mut inner = self.inner.lock().await;
        if inner.clients.contains_key(&cfg.id) {
            inner.failures.remove(&cfg.id);
            return Ok(());
        }
        let mut client = McpClient::spawn(cfg)
            .await
            .map_err(|e| {
                inner.failures.insert(cfg.id.clone(), e.clone());
                e
            })?;
        let tools = client.list_tools().await.map_err(|e| {
            let _ = inner.clients.remove(&cfg.id);
            inner.failures.insert(cfg.id.clone(), e.clone());
            e
        })?;
        inner.tools.insert(cfg.id.clone(), tools);
        inner.clients.insert(cfg.id.clone(), client);
        inner.failures.remove(&cfg.id);
        Ok(())
    }

    /// 服务工具清单（连接失败 → Err；注册表合并时降级跳过）
    pub async fn tools_of(&self, cfg: &McpServerConfig) -> Result<Vec<McpRemoteTool>, String> {
        self.ensure(cfg).await?;
        let inner = self.inner.lock().await;
        Ok(inner.tools.get(&cfg.id).cloned().unwrap_or_default())
    }

    /// 调用远程工具（服务侧原始工具名）
    pub async fn call(
        &self,
        cfg: &McpServerConfig,
        tool: &str,
        args: &Value,
    ) -> Result<String, String> {
        self.ensure(cfg).await?;
        let mut inner = self.inner.lock().await;
        let client = inner
            .clients
            .get_mut(&cfg.id)
            .ok_or_else(|| format!("服务「{}」未连接", cfg.name))?;
        client.call_tool(tool, args).await
    }

    /// 面板状态：按配置顺序返回（含连接态/工具数/最近失败）
    pub async fn status(&self, cfgs: &[McpServerConfig]) -> Vec<Value> {
        let inner = self.inner.lock().await;
        cfgs.iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "name": c.name,
                    "transport": c.transport,
                    "command": c.command,
                    "args": c.args,
                    "enabled": c.enabled,
                    "connected": inner.clients.contains_key(&c.id),
                    "tools": inner.tools.get(&c.id).map(|t| t.len()).unwrap_or(0),
                    "failure": inner.failures.get(&c.id).cloned().unwrap_or_default()
                })
            })
            .collect()
    }

    /// 测试连接：强制 ensure + 返回工具清单（面板"测试"按钮全链路验证）
    pub async fn test(&self, cfg: &McpServerConfig) -> Result<Vec<McpRemoteTool>, String> {
        self.tools_of(cfg).await
    }

    /// 停用/移除：kill 子进程 + 清缓存
    pub async fn disconnect(&self, id: &str) {
        let mut inner = self.inner.lock().await;
        if let Some(mut c) = inner.clients.remove(id) {
            let _ = c.child.kill().await;
        }
        inner.tools.remove(id);
        inner.failures.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_id_rules() {
        assert!(valid_id("erp"));
        assert!(valid_id("my-server_1"));
        assert!(!valid_id(""));
        assert!(!valid_id("a/b"));
        assert!(!valid_id("a..b"));
        assert!(!valid_id("中文"));
        assert!(!valid_id("a b"));
    }

    #[test]
    fn split_tool_name_parses() {
        assert_eq!(split_tool_name("mcp__erp.candidates_list"), Some(("erp", "candidates_list")));
        assert_eq!(split_tool_name("mcp__a.b.c"), Some(("a", "b.c")));
        assert_eq!(split_tool_name("search"), None);
        assert_eq!(split_tool_name("mcp__erp"), None);
        assert_eq!(split_tool_name("mcp__."), None);
        assert_eq!(split_tool_name("mcp___x"), None); // 点前 id 空
    }

    #[test]
    fn schema_to_params_maps_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "q": {"type": "string", "description": "检索词"},
                "limit": {"type": "integer"}
            },
            "required": ["q"]
        });
        let params = schema_to_params(&schema);
        assert_eq!(params.len(), 2);
        let q = params.iter().find(|p| p["name"] == "q").unwrap();
        assert_eq!(q["required"], true);
        assert_eq!(q["desc"], "检索词");
        let limit = params.iter().find(|p| p["name"] == "limit").unwrap();
        assert_eq!(limit["required"], false);
        assert_eq!(limit["type"], "integer");
        // 空 schema → 空 params
        assert!(schema_to_params(&json!({})).is_empty());
    }

    #[test]
    fn config_serde_default_compat() {
        // 旧配置（无 mcp_servers 段）反序列化 → 空列表
        let old = r#"{"kb_root": "kb", "llm": {"endpoint": "", "model": "", "api_key": ""}}"#;
        let cfg: crate::config::Config = serde_json::from_str(old).unwrap();
        assert!(cfg.mcp_servers.is_empty());
        // 完整配置 roundtrip
        let c = McpServerConfig {
            id: "erp".into(),
            name: "猎头 ERP".into(),
            transport: "stdio".into(),
            command: "node".into(),
            args: vec!["mcp-server.js".into()],
            enabled: true,
        };
        let s = serde_json::to_string(&c).unwrap();
        let back: McpServerConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(back, c);
    }
}
