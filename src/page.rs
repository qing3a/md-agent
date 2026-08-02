//! /page 动态网页读取：CDP 直连本机 Chrome/Edge（headless），等渲染后取 innerText。
//! 与 /fetch（静态 HTTP 抓取）互补：本模块面向 JS 渲染页面。
//! 只读提取；写回/发布仍需人工走 /fetch + 待审通道。

use chromiumoxide::{Browser, BrowserConfig};
use futures_util::StreamExt;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct PageResult {
    pub url: String,
    pub title: String,
    pub text: String,
    pub truncated: bool,
    pub engine: String, // chrome | edge
}

const MAX_TEXT: usize = 20_000;

/// 探测本机可用的 Chromium 系浏览器。
/// Edge 优先：Windows 系统级安装（sxs 清单完备），用户级 Chrome（LOCALAPPDATA）偶发
/// 并行配置错误（os error 14001）。
pub fn chrome_executable() -> Option<(PathBuf, String)> {
    let mut v: Vec<(PathBuf, &str)> = Vec::new();
    for pf in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(p) = std::env::var(pf) {
            v.push((PathBuf::from(&p).join("Microsoft/Edge/Application/msedge.exe"), "edge"));
        }
    }
    for pf in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(p) = std::env::var(pf) {
            v.push((PathBuf::from(&p).join("Google/Chrome/Application/chrome.exe"), "chrome"));
        }
    }
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        v.push((PathBuf::from(&la).join("Google/Chrome/Application/chrome.exe"), "chrome"));
    }
    v.into_iter().find(|(p, _)| p.exists()).map(|(p, e)| (p, e.to_string()))
}

pub async fn extract_page(url: &str) -> Result<PageResult, String> {
    let u = url.trim();
    if !u.starts_with("http://") && !u.starts_with("https://") {
        return Err("仅支持 http/https 链接".to_string());
    }
    let (exe, engine) =
        chrome_executable().ok_or_else(|| "未找到本机 Chrome/Edge（需任一浏览器支持 headless CDP）".to_string())?;

    let config = BrowserConfig::builder()
        .chrome_executable(exe)
        .no_sandbox()
        .build()
        .map_err(|e| format!("浏览器配置失败: {e}"))?;

    // 0.8: launch 返回 (Browser, Handler)，Handler 必须驻留驱动事件流
    let (mut browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|e| format!("启动浏览器失败（{engine}）: {e}"))?;
    let driver = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let out = tokio::time::timeout(
        Duration::from_secs(25),
        read_page(&browser, u, &engine),
    )
    .await;

    let _ = browser.close().await; // 无论成败都收掉浏览器进程
    let _ = driver.await;
    match out {
        Ok(r) => r,
        Err(_) => Err("页面加载超时(25s)或渲染卡死".to_string()),
    }
}

async fn read_page(browser: &Browser, url: &str, engine: &str) -> Result<PageResult, String> {
    let page = browser
        .new_page(url)
        .await
        .map_err(|e| format!("打开页面失败: {e}"))?;
    let _ = page.wait_for_navigation().await; // 有的页面持续导航，等不到就继续

    let title = page.get_title().await.ok().flatten().unwrap_or_default();
    // 等到 body 有内容（首屏/跳转链完成），最多 ~5s；SPA 空 body 时重试 innerText
    let mut text = String::new();
    for _ in 0..10 {
        if !text.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        text = page
            .evaluate("document.body ? document.body.innerText : ''")
            .await
            .map_err(|e| format!("读取页面文本失败: {e}"))?
            .into_value::<String>()
            .unwrap_or_default();
    }
    // 压缩空白/空行
    let text = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    let truncated = text.chars().count() > MAX_TEXT;
    Ok(PageResult {
        url: url.to_string(),
        title,
        text: text.chars().take(MAX_TEXT).collect(),
        truncated,
        engine: engine.to_string(),
    })
}

// ---------- /page act：动作执行（写侧，前端人审清单确认后调用）----------

#[derive(Debug, serde::Deserialize, Clone)]
pub struct ActStep {
    pub kind: String, // click | fill | select | scroll
    pub selector: String,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ActResult {
    pub ok: bool,
    pub step: usize,
    pub message: String,
    pub title: String,
    pub text: String,
}

/// 在页面上依次执行动作，返回执行结果与页面文本摘要
pub async fn act_page(url: &str, steps: &[ActStep]) -> Result<ActResult, String> {
    let (exe, engine) =
        chrome_executable().ok_or_else(|| "未找到本机 Chrome/Edge（需任一浏览器支持 headless CDP）".to_string())?;
    let config = BrowserConfig::builder()
        .chrome_executable(exe)
        .no_sandbox()
        .build()
        .map_err(|e| format!("浏览器配置失败: {e}"))?;
    let (mut browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|e| format!("启动浏览器失败（{engine}）: {e}"))?;
    let driver = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let out = tokio::time::timeout(Duration::from_secs(30), async {
        let page = browser
            .new_page(url)
            .await
            .map_err(|e| format!("打开页面失败: {e}"))?;
        let _ = page.wait_for_navigation().await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        for (i, st) in steps.iter().enumerate() {
            let label = format!("第 {} 步 {} {}", i + 1, st.kind, st.selector);
            match st.kind.as_str() {
                "click" => {
                    let el = page
                        .find_element(&st.selector)
                        .await
                        .map_err(|e| format!("{label}: 元素未找到: {e}"))?;
                    el.click().await.map_err(|e| format!("{label}: 点击失败: {e}"))?;
                }
                "fill" => {
                    let el = page
                        .find_element(&st.selector)
                        .await
                        .map_err(|e| format!("{label}: 元素未找到: {e}"))?;
                    el.click().await.map_err(|e| format!("{label}: 聚焦失败: {e}"))?;
                    el.type_str(st.value.as_deref().unwrap_or(""))
                        .await
                        .map_err(|e| format!("{label}: 输入失败: {e}"))?;
                }
                "select" => {
                    let v = st.value.as_deref().unwrap_or("");
                    let js = format!(
                        "(()=>{{const el=document.querySelector({:?});if(!el)return false;el.value={:?};el.dispatchEvent(new Event('change',{{bubbles:true}}));return true;}})()",
                        st.selector, v
                    );
                    let ok: bool = page
                        .evaluate(js)
                        .await
                        .map_err(|e| format!("{label}: 执行失败: {e}"))?
                        .into_value::<bool>()
                        .unwrap_or(false);
                    if !ok {
                        return Err(format!("{label}: 元素未找到"));
                    }
                }
                "scroll" => {
                    page.evaluate("window.scrollTo(0, document.body.scrollHeight)")
                        .await
                        .map_err(|e| format!("{label}: 滚动失败: {e}"))?;
                }
                other => return Err(format!("未知动作 {other}（可选 click/fill/select/scroll）")),
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let title = page.get_title().await.ok().flatten().unwrap_or_default();
        let text: String = page
            .evaluate("document.body ? document.body.innerText : ''")
            .await
            .map_err(|e| format!("读取页面文本失败: {e}"))?
            .into_value::<String>()
            .unwrap_or_default();
        let text = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(ActResult {
            ok: true,
            step: steps.len(),
            message: format!("{} 个动作执行完成", steps.len()),
            title,
            text: text.chars().take(4000).collect(),
        })
    })
    .await;

    let _ = browser.close().await;
    let _ = driver.await;
    match out {
        Ok(r) => r,
        Err(_) => Err("页面操作超时(30s)".to_string()),
    }
}
