//! LLM 代理：转发到 Ollama / OpenAI 兼容接口（/chat/completions）。
//! 浏览器不直连 LLM——走后端代理，避免 CORS 问题与 API Key 暴露。
//! 支持非流式（JSON 透传）与流式（SSE 透传）。

use axum::body::Body;
use axum::http::header;
use axum::response::Response;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;

/// 归一化 chat/completions 地址：
/// - `http://127.0.0.1:11434`             -> 补 `/v1/chat/completions`（Ollama 基址）
/// - `https://api.openai.com/v1`          -> 补 `/chat/completions`
/// - 已含 `/chat/completions`             -> 原样
pub fn completions_url(endpoint: &str) -> String {
    let e = endpoint.trim().trim_end_matches('/');
    if e.ends_with("/chat/completions") {
        e.to_string()
    } else if e.ends_with("/v1") {
        format!("{e}/chat/completions")
    } else {
        format!("{e}/v1/chat/completions")
    }
}

/// Responses API 地址：`https://api.deepseek.com/v1` -> `https://api.deepseek.com/responses`
/// （DeepSeek 实测 Responses 挂在域名根，不带 /v1）
pub fn responses_url(endpoint: &str) -> String {
    let e = endpoint.trim().trim_end_matches('/');
    format!("{}/responses", e.trim_end_matches("/v1"))
}

/// chat 格式 messages → Responses API input items（role 透传，content 统一 input_text 结构）
fn messages_to_items(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|m| {
            let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
            let content = m.get("content").and_then(Value::as_str).unwrap_or("");
            if content.is_empty() {
                return None;
            }
            Some(json!({
                "type": "message",
                "role": role,
                "content": [{"type": "input_text", "text": content}]
            }))
        })
        .collect()
}

/// Responses 响应 → chat 兼容结构（前端零改：choices[0].message.content + reasoning_content + usage）
fn responses_to_chat(v: &Value) -> Value {
    // 拼接所有 message 输出块文本为最终回答；reasoning 输出块拼接为 reasoning_content（前端深度思考块展示）
    let mut text = String::new();
    let mut reasoning = String::new();
    if let Some(out) = v.get("output").and_then(Value::as_array) {
        for o in out {
            match o.get("type").and_then(Value::as_str) {
                Some("message") => {
                    if let Some(cs) = o.get("content").and_then(Value::as_array) {
                        for c in cs {
                            if let Some(t) = c.get("text").and_then(Value::as_str) {
                                text.push_str(t);
                            }
                        }
                    }
                }
                Some("reasoning") => {
                    if let Some(cs) = o.get("content").and_then(Value::as_array) {
                        for c in cs {
                            if let Some(t) = c.get("text").and_then(Value::as_str) {
                                reasoning.push_str(t);
                            }
                        }
                    }
                }
                _ => {} // web_search_call / function_call 事件不产出文本
            }
        }
    }
    let usage = v.get("usage").cloned().unwrap_or_else(|| json!({}));
    json!({
        "choices": [{
            "message": { "role": "assistant", "content": text, "reasoning_content": reasoning },
            "finish_reason": v.get("status").and_then(Value::as_str).unwrap_or("stop")
        }],
        "usage": {
            "prompt_tokens": usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            "completion_tokens": usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
            "reasoning_tokens": usage.get("output_tokens_details").and_then(|d| d.get("reasoning_tokens")).and_then(Value::as_u64).unwrap_or(0),
            "input_tokens_details": usage.get("input_tokens_details").cloned().unwrap_or(json!({}))
        },
        "model": v.get("model").cloned().unwrap_or(json!("")),
        "responses_api": true
    })
}

/// 联网版 chat：chat 格式 body（messages）→ Responses API（tools=[web_search]，服务端执行）。
/// 返回归一化为 chat 兼容结构（前端零改）。仅非流式（Responses 流式事件协议不同）。
pub async fn chat_responses(
    endpoint: &str,
    model: &str,
    api_key: &str,
    body: &Value,
) -> Result<Value, String> {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let items = messages_to_items(&messages);
    if items.is_empty() {
        return Err("messages 为空，无法构造 Responses input".to_string());
    }
    let payload = json!({
        "model": model,
        "input": items,
        "tools": [{"type": "web_search"}],
        "stream": false
    });
    let client = http_client()?;
    let url = responses_url(endpoint);
    let resp = auth(client.post(&url).json(&payload), api_key)
        .send()
        .await
        .map_err(|e| format!("请求 Responses API 失败: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取 Responses 响应失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("Responses API 返回 {status}: {}", truncate(&text, 300)));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("解析 Responses 响应失败: {e}"))?;
    Ok(responses_to_chat(&v))
}

/// 构建上游请求体：模型缺省用配置值；stream 按需设置
fn prepare_body(body: &mut Value, model: &str, stream: bool) {
    let has_model = body
        .get("model")
        .and_then(Value::as_str)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if !has_model {
        body["model"] = json!(model);
    }
    body["stream"] = json!(stream);
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))
}

