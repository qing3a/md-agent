//! Embedding 客户端（语义召回的向量来源，Phase 4 M1）。
//! 双轨策略：配置了 llm.embedding（endpoint/model/api_key）→ 调 OpenAI 兼容 `/embeddings`；
//! 未配置 → 调用方降级纯 grep（现状），零依赖不破坏既有链路。
//! 只走 OpenAI 兼容协议（Ollama 的 /v1/embeddings、OpenAI 系均一致），不额外分叉。

use serde_json::{json, Value};
use std::time::Duration;

/// 归一化 embeddings 地址（与 llm.rs::completions_url 同构）：
/// - `http://127.0.0.1:11434`             -> 补 `/v1/embeddings`（Ollama 基址，OpenAI 兼容）
/// - `https://api.openai.com/v1`          -> 补 `/embeddings`
/// - 已含 `/embeddings`                   -> 原样
pub fn embedding_url(endpoint: &str) -> String {
    let e = endpoint.trim().trim_end_matches('/');
    if e.ends_with("/embeddings") {
        e.to_string()
    } else if e.ends_with("/v1") {
        format!("{e}/embeddings")
    } else {
        format!("{e}/v1/embeddings")
    }
}

/// 是否已配置 embedding 通道（endpoint 与 model 均非空才算）
pub fn configured(endpoint: &str, model: &str) -> bool {
    !endpoint.trim().is_empty() && !model.trim().is_empty()
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))
}

/// 批量文本 -> 向量（OpenAI 兼容 embeddings 响应：data[i].embedding 为 f32 数组）。
/// 批次上限 32（多数服务端单批限制；调用方负责分批）。
pub async fn embed_texts(
    endpoint: &str,
    model: &str,
    api_key: &str,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let client = http_client()?;
    let url = embedding_url(endpoint);
    let body = json!({ "model": model, "input": texts });
    let mut req = client.post(&url).json(&body);
    if !api_key.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
    }
    let resp = req.send().await.map_err(|e| format!("请求 embedding 失败: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取 embedding 响应失败: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "embedding 返回 {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    let v: Value =
        serde_json::from_str(&text).map_err(|e| format!("解析 embedding 响应失败: {e}"))?;
    parse_embeddings(&v)
}

/// 解析 OpenAI 兼容 embeddings 响应 → 向量列表（数量与输入一致，顺序对应）
pub fn parse_embeddings(v: &Value) -> Result<Vec<Vec<f32>>, String> {
    let data = v
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("响应缺 data 数组: {}", truncate(&v.to_string(), 200)))?;
    let mut out = Vec::with_capacity(data.len());
    for item in data {
        let arr = item
            .get("embedding")
            .and_then(Value::as_array)
            .ok_or_else(|| "data 项缺 embedding 数组".to_string())?;
        let mut vec = Vec::with_capacity(arr.len());
        for n in arr {
            let f = n
                .as_f64()
                .ok_or_else(|| "embedding 元素非数字".to_string())?;
            vec.push(f as f32);
        }
        if vec.is_empty() {
            return Err("embedding 向量为空".to_string());
        }
        out.push(vec);
    }
    Ok(out)
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_url_normalization() {
        assert_eq!(embedding_url("http://127.0.0.1:11434"), "http://127.0.0.1:11434/v1/embeddings");
        assert_eq!(embedding_url("https://api.openai.com/v1"), "https://api.openai.com/v1/embeddings");
        assert_eq!(
            embedding_url("https://api.openai.com/v1/embeddings"),
            "https://api.openai.com/v1/embeddings"
        );
        assert_eq!(embedding_url("http://x:11434/"), "http://x:11434/v1/embeddings");
    }

    #[test]
    fn configured_requires_endpoint_and_model() {
        assert!(!configured("", "bge-m3"));
        assert!(!configured("http://127.0.0.1:11434", ""));
        assert!(configured("http://127.0.0.1:11434", "bge-m3"));
    }

    #[test]
    fn parse_embeddings_ok() {
        let v = json!({
            "model": "bge-m3",
            "data": [
                {"embedding": [0.1, 0.2, 0.3], "index": 0},
                {"embedding": [1.0, -1.0], "index": 1}
            ]
        });
        let out = parse_embeddings(&v).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], vec![0.1f32, 0.2, 0.3]);
        assert_eq!(out[1], vec![1.0f32, -1.0]);
    }

    #[test]
    fn parse_embeddings_rejects_missing_data() {
        assert!(parse_embeddings(&json!({})).is_err());
        assert!(parse_embeddings(&json!({ "data": [{}] })).is_err());
        assert!(parse_embeddings(&json!({ "data": [{ "embedding": [] }] })).is_err());
    }

    #[test]
    fn parse_embeddings_rejects_non_numeric() {
        assert!(parse_embeddings(&json!({ "data": [{ "embedding": [1, "x"] }] })).is_err());
    }

    // mock 单测：本地 std TcpListener 起一个一次性 HTTP 响应（纯 std，无第三方测试依赖）
    fn mock_server(resp_body: &'static str) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf); // 吃掉请求行/头
                let body = resp_body;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.write_all(body.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn embed_texts_hits_mock_endpoint() {
        let body = r#"{"model":"bge-m3","data":[{"embedding":[0.5,0.25],"index":0}]}"#;
        let ep = mock_server(body);
        let out = embed_texts(&ep, "bge-m3", "", &["你好".to_string()]).await.unwrap();
        assert_eq!(out, vec![vec![0.5f32, 0.25]]);
    }

    #[tokio::test]
    async fn embed_texts_empty_returns_empty() {
        let out = embed_texts("http://127.0.0.1:9", "m", "", &[]).await.unwrap();
        assert!(out.is_empty());
    }
}
