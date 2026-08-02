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