fn auth(req: reqwest::RequestBuilder, api_key: &str) -> reqwest::RequestBuilder {
    if api_key.trim().is_empty() {
        req
    } else {
        req.header("Authorization", format!("Bearer {}", api_key.trim()))
    }
}

/// 非流式 chat 补全：成功返回上游 JSON 原样透传。
pub async fn chat(
    endpoint: &str,
    model: &str,
    api_key: &str,
    mut body: Value,
) -> Result<Value, String> {
    prepare_body(&mut body, model, false);
    let client = http_client()?;
    let url = completions_url(endpoint);
    let resp = auth(client.post(&url).json(&body), api_key)
        .send()
        .await
        .map_err(|e| format!("请求 LLM 失败: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取 LLM 响应失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("LLM 返回 {status}: {}", truncate(&text, 300)));
    }
    serde_json::from_str(&text).map_err(|e| format!("解析 LLM 响应失败: {e}"))
}

/// 流式 chat 补全：SSE 原样透传（text/event-stream）。
pub async fn chat_stream(
    endpoint: &str,
    model: &str,
    api_key: &str,
    mut body: Value,
) -> Result<Response, String> {
    prepare_body(&mut body, model, true);
    let client = http_client()?;
    let url = completions_url(endpoint);
    let resp = auth(client.post(&url).json(&body), api_key)
        .send()
        .await
        .map_err(|e| format!("请求 LLM 失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp
            .text()
            .await
            .map_err(|e| format!("读取 LLM 响应失败: {e}"))?;
        return Err(format!("LLM 返回 {status}: {}", truncate(&text, 300)));
    }
    let stream = resp
        .bytes_stream()
        .map(|r| r.map_err(|e| std::io::Error::other(e.to_string())));
    Ok(Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .expect("构造流式响应失败"))
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn url_normalization() {
        assert_eq!(completions_url("http://127.0.0.1:11434"), "http://127.0.0.1:11434/v1/chat/completions");
        assert_eq!(completions_url("https://api.deepseek.com/v1"), "https://api.deepseek.com/v1/chat/completions");
        assert_eq!(completions_url("http://x/chat/completions"), "http://x/chat/completions");
        assert_eq!(completions_url("  http://x/  "), "http://x/v1/chat/completions");
        assert_eq!(responses_url("https://api.deepseek.com/v1"), "https://api.deepseek.com/responses");
        assert_eq!(responses_url("https://api.deepseek.com/v1/"), "https://api.deepseek.com/responses");
        assert_eq!(responses_url("https://api.deepseek.com"), "https://api.deepseek.com/responses");
    }

    #[test]
    fn messages_to_items_maps_roles() {
        let msgs = json!([
            { "role": "system", "content": "你是助手" },
            { "role": "user", "content": "你好" },
            { "role": "assistant", "content": "在" },
            { "role": "user", "content": "" }, // 空内容应跳过
        ]);
        let items = messages_to_items(msgs.as_array().unwrap());
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["role"], "system");
        assert_eq!(items[1]["content"][0]["type"], "input_text");
        assert_eq!(items[1]["content"][0]["text"], "你好");
        assert_eq!(items[2]["role"], "assistant");
    }

    #[test]
    fn responses_to_chat_normalizes() {
        let v = json!({
            "id": "r1",
            "model": "deepseek-v4-flash",
            "status": "completed",
            "output": [
                { "type": "reasoning", "content": [{"type": "reasoning_text", "text": "思考"}] },
                { "type": "web_search_call", "status": "completed", "action": {"type": "search", "queries": ["x"]} },
                { "type": "message", "content": [{"type": "output_text", "text": "最终答案"}] }
            ],
            "usage": {
                "input_tokens": 100, "output_tokens": 50,
                "output_tokens_details": {"reasoning_tokens": 30},
                "input_tokens_details": {"cached_tokens": 40}
            }
        });
        let out = responses_to_chat(&v);
        assert_eq!(out["choices"][0]["message"]["content"], "最终答案");
        assert_eq!(out["choices"][0]["finish_reason"], "completed");
        assert_eq!(out["usage"]["prompt_tokens"], 100);
        assert_eq!(out["usage"]["reasoning_tokens"], 30);
        assert_eq!(out["usage"]["input_tokens_details"]["cached_tokens"], 40);
        assert_eq!(out["responses_api"], true);
    }

    #[test]
    fn responses_to_chat_empty_output() {
        let v = json!({ "output": [], "usage": {} });
        let out = responses_to_chat(&v);
        assert_eq!(out["choices"][0]["message"]["content"], "");
    }
}
