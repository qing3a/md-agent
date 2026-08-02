//! /fetch 静态网页抓取（零浏览器依赖）：HTTP 获取 + 简易 HTML 文本提取。
//! 动态/交互页面走 Page 引擎（/page），本模块只做静态读取。

use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct FetchResult {
    pub url: String,
    pub title: String,
    pub text: String,
    pub truncated: bool,
}

const MAX_BYTES: usize = 4_000_000;
const MAX_TEXT: usize = 20_000;

pub async fn fetch_page(url: &str) -> Result<FetchResult, String> {
    let u = url.trim();
    if !u.starts_with("http://") && !u.starts_with("https://") {
        return Err("仅支持 http/https 链接".to_string());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("md-agent/0.1 (local knowledge base)")
        .build()
        .map_err(|e| format!("构建客户端失败: {e}"))?;
    let resp = client
        .get(u)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("读取失败: {e}"))?;
    let html = String::from_utf8_lossy(&bytes);
    let html = &html[..html.len().min(MAX_BYTES)];

    let title = extract_title(html).unwrap_or_else(|| u.to_string());
    let text = extract_text(html);
    let truncated = text.chars().count() > MAX_TEXT;
    Ok(FetchResult {
        url: u.to_string(),
        title,
        text: text.chars().take(MAX_TEXT).collect(),
        truncated,
    })
}

fn extract_title(html: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title>").ok()?;
    let t = re.captures(html)?.get(1)?.as_str().trim();
    let cleaned = clean_text(t);
    if cleaned.is_empty() { None } else { Some(cleaned) }
}

fn extract_text(html: &str) -> String {
    // 去掉 script/style/noscript/svg/head 与注释（regex 不支持反向引用，展开闭合标签）
    let re_skip = regex::Regex::new(
        r"(?is)<(script|style|noscript|svg|head)[^>]*>.*?</(script|style|noscript|svg|head)>|<!--.*?-->",
    )
    .unwrap();
    let mut s = re_skip.replace_all(html, " ").to_string();
    // 块级标签换行
    let re_block = regex::Regex::new(
        r"(?i)</?(p|div|h[1-6]|li|br|tr|section|article|blockquote|pre|table)[^>]*>",
    )
    .unwrap();
    s = re_block.replace_all(&s, "\n").to_string();
    // 去标签
    let re_tag = regex::Regex::new(r"(?s)<[^>]+>").unwrap();
    s = re_tag.replace_all(&s, " ").to_string();
    // 常见实体解码
    for (k, v) in [
        ("&nbsp;", " "),
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
    ] {
        s = s.replace(k, v);
    }
    s.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn clean_text(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
