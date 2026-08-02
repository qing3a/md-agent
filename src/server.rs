//! Axum HTTP 服务：托管 xterm.js 前端 + 知识库接口（同源，免 CORS）。

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Clone)]
pub struct AppState {
    pub kb_root: PathBuf,
    pub web_dir: PathBuf,
}

pub async fn serve(
    port: u16,
    kb_root: PathBuf,
    web_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    crate::kb::ensure_layout(&kb_root)?;
    let state = AppState { kb_root, web_dir };

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/search", get(search_handler))
        .route("/api/l1", get(l1_handler))
        .route("/api/file", get(file_read))
        .route("/api/file", post(file_write))
        .route("/api/kb/sync", post(kb_sync))
        .route("/api/kb/pending", get(kb_pending_list))
        .route("/api/kb/pending/approve", post(kb_pending_approve))
        .route("/api/kb/pending/reject", post(kb_pending_reject))
        .route("/api/graph/sync", post(graph_sync))
        .route("/api/graph/stats", get(graph_stats))
        .route("/api/graph/graph", get(graph_graph))
        .route("/api/graph/backlinks", get(graph_backlinks))
        .route("/api/graph/linked", get(graph_linked))
        .route("/api/graph/related", get(graph_related))
        .route("/api/graph/orphans", get(graph_orphans))
        .route("/api/graph/tags", get(graph_tags))
        .route("/api/graph/projects", get(graph_projects))
        .route("/api/audit", get(audit_report))
        .route("/api/link", post(link_add))
        .route("/api/config", get(config_get))
        .route("/api/config", post(config_set))
        .route("/api/llm", post(llm_chat))
        .fallback_service(tower_http::services::ServeDir::new(state.web_dir.clone()))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// ---------- handlers ----------

async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "service": "md-agent",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    #[serde(default = "default_layer")]
    layer: String,
    /// 1/true/on 时返回命中行前后上下文片段（Prompt 注入用）
    #[serde(default)]
    ctx: String,
}

fn default_layer() -> String {
    "all".to_string()
}

fn ctx_enabled(ctx: &str) -> bool {
    matches!(ctx, "1" | "true" | "on" | "yes")
}

async fn search_handler(
    State(st): State<AppState>,
    Query(p): Query<SearchParams>,
) -> Response {
    match crate::search::search(&st.kb_root, &p.q, &p.layer, ctx_enabled(&p.ctx)) {
        Ok(r) => Json(r).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

async fn l1_handler(State(st): State<AppState>, Query(p): Query<L1Params>) -> Json<serde_json::Value> {
    let full = matches!(p.full.as_str(), "1" | "true" | "on" | "yes");
    let files: Vec<serde_json::Value> = crate::kb::list_l1(&st.kb_root, full)
        .into_iter()
        .map(|f| json!({ "name": f.name, "path": f.path, "head": f.head, "content": f.content }))
        .collect();
    Json(json!({ "l1": files }))
}

#[derive(Deserialize)]
struct L1Params {
    /// 1 时返回完整内容（Agent 启动注入 L1 用）
    #[serde(default)]
    full: String,
}

#[derive(Deserialize)]
struct FileParams {
    path: String,
}

async fn file_read(State(st): State<AppState>, Query(p): Query<FileParams>) -> Response {
    match crate::kb::resolve_in_kb(&st.kb_root, &p.path) {
        Some(pb) if pb.is_file() => match tokio::fs::read_to_string(&pb).await {
            Ok(content) => Json(json!({ "path": p.path, "content": content })).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response(),
        },
        _ => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "文件不存在或超出 KB 范围" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct FileWriteBody {
    path: String,
    content: String,
}

async fn file_write(State(st): State<AppState>, Json(body): Json<FileWriteBody>) -> Response {
    let Some(pb) = crate::kb::resolve_in_kb(&st.kb_root, &body.path) else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "路径超出 KB 范围" })),
        )
            .into_response();
    };
    if let Some(parent) = pb.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    }
    match tokio::fs::write(&pb, body.content).await {
        Ok(()) => Json(json!({ "ok": true, "path": body.path })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn kb_sync(State(st): State<AppState>) -> Response {
    match crate::kb::sync_index(&st.kb_root) {
        Ok(r) => Json(json!({ "ok": true, "index": r.index_path, "files": r.files })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ---------- 记忆自组织（Phase 3-A：审计 / 补链接） ----------

async fn audit_report(State(st): State<AppState>) -> Response {
    if let Err(e) = ensure_graph(&st.kb_root) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response();
    }
    match crate::graph::audit(&st.kb_root) {
        Ok(r) => Json(r).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    }
}

#[derive(Deserialize)]
struct LinkBody {
    src: String,
    dst: String,
}

/// 补链接：在 src 文档末尾追加 `- 关联：[[dst]]`，重建 INDEX + 图谱。
/// 由用户主动调用 = 已人工确认（不进待审）。
async fn link_add(State(st): State<AppState>, Json(body): Json<LinkBody>) -> Response {
    if let Err(e) = ensure_graph(&st.kb_root) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response();
    }
    let src = match crate::graph::resolve_doc(&st.kb_root, &body.src) {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("源文档未找到: {}", body.src) })),
            )
                .into_response();
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    };
    let dst = match crate::graph::resolve_doc(&st.kb_root, &body.dst) {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("目标文档未找到: {}", body.dst) })),
            )
                .into_response();
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    };
    if src == dst {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "不能链接到自身" }))).into_response();
    }

    let file = st.kb_root.join(&src);
    let Ok(mut content) = tokio::fs::read_to_string(&file).await else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "读取源文档失败" }))).into_response();
    };
    // 双链约定用文件名（如 [[托盘应用]]），不用完整路径
    let dst_stem = std::path::Path::new(&dst)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&dst)
        .to_string();
    let link_line = format!("- 关联：[[{dst_stem}]]");
    if content.contains(&link_line)
        || content.contains(&format!("[[{dst_stem}]]"))
        || content.contains(&format!("[[{dst_stem}.md]]"))
    {
        return Json(json!({ "ok": false, "note": "链接已存在", "src": src, "dst": dst })).into_response();
    }
    content = format!("{}\n\n{}", content.trim_end(), link_line);
    if let Err(e) = tokio::fs::write(&file, content).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
    }
    let _ = crate::kb::sync_index(&st.kb_root);
    let _ = crate::graph::sync_graph(&st.kb_root);
    Json(json!({ "ok": true, "src": src, "dst": dst, "link": link_line })).into_response()
}

// ---------- 待审机制 ----------

async fn kb_pending_list(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({ "pending": crate::kb::list_pending(&st.kb_root) }))
}

#[derive(Deserialize)]
struct PendingBody {
    path: String,
}

async fn kb_pending_approve(State(st): State<AppState>, Json(body): Json<PendingBody>) -> Response {
    let paths: Vec<String> = if body.path == "all" {
        crate::kb::list_pending(&st.kb_root).into_iter().map(|p| p.path).collect()
    } else {
        vec![body.path]
    };
    if paths.is_empty() {
        return Json(json!({ "ok": [], "errors": [], "note": "待审区为空" })).into_response();
    }
    let mut ok: Vec<serde_json::Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for p in paths {
        match crate::kb::approve_pending(&st.kb_root, &p) {
            Ok((target, note)) => ok.push(json!({ "path": p, "target": target, "note": note })),
            Err(e) => errors.push(format!("{p}: {e}")),
        }
    }
    if !ok.is_empty() {
        let _ = crate::kb::sync_index(&st.kb_root);
        let _ = crate::graph::sync_graph(&st.kb_root);
    }
    Json(json!({ "ok": ok, "errors": errors })).into_response()
}

async fn kb_pending_reject(State(st): State<AppState>, Json(body): Json<PendingBody>) -> Response {
    match crate::kb::reject_pending(&st.kb_root, &body.path) {
        Ok(n) => Json(json!({ "ok": [json!({ "path": body.path, "removed": n })], "errors": [] })).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({ "error": e }))).into_response(),
    }
}

// ---------- 知识图谱 ----------

/// 图谱库不存在时自动全量同步（惰性初始化）
fn ensure_graph(root: &std::path::Path) -> Result<(), String> {
    let db = root.join(crate::graph::DB_FILE);
    if !db.exists() {
        crate::graph::sync_graph(root)?;
    }
    Ok(())
}

async fn graph_sync(State(st): State<AppState>) -> Response {
    match crate::graph::sync_graph(&st.kb_root) {
        Ok(r) => Json(json!({ "ok": true, "graph": r })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        )
            .into_response(),
    }
}

async fn graph_stats(State(st): State<AppState>) -> Response {
    if let Err(e) = ensure_graph(&st.kb_root) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response();
    }
    match crate::graph::stats(&st.kb_root) {
        Ok(s) => Json(s).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    }
}

async fn graph_graph(State(st): State<AppState>) -> Response {
    if let Err(e) = ensure_graph(&st.kb_root) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response();
    }
    match crate::graph::graph_data(&st.kb_root) {
        Ok(d) => Json(d).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    }
}

#[derive(Deserialize)]
struct GraphPathParams {
    path: String,
}

async fn graph_backlinks(State(st): State<AppState>, Query(p): Query<GraphPathParams>) -> Response {
    if let Err(e) = ensure_graph(&st.kb_root) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response();
    }
    match crate::graph::backlinks(&st.kb_root, &p.path) {
        Ok(v) => Json(json!({ "path": p.path, "backlinks": v })).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({ "error": e }))).into_response(),
    }
}

async fn graph_linked(State(st): State<AppState>, Query(p): Query<GraphPathParams>) -> Response {
    if let Err(e) = ensure_graph(&st.kb_root) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response();
    }
    match crate::graph::linked(&st.kb_root, &p.path) {
        Ok(v) => Json(json!({ "path": p.path, "linked": v })).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({ "error": e }))).into_response(),
    }
}

async fn graph_related(State(st): State<AppState>, Query(p): Query<GraphPathParams>) -> Response {
    if let Err(e) = ensure_graph(&st.kb_root) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response();
    }
    match crate::graph::related(&st.kb_root, &p.path) {
        Ok(v) => Json(json!({ "path": p.path, "related": v })).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({ "error": e }))).into_response(),
    }
}

async fn graph_orphans(State(st): State<AppState>) -> Response {
    if let Err(e) = ensure_graph(&st.kb_root) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response();
    }
    match crate::graph::orphans(&st.kb_root) {
        Ok(v) => Json(json!({ "orphans": v })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    }
}

async fn graph_tags(State(st): State<AppState>) -> Response {
    if let Err(e) = ensure_graph(&st.kb_root) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response();
    }
    match crate::graph::tags(&st.kb_root) {
        Ok(v) => Json(json!({ "tags": v })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    }
}

async fn graph_projects(State(st): State<AppState>) -> Response {
    if let Err(e) = ensure_graph(&st.kb_root) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response();
    }
    match crate::graph::projects(&st.kb_root) {
        Ok(v) => Json(json!({ "projects": v })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    }
}

/// 掩码 API Key：仅显示前 3 + 后 4 位
fn mask_key(key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    if key.len() <= 8 {
        return "****".to_string();
    }
    let head: String = key.chars().take(3).collect();
    let tail: String = key.chars().rev().take(4).collect::<String>().chars().rev().collect();
    format!("{head}****{tail}")
}

async fn config_get() -> Json<serde_json::Value> {
    let mut v = serde_json::to_value(crate::config::load()).unwrap_or_default();
    if let Some(llm) = v.get_mut("llm") {
        if let Some(k) = llm.get("api_key").and_then(|k| k.as_str()) {
            llm["api_key"] = json!(mask_key(k));
        }
    }
    Json(v)
}

async fn config_set(Json(body): Json<serde_json::Value>) -> Response {
    let mut cfg = crate::config::load();
    // 部分更新：只覆盖给定字段
    if let Some(kb) = body.get("kb_root").and_then(|v| v.as_str()) {
        if !kb.trim().is_empty() {
            cfg.kb_root = kb.to_string();
        }
    }
    if let Some(llm) = body.get("llm") {
        if let Ok(mut l) = serde_json::from_value::<crate::config::LlmConfig>(llm.clone()) {
            // 掩码（含 *）或空串 → 保留旧 key（前端未改动时不清空）
            if l.api_key.contains('*') || l.api_key.is_empty() {
                l.api_key = cfg.llm.api_key.clone();
            }
            cfg.llm = l;
        }
    }
    match crate::config::save(&cfg) {
        Ok(()) => {
            let mut v = serde_json::to_value(cfg).unwrap_or_default();
            if let Some(llm) = v.get_mut("llm") {
                if let Some(k) = llm.get("api_key").and_then(|k| k.as_str()) {
                    llm["api_key"] = json!(mask_key(k));
                }
            }
            Json(json!({ "ok": true, "config": v })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// LLM 代理：转发到 Ollama / OpenAI 兼容接口。
/// body 为 OpenAI chat 格式：`{ messages, model?, temperature?, stream? }`；
/// 模型缺省用配置值；stream=true 走 SSE 流式透传，否则 JSON 透传。
async fn llm_chat(Json(body): Json<serde_json::Value>) -> Response {
    let cfg = crate::config::load();
    if cfg.llm.endpoint.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "未配置 LLM。请先 POST /api/config 设置 llm.endpoint（如 http://127.0.0.1:11434）与 llm.model。"
            })),
        )
            .into_response();
    }
    let has_model = cfg.llm.model.trim().is_empty()
        && body
            .get("model")
            .and_then(|m| m.as_str())
            .map(|s| s.trim().is_empty())
            .unwrap_or(true);
    if has_model {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "未配置 LLM 模型（llm.model 或请求体 model 字段）。" })),
        )
            .into_response();
    }
    let messages_ok = body
        .get("messages")
        .and_then(|m| m.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if !messages_ok {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "请求体需要非空 messages 数组（OpenAI chat 格式）。" })),
        )
            .into_response();
    }
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let ep = cfg.llm.endpoint.clone();
    let model = cfg.llm.model.clone();
    let key = cfg.llm.api_key.clone();
    if stream {
        match crate::llm::chat_stream(&ep, &model, &key, body).await {
            Ok(r) => r.into_response(),
            Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e }))).into_response(),
        }
    } else {
        match crate::llm::chat(&ep, &model, &key, body).await {
            Ok(v) => Json(v).into_response(),
            Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e }))).into_response(),
        }
    }
}
