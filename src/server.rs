//! Axum HTTP 服务：托管 xterm.js 前端 + 知识库接口（同源，免 CORS）。

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct AppState {
    pub kb_root: PathBuf,
    pub web_dir: PathBuf,
    /// 同步互斥锁：心跳与手动写端点（sync/approve/link）共用，防并发写
    pub sync_lock: Arc<tokio::sync::Mutex<()>>,
    /// 心跳状态（跨线程共享）
    pub hb_status: Arc<std::sync::Mutex<crate::heartbeat::HeartbeatStatus>>,
}

pub async fn serve(
    port: u16,
    kb_root: PathBuf,
    web_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    crate::kb::ensure_layout(&kb_root)?;
    let state = AppState {
        kb_root: kb_root.clone(),
        web_dir,
        sync_lock: Arc::new(tokio::sync::Mutex::new(())),
        hb_status: Arc::new(std::sync::Mutex::new(Default::default())),
    };
    // 心跳自动同步任务（默认关闭；开关走 config）
    {
        let state = state.clone();
        tokio::spawn(heartbeat_loop(state));
    }

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/tools", get(tools_handler))
        .route("/api/skills", get(skills_handler))
        .route("/api/consolidate", post(consolidate_handler))
        .route("/api/search", get(search_handler))
        .route("/api/l1", get(l1_handler))
        .route("/api/l1/read", get(l1_read_handler))
        .route("/api/file", get(file_read))
        .route("/api/file", post(file_write))
        .route("/api/sessions", get(sessions_list))
        .route("/api/kb/sync", post(kb_sync))
        .route("/api/kb/pending", get(kb_pending_list))
        .route("/api/kb/pending/preview", get(kb_pending_preview))
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
        .route("/api/heartbeat", get(heartbeat_get))
        .route("/api/heartbeat", post(heartbeat_set))
        .route("/api/link", post(link_add))
        .route("/api/link/suggest", post(link_suggest))
        .route("/api/fetch", get(fetch_page))
        .route("/api/page", get(page_read))
        .route("/api/page/act", post(page_act))
        .route("/api/tasks", get(tasks_list))
        .route("/api/tasks", post(tasks_create))
        .route("/api/tasks/{id}", patch(tasks_update))
        .route("/api/tasks/{id}", delete(tasks_delete))
        .route("/api/tasks/stats", get(tasks_stats))
        .route("/api/config", get(config_get))
        .route("/api/config", post(config_set))
        .route("/api/llm", post(llm_chat))
        .route("/api/context/log", post(context_log))
        .route("/api/context/stats", get(context_stats))
        .route("/api/apps", get(apps_list))
        .route("/api/apps/{id}/data", get(app_data_get).post(app_data_post))
        .route("/api/hubs", get(hubs_list))
        .route("/api/hubs/connect", post(hubs_connect))
        .route("/api/hubs/refresh", post(hubs_refresh))
        .route("/api/hubs/disconnect", post(hubs_disconnect))
        .route("/api/market/catalog", get(market_catalog))
        .route("/api/market/install", post(market_install))
        .route("/api/market/uninstall", post(market_uninstall))
        .route("/api/market/update", post(market_update))
        // 应用市场（阶段 0）：kb/apps/ 静态挂载到 /apps/*——沙箱 iframe 加载 app 的 HTML+assets（脚本子资源不受 CORS 限制）；/api/* 仍只走桥（沙箱 opaque origin 直连被拦，权限白名单在桥层）
        .nest_service("/apps", tower_http::services::ServeDir::new(state.kb_root.join("apps")))
        .fallback_service(tower_http::services::ServeDir::new(state.web_dir.clone()))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// ---------- handlers ----------

/// 声明式工具清单（Phase 3-C Step 1）：LLM 显式工具调用的决策依据；
/// 工具全部映射到现有端点（前端编排执行，宿主侧仅声明）。
fn tools_json() -> Value {
    json!([
        {
            "name": "search",
            "desc": "检索知识库 L2 内容层（全文 grep，多关键词任一命中，返回命中片段与所属小节）",
            "params": [
                {"name": "q", "type": "string", "required": true, "desc": "检索关键词，空格分隔"},
                {"name": "layer", "type": "string", "required": false, "desc": "notes（默认，仅内容层）| all（含 L1 规范/记忆）"},
                {"name": "ctx", "type": "string", "required": false, "desc": "传 1 返回命中行前后上下文片段"}
            ],
            "example": "{\"q\":\"托盘 架构\",\"layer\":\"notes\",\"ctx\":\"1\"}"
        },
        {
            "name": "read_l1",
            "desc": "读取知识库 L1 规范/记忆/索引层（KB/FRAMEWORK/RULES/MEMORY/INDEX/记忆摘要）。回答涉及规范约定、历史决策、既有记忆且前缀摘要不足时，先调它定位原文",
            "params": [
                {"name": "file", "type": "string", "required": true, "desc": "L1 文件名：KB.md|FRAMEWORK.md|RULES.md|MEMORY.md|INDEX.md|memory_summary.md"},
                {"name": "q", "type": "string", "required": false, "desc": "定位词：空=返回文件头+章节清单；非空=返回第一个含该词的 ## 小节原文"},
                {"name": "max_chars", "type": "string", "required": false, "desc": "字符上限（默认 1200）"}
            ],
            "example": "{\"file\":\"FRAMEWORK.md\",\"q\":\"双链\"}"
        },
        {
            "name": "memory_search",
            "desc": "记忆检索：检索整个持久记忆系统（L1 规范/记忆/索引 + L2 内容层），返回 top 片段——回答涉及历史决策/规范/既有知识时先查这里",
            "params": [
                {"name": "q", "type": "string", "required": true, "desc": "检索关键词，空格分隔"}
            ],
            "example": "{\"q\":\"双层记忆 架构\"}"
        },
        {
            "name": "graph.linked",
            "desc": "查文档的出链（该文档的 [[双链]] 指向谁，含悬空检测）",
            "params": [
                {"name": "path", "type": "string", "required": true, "desc": "文档相对 KB 根路径，如 notes/架构/托盘应用.md"}
            ],
            "example": "{\"path\":\"notes/架构/托盘应用.md\"}"
        },
        {
            "name": "graph.backlinks",
            "desc": "查文档的入链（谁链向该文档）",
            "params": [
                {"name": "path", "type": "string", "required": true, "desc": "文档相对 KB 根路径"}
            ],
            "example": "{\"path\":\"notes/架构/托盘应用.md\"}"
        },
        {
            "name": "fetch",
            "desc": "抓取网页正文（静态 HTTP + HTML 解析，无 JS 渲染）",
            "params": [
                {"name": "url", "type": "string", "required": true, "desc": "完整 URL，如 https://example.com"}
            ],
            "example": "{\"url\":\"https://example.com\"}"
        },
        {
            "name": "page",
            "desc": "读取动态网页正文（headless Edge/Chrome，等 JS 渲染，较慢约 5-10s）",
            "params": [
                {"name": "url", "type": "string", "required": true, "desc": "完整 URL"}
            ],
            "example": "{\"url\":\"https://example.com\"}"
        },
        {
            "name": "file",
            "desc": "读取 KB 内 Markdown 文件全文（L1 或 L2）",
            "params": [
                {"name": "path", "type": "string", "required": true, "desc": "文件相对 KB 根路径，如 MEMORY.md 或 notes/xxx.md"}
            ],
            "example": "{\"path\":\"MEMORY.md\"}"
        },
        {
            "name": "tasks",
            "desc": "列出任务引擎的当前任务（状态机：待办/进行中/完成/放弃）",
            "params": [],
            "example": "{}"
        },
        {
            "name": "market.connect",
            "desc": "连接第三方 SkillHub 商店（应用市场索引）：拉取并校验 hub 索引后入库，返回 hub 名与可用应用清单——用户要求安装/连接/添加应用商店、应用市场、SkillHub 时使用",
            "params": [
                {"name": "hub_url", "type": "string", "required": true, "desc": "hub 索引 URL，如 https://skillhub.cn/install/skillhub.md"}
            ],
            "example": "{\"hub_url\":\"https://skillhub.cn/install/skillhub.md\"}"
        }
    ])
}

async fn tools_handler() -> Json<Value> {
    Json(tools_json())
}

/// 技能注册表（Phase 3-C Step 2：trigger 触发注入用）
async fn skills_handler(State(st): State<AppState>) -> Json<Value> {
    let items: Vec<Value> = crate::kb::list_skills(&st.kb_root)
        .into_iter()
        .map(|s| json!({ "name": s.name, "title": s.title, "trigger": s.trigger, "desc": s.desc }))
        .collect();
    Json(json!({ "skills": items }))
}

/// 巩固器：按确定性规则（MEMORY 去重 / 重复标题提示）生成巩固提案进待审
#[derive(Deserialize)]
struct ConsolidateParams {
    /// 1/true/on 时启用 v2：LLM 生成重复标题文档的整合版（需配置 LLM）
    #[serde(default)]
    llm: String,
}

async fn consolidate_handler(State(st): State<AppState>, Query(q): Query<ConsolidateParams>) -> Response {
    let audit = match crate::graph::audit(&st.kb_root) {
        Ok(a) => a,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    };
    let mut created = match crate::consolidate::generate_proposals(&st.kb_root, &audit) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    if matches!(q.llm.as_str(), "1" | "true" | "on" | "yes") {
        match crate::consolidate::generate_llm_proposals(&st.kb_root, &audit).await {
            Ok(mut llm_created) => created.append(&mut llm_created),
            Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
        }
    }
    Json(json!({ "ok": true, "created": created })).into_response()
}

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
struct L1ReadParams {
    /// L1 文件名（白名单：KB.md/FRAMEWORK.md/RULES.md/MEMORY.md/INDEX.md/memory_summary.md）
    file: Option<String>,
    /// 定位词：定位第一个含 q 的 `## ` 小节
    q: Option<String>,
    /// 字符上限（默认 1200；超出部分截断）
    max: Option<usize>,
}

/// read_l1（上下文组装 v2：LLM 显式工具取用 L1 规范/记忆/索引层）。
/// 参数行为矩阵：无 file → 400；file+无 q → head+章节清单；file+q 命中 → 小节原文；未命中 → 章节清单。
/// 白名单外 → 400；返回源文件原文（memory_summary 是允许读的派生产物）。
async fn l1_read_handler(State(st): State<AppState>, Query(p): Query<L1ReadParams>) -> Response {
    let Some(file) = p.file.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "缺 file 参数（KB.md/FRAMEWORK.md/RULES.md/MEMORY.md/INDEX.md/memory_summary.md）" })),
        )
            .into_response();
    };
    let max = p.max.unwrap_or(1200).min(20_000);
    match crate::kb::read_l1(&st.kb_root, file, p.q.as_deref(), max) {
        Ok(r) => Json(r).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
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

// ---------- 会话管理（A2）：lite 枚举 kb/sessions/（frontmatter 元数据，不读全文） ----------
// 列表/恢复是"实体层"操作；sessions 流水本身仍三排除（不入图谱/检索/指纹）
async fn sessions_list(State(st): State<AppState>) -> Response {
    let dir = st.kb_root.join("sessions");
    let mut items: Vec<Value> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_string();
            let mtime = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            // 只读文件头 2KB（frontmatter 区），不读全文
            let head = std::fs::read(&path)
                .ok()
                .map(|b| {
                    let n = b.len().min(2048);
                    String::from_utf8_lossy(&b[..n]).to_string()
                })
                .unwrap_or_default();
            let (title, status, count, date) = parse_session_frontmatter(&head, &id);
            items.push(json!({
                "id": id,
                "title": title,
                "status": status,
                "count": count,
                "date": date,
                "mtime": mtime,
            }));
        }
    }
    items.sort_by(|a, b| b["mtime"].as_u64().unwrap_or(0).cmp(&a["mtime"].as_u64().unwrap_or(0)));
    Json(json!({ "sessions": items, "total": items.len() })).into_response()
}

/// 解析会话文件头 frontmatter（title/status/count/date）；旧文件缺字段容错。
/// title 缺省回退：首条 `## Q:` 截断 30 字 → 仍空则用文件名；status 缺省 archived（历史会话）
fn parse_session_frontmatter(head: &str, id: &str) -> (String, String, u64, String) {
    let mut title = String::new();
    let mut status = "archived".to_string();
    let mut count = 0u64;
    let mut date = String::new();
    let fm = head.split("---").nth(1).unwrap_or("");
    for line in fm.lines() {
        if let Some((k, v)) = line.split_once(':') {
            match k.trim() {
                "title" => title = v.trim().to_string(),
                "status" => status = v.trim().to_string(),
                "count" => count = v.trim().parse().unwrap_or(0),
                "date" => date = v.trim().to_string(),
                _ => {}
            }
        }
    }
    if title.is_empty() {
        if let Some(qi) = head.find("## Q:") {
            let rest = head[qi + 5..].trim_start();
            title = rest.lines().next().unwrap_or("").chars().take(30).collect();
        }
        if title.is_empty() {
            title = id.to_string();
        }
    }
    (title, status, count, date)
}

async fn kb_sync(State(st): State<AppState>) -> Response {
    let _guard = st.sync_lock.lock().await; // 与心跳共用互斥，防并发写
    match crate::kb::sync_index(&st.kb_root) {
        Ok(r) => {
            let _ = crate::kb::sync_skills(&st.kb_root); // 技能注册表顺带重建（技能提案经 approve 安装）
            Json(json!({ "ok": true, "index": r.index_path, "files": r.files })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ---------- 心跳自动同步（自组织自动发现侧） ----------

/// 心跳循环：每秒 tick；config 文件变化（开关/周期改动）即时重载生效；
/// 开启时按周期累积，到点指纹比对，变化才重建 INDEX+图谱，重建后顺带跑审计摘要
async fn heartbeat_loop(state: AppState) {
    let mut last_key: Option<String> = None;
    loop {
        let cfg = crate::config::load();
        let mut interval = cfg.heartbeat.interval_secs.max(5).min(3600);
        let mut acc: u64 = 0;
        let mut cfg_mtime = config_mtime();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            acc += 1;
            // config 文件变化 → 立即重读（开关/周期 ≤1s 生效）
            if config_mtime() != cfg_mtime {
                break;
            }
            if cfg.heartbeat.enabled && acc >= interval {
                break;
            }
            if !cfg.heartbeat.enabled {
                // 关闭时也要响应 config 变化（上面的 mtime 检查已覆盖），空闲等待即可
                continue;
            }
        }
        let cfg2 = crate::config::load();
        interval = cfg2.heartbeat.interval_secs.max(5).min(3600);
        if !cfg2.heartbeat.enabled {
            last_key = None; // 关闭时重置，重开后首轮同步
            continue;
        }
        // 开启：指纹比对，变化才重建
        let fp = crate::heartbeat::fingerprint(&state.kb_root);
        let key = crate::heartbeat::fingerprint_key(&fp);
        let changed = last_key.as_deref() != Some(key.as_str());
        if last_key.is_none() || changed {
            // 锁内重建（同步 std::fs，锁内无 await 点）
            let _guard = state.sync_lock.lock().await;
            let _ = crate::kb::sync_index(&state.kb_root);
            let _ = crate::kb::sync_skills(&state.kb_root); // 技能注册表顺带重建
            let _ = crate::graph::sync_graph(&state.kb_root);
            let audit = crate::graph::audit(&state.kb_root).ok();
            let brief = audit.map(|a| crate::heartbeat::AuditBrief {
                orphans: a.orphans.len(),
                dangling: a.dangling.len(),
                duplicates: a.duplicates.len(),
                mentions: a.mentions.len(),
            });
            if let Ok(mut st) = state.hb_status.lock() {
                st.enabled = true;
                st.interval_secs = interval;
                st.last_sync = Some(chrono::Local::now().format("%H:%M:%S").to_string());
                st.files = fp.len();
                st.changed = changed;
                st.audit = brief;
            }
            last_key = Some(key);
        }
    }
}

/// config.json 的修改时间（秒），用于检测配置变化即时生效
fn config_mtime() -> Option<i64> {
    std::fs::metadata(crate::config::config_path())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

async fn heartbeat_get(State(st): State<AppState>) -> Response {
    let cfg = crate::config::load();
    let status = st.hb_status.lock().map(|g| g.clone()).unwrap_or_default();
    Json(json!({
        "enabled": cfg.heartbeat.enabled,
        "interval_secs": cfg.heartbeat.interval_secs,
        "last_sync": status.last_sync,
        "files": status.files,
        "changed": status.changed,
        "audit": status.audit,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct HeartbeatSetBody {
    enabled: Option<bool>,
    interval_secs: Option<u64>,
}

async fn heartbeat_set(State(st): State<AppState>, Json(b): Json<HeartbeatSetBody>) -> Response {
    let mut cfg = crate::config::load();
    if let Some(e) = b.enabled {
        cfg.heartbeat.enabled = e;
    }
    if let Some(i) = b.interval_secs {
        cfg.heartbeat.interval_secs = i.max(5).min(3600);
    }
    if let Err(e) = crate::config::save(&cfg) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
    }
    if let Ok(mut s) = st.hb_status.lock() {
        s.enabled = cfg.heartbeat.enabled;
        s.interval_secs = cfg.heartbeat.interval_secs;
    }
    Json(json!({ "ok": true, "enabled": cfg.heartbeat.enabled, "interval_secs": cfg.heartbeat.interval_secs }))
        .into_response()
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
    let _guard = st.sync_lock.lock().await; // 与心跳共用互斥
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

/// 记忆关联建议（记忆断链修复 B）：给定记忆条目文本，返回相关 L2 文档（词重叠评分，
/// 命中词数 ≥2 优先）。前端写回 MEMORY 进待审时调用，生成「相关：[[双链]]」建议行交人审。
#[derive(Deserialize)]
struct LinkSuggestBody {
    content: String,
}

async fn link_suggest(State(st): State<AppState>, Json(body): Json<LinkSuggestBody>) -> Response {
    match crate::search::suggest_links(&st.kb_root, &body.content, 3) {
        Ok(links) => Json(json!({ "ok": true, "links": links })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        )
            .into_response(),
    }
}

// ---------- 网页读取（/fetch 静态抓取） ----------

#[derive(Deserialize)]
struct FetchParams {
    url: String,
}

async fn fetch_page(Query(p): Query<FetchParams>) -> Response {
    match crate::fetch::fetch_page(&p.url).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e }))).into_response(),
    }
}

/// /page 动态网页（headless Chrome/Edge CDP，等 JS 渲染）
async fn page_read(Query(p): Query<FetchParams>) -> Response {
    match crate::page::extract_page(&p.url).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e }))).into_response(),
    }
}

/// /page act：动作执行（click/fill/select/scroll；前端人审清单确认后调用）
#[derive(Deserialize)]
struct PageActBody {
    url: String,
    actions: Vec<crate::page::ActStep>,
}

async fn page_act(Json(b): Json<PageActBody>) -> Response {
    match crate::page::act_page(&b.url, &b.actions).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e }))).into_response(),
    }
}

// ---------- 待审机制 ----------

async fn kb_pending_list(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({ "pending": crate::kb::list_pending(&st.kb_root) }))
}

async fn kb_pending_preview(State(st): State<AppState>, Query(p): Query<GraphPathParams>) -> Response {
    match crate::kb::preview_pending(&st.kb_root, &p.path) {
        Ok(prev) => Json(prev).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({ "error": e }))).into_response(),
    }
}

#[derive(Deserialize)]
struct PendingBody {
    path: String,
    #[serde(default)]
    content: Option<String>,
}

async fn kb_pending_approve(State(st): State<AppState>, Json(body): Json<PendingBody>) -> Response {
    let _guard = st.sync_lock.lock().await; // 与心跳共用互斥
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
        match crate::kb::approve_pending(&st.kb_root, &p, body.content.as_deref()) {
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
    let _guard = st.sync_lock.lock().await; // 与心跳共用互斥
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
    if let Some(hb) = body.get("heartbeat") {
        if let Ok(h) = serde_json::from_value::<crate::config::HeartbeatConfig>(hb.clone()) {
            cfg.heartbeat.enabled = h.enabled;
            cfg.heartbeat.interval_secs = h.interval_secs.clamp(5, 3600);
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

// ---------- 任务引擎（Phase 3-B，kb/.tasks.db 独立库） ----------

#[derive(Deserialize)]
struct TaskCreateBody {
    goal: String,
    #[serde(default)]
    title: String,
}

#[derive(Deserialize)]
struct TaskUpdateBody {
    status: Option<String>,
    note: Option<String>,
    deps: Option<Vec<String>>,
}

async fn tasks_list(State(s): State<AppState>) -> Response {
    match crate::task::list(&s.kb_root) {
        Ok(t) => Json(json!({ "tasks": t, "stats": crate::task::stats(&s.kb_root).unwrap_or(json!({})) }))
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    }
}

async fn tasks_stats(State(s): State<AppState>) -> Response {
    match crate::task::stats(&s.kb_root) {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    }
}

async fn tasks_create(State(s): State<AppState>, Json(b): Json<TaskCreateBody>) -> Response {
    match crate::task::create(&s.kb_root, &b.goal, &b.title) {
        Ok(t) => (StatusCode::CREATED, Json(json!({ "task": t }))).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

async fn tasks_update(
    State(s): State<AppState>,
    AxumPath(id): AxumPath<i64>,
    Json(b): Json<TaskUpdateBody>,
) -> Response {
    match crate::task::update(&s.kb_root, id, b.status.as_deref(), b.note.as_deref(), b.deps.as_deref()) {
        Ok(t) => Json(json!({ "task": t })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

async fn tasks_delete(State(s): State<AppState>, AxumPath(id): AxumPath<i64>) -> Response {
    match crate::task::remove(&s.kb_root, id) {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": format!("任务 #{id} 不存在") }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
    }
}

// ---------- 应用市场（阶段 1：已安装 app 列表，manifest 在 kb/apps/<id>/app.json） ----------
async fn apps_list(State(s): State<AppState>) -> Response {
    Json(json!({ "apps": crate::kb::list_apps(&s.kb_root) })).into_response()
}

// ---------- App 状态持久化（storage 权限）：kb/apps/<id>/data/localstorage.json ----------
// 桥层 localStorage 代理的落盘目标（沙箱无 allow-same-origin → localStorage 不可用，代理经此端点持久化）。
// 宿主桥已校验 tab.appId 与 storage 权限；端点自身防御 id 穿越。

#[derive(Deserialize)]
struct AppDataBody {
    data: serde_json::Value,
}

/// app id → 数据文件路径；id 必须是 app 目录名（拒绝路径分隔符/穿越）
fn app_data_file(kb_root: &std::path::Path, id: &str) -> Option<std::path::PathBuf> {
    if id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || id.contains(':')
        || id == "."
    {
        return None;
    }
    Some(kb_root.join("apps").join(id).join("data").join("localstorage.json"))
}

fn read_app_data(kb_root: &std::path::Path, id: &str) -> serde_json::Value {
    match app_data_file(kb_root, id).and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(s) => serde_json::from_str(&s).unwrap_or_else(|_| serde_json::json!({})),
        None => serde_json::json!({}), // 首次无文件 → 空对象
    }
}

fn write_app_data(kb_root: &std::path::Path, id: &str, v: &serde_json::Value) -> Result<std::path::PathBuf, String> {
    let p = app_data_file(kb_root, id).ok_or_else(|| "非法 app id".to_string())?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建 data 目录失败: {e}"))?;
    }
    let s = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    std::fs::write(&p, s).map_err(|e| format!("写入失败: {e}"))?;
    Ok(p)
}

async fn app_data_get(State(s): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    if app_data_file(&s.kb_root, &id).is_none() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "非法 app id" }))).into_response();
    }
    Json(json!({ "data": read_app_data(&s.kb_root, &id) })).into_response()
}

async fn app_data_post(State(s): State<AppState>, AxumPath(id): AxumPath<String>, Json(b): Json<AppDataBody>) -> Response {
    let v = if b.data.is_object() { b.data } else { serde_json::json!({}) };
    match write_app_data(&s.kb_root, &id, &v) {
        Ok(p) => Json(json!({ "ok": true, "path": p.display().to_string() })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

#[cfg(test)]
mod app_data_tests {
    use super::*;

    fn tmp_kb(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("md-agent-appdata-{}-{}", name, std::process::id()))
    }

    #[test]
    fn id_validation_rejects_traversal() {
        let kb = tmp_kb("valid");
        assert!(app_data_file(&kb, "match").is_some());
        assert!(app_data_file(&kb, "../evil").is_none());
        assert!(app_data_file(&kb, "a/b").is_none());
        assert!(app_data_file(&kb, "a\\b").is_none());
        assert!(app_data_file(&kb, "").is_none());
        assert!(app_data_file(&kb, "c:evil").is_none());
        let _ = std::fs::remove_dir_all(&kb);
    }

    #[test]
    fn read_write_roundtrip() {
        let kb = tmp_kb("rt");
        // 无文件 → 空对象
        assert_eq!(read_app_data(&kb, "match"), serde_json::json!({}));
        // 写入 → 读回
        let v = serde_json::json!({ "candidates": [{ "name": "张三" }], "count": 3 });
        write_app_data(&kb, "match", &v).expect("write");
        assert_eq!(read_app_data(&kb, "match"), v);
        // 覆盖写
        let v2 = serde_json::json!({ "candidates": [] });
        write_app_data(&kb, "match", &v2).expect("write2");
        assert_eq!(read_app_data(&kb, "match"), v2);
        let _ = std::fs::remove_dir_all(&kb);
    }

    #[test]
    fn write_rejects_bad_id() {
        let kb = tmp_kb("badid");
        assert!(write_app_data(&kb, "../x", &serde_json::json!({})).is_err());
        let _ = std::fs::remove_dir_all(&kb);
    }

    #[test]
    fn cache_breaker_attribution() {
        use serde_json::json;
        let entries = vec![
            json!({"fp_system": "a", "fp_skills": "s1", "fp_mid": "m1", "fp_user": "u1"}),
            // 仅 user 变 → 尾部增长（正常）
            json!({"fp_system": "a", "fp_skills": "s1", "fp_mid": "m1", "fp_user": "u2"}),
            // mid 变（首个变化桶=mid，user 同时变不算）
            json!({"fp_system": "a", "fp_skills": "s1", "fp_mid": "m2", "fp_user": "u3"}),
            // system 变 → 整前缀炸
            json!({"fp_system": "b", "fp_skills": "s1", "fp_mid": "m2", "fp_user": "u3"}),
            // skills 变
            json!({"fp_system": "b", "fp_skills": "s2", "fp_mid": "m2", "fp_user": "u3"}),
            // 完全相同 → 不计数
            json!({"fp_system": "b", "fp_skills": "s2", "fp_mid": "m2", "fp_user": "u3"}),
            // 旧格式无指纹 → 跳过（不污染 prev），后续有指纹条目与最近一次有指纹条目比较
            json!({"input_tokens": 100}),
            // prev 仍为 e6（b,s2,m2,u3）：system b→c 变化 → 再计一次 system
            json!({"fp_system": "c", "fp_skills": "s2", "fp_mid": "m2", "fp_user": "u3"}),
        ];
        let b = cache_breakers(&entries);
        assert_eq!(b.get("user"), Some(&1));
        assert_eq!(b.get("mid"), Some(&1));
        assert_eq!(b.get("system"), Some(&2));
        assert_eq!(b.get("skills"), Some(&1));
        assert_eq!(b.len(), 4);
    }

    #[test]
    fn session_frontmatter_parse() {
        // 新格式：完整 frontmatter
        let head = "---\ntype: session\ndate: 2026-08-05\ntitle: 如何配置 LLM？\nstatus: active\ncount: 3\n---\n\n# 会话记录\n\n## Q: 如何配置 LLM？\nA: 打开 config.html\n";
        let (t, s, c, d) = parse_session_frontmatter(head, "2026-08-05-120000");
        assert_eq!(t, "如何配置 LLM？");
        assert_eq!(s, "active");
        assert_eq!(c, 3);
        assert_eq!(d, "2026-08-05");
        // 旧格式：无 frontmatter → title 回退首条 ## Q:（截断 30 字），status 缺省 archived
        let old = "# 会话记录\n\n## Q: 这是一条很长的问题用来验证标题截断逻辑是否正确生效啊啊啊啊啊啊啊啊\nA: 回答\n";
        let (t2, s2, c2, _d2) = parse_session_frontmatter(old, "2026-08-04-154440");
        assert_eq!(s2, "archived");
        assert_eq!(c2, 0);
        assert_eq!(t2.chars().count(), 30);
        // 空文件 → title 回退文件名
        let (t3, s3, _, _) = parse_session_frontmatter("", "2026-08-04-154440");
        assert_eq!(t3, "2026-08-04-154440");
        assert_eq!(s3, "archived");
    }
}

// ---------- SkillHub（阶段 4）：hub 注册表 + 目录 ----------
#[derive(Deserialize)]
struct HubUrlBody {
    url: String,
}

#[derive(Deserialize)]
struct HubNameBody {
    name: String,
}

async fn hubs_list(State(s): State<AppState>) -> Response {
    Json(json!({ "hubs": crate::hub::list_hubs(&s.kb_root) })).into_response()
}

async fn hubs_connect(State(s): State<AppState>, Json(b): Json<HubUrlBody>) -> Response {
    match crate::hub::connect_hub(&s.kb_root, &b.url).await {
        Ok(h) => (StatusCode::CREATED, Json(json!({ "ok": true, "hub": h }))).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

async fn hubs_refresh(State(s): State<AppState>, Json(b): Json<HubNameBody>) -> Response {
    match crate::hub::refresh_hub(&s.kb_root, &b.name).await {
        Ok(h) => Json(json!({ "ok": true, "hub": h })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

async fn hubs_disconnect(State(s): State<AppState>, Json(b): Json<HubNameBody>) -> Response {
    match crate::hub::disconnect_hub(&s.kb_root, &b.name) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

/// 目录：已连接 hub 的合并 app 清单（条目带 source + 来源 hub 标记，供面板「目录」Tab / install 用）
async fn market_catalog(State(s): State<AppState>) -> Response {
    let hubs = crate::hub::list_hubs(&s.kb_root);
    let mut apps: Vec<Value> = Vec::new();
    for h in &hubs {
        for a in &h.apps {
            apps.push(json!({
                "id": a.id,
                "name": a.name,
                "version": a.version,
                "entry": a.entry,
                "permissions": a.permissions,
                "description": a.description,
                "source": a.source,
                "hub": h.name,
                "kind": a.kind,
            }));
        }
    }
    Json(json!({ "apps": apps })).into_response()
}

// ---------- 应用市场（阶段 2：安装/卸载/更新，本地路径导入通道） ----------
#[derive(Deserialize)]
struct MarketInstallBody {
    source: Option<String>,
    path: Option<String>,
    id: Option<String>,
    /// 来源 hub 名（面板目录安装时随 source 传入，落盘后记录到 app.json）
    hub: Option<String>,
    /// dry_run=true 只校验并返回 manifest（前端人审确认用），不落盘
    dry_run: Option<bool>,
}

#[derive(Deserialize)]
struct MarketUninstallBody {
    id: String,
}

async fn market_install(State(s): State<AppState>, Json(b): Json<MarketInstallBody>) -> Response {
    // hub 条目安装：source（git/zip/裸 md/local）→ 下载到临时目录 → 按包内容识别（app→kb/apps/，skill→kb/skills/）
    if let Some(src) = b.source.as_deref().filter(|s| !s.is_empty()) {
        let dry = b.dry_run.unwrap_or(false);
        let hub = b.hub.clone();
        let tmp = std::env::temp_dir().join(format!("md-agent-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        match crate::hub::download_app(src, &tmp).await {
            Ok(loc) => {
                let r = if dry {
                    crate::market::probe_bundle(&loc)
                        .map(|(kind, info)| json!({ "ok": true, "dry_run": true, "kind": kind, "app": info }))
                } else {
                    crate::market::install_bundle(&s.kb_root, &loc).map(|(kind, id)| {
                        let app = if kind == "app" {
                            crate::market::read_manifest(&s.kb_root.join("apps").join(&id))
                                .map(|m| json!(m))
                                .unwrap_or(json!({ "id": id }))
                        } else {
                            json!({ "id": id, "name": id, "version": "0.0.0" })
                        };
                        if kind == "app" {
                            if let Some(h) = hub.as_deref().filter(|h| !h.is_empty()) {
                                record_source_hub(&s.kb_root, &id, h);
                            }
                        }
                        json!({ "ok": true, "kind": kind, "id": id, "app": app })
                    })
                };
                let _ = std::fs::remove_dir_all(&tmp);
                match r {
                    Ok(v) => Json(v).into_response(),
                    Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
                }
            }
            Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("下载失败: {e}") }))).into_response(),
        }
    } else {
        // 本地路径通道（手动导入兜底）：path（应用目录 / 技能目录 / 裸 SKILL.md 文件）
        let path = b.path.unwrap_or_default();
        if path.trim().is_empty() {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "缺 path（本地路径）或 source（hub 条目）" }))).into_response();
        }
        let loc = std::path::PathBuf::from(&path);
        let r = if b.dry_run.unwrap_or(false) {
            crate::market::probe_bundle(&loc)
                .map(|(kind, info)| json!({ "ok": true, "dry_run": true, "kind": kind, "app": info }))
        } else {
            crate::market::install_bundle(&s.kb_root, &loc)
                .map(|(kind, id)| json!({ "ok": true, "kind": kind, "id": id, "app": json!({ "id": id }) }))
        };
        match r {
            Ok(v) => Json(v).into_response(),
            Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
        }
    }
}

/// 记录来源 hub：安装落盘后把 hub 名写回 kb/apps/<id>/app.json 的 source_hub 字段（幂等，失败静默）
fn record_source_hub(root: &std::path::Path, id: &str, hub: &str) {
    let mf = root.join("apps").join(id).join("app.json");
    let Ok(content) = std::fs::read_to_string(&mf) else { return };
    let Ok(mut v) = serde_json::from_str::<Value>(&content) else { return };
    v["source_hub"] = json!(hub);
    let _ = std::fs::write(&mf, v.to_string());
}

async fn market_uninstall(State(s): State<AppState>, Json(b): Json<MarketUninstallBody>) -> Response {
    match crate::market::uninstall(&s.kb_root, &b.id) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

async fn market_update(State(s): State<AppState>, Json(b): Json<MarketInstallBody>) -> Response {
    let id = b.id.unwrap_or_default();
    let path = b.path.unwrap_or_default();
    match crate::market::update_local(&s.kb_root, &id, &path) {
        Ok(m) => Json(json!({ "ok": true, "app": m })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

// ---------- Context Engineering 记账（CE 第 1 步：记账先行，零侵入） ----------
// 记录每次 Agent 请求的 token 用量与缓存命中（前端从 /api/llm 响应 usage 透传上报），
// 供 /api/context/stats 聚合——D1（缓存命中率是否核心）的数据来源。

#[derive(Deserialize)]
struct ContextLogBody {
    kind: Option<String>,
    tool_count: Option<u32>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read: Option<u64>,
    cache_creation: Option<u64>,
    total_tokens: Option<u64>,
    // per-source 分桶（方向 2）：src_* 为估算 token 数，fp_* 为内容指纹（miss 归因）
    src_system: Option<u64>,
    src_skills: Option<u64>,
    src_mid: Option<u64>,
    src_user: Option<u64>,
    fp_system: Option<String>,
    fp_skills: Option<String>,
    fp_mid: Option<String>,
    fp_user: Option<String>,
}

async fn context_log(State(s): State<AppState>, Json(b): Json<ContextLogBody>) -> Response {
    let line = json!({
        "ts": chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        "kind": b.kind.unwrap_or_else(|| "question".to_string()),
        "tool_count": b.tool_count.unwrap_or(0),
        "input_tokens": b.input_tokens.unwrap_or(0),
        "output_tokens": b.output_tokens.unwrap_or(0),
        "cache_read": b.cache_read.unwrap_or(0),
        "cache_creation": b.cache_creation.unwrap_or(0),
        "total_tokens": b.total_tokens.unwrap_or(0),
        "src_system": b.src_system.unwrap_or(0),
        "src_skills": b.src_skills.unwrap_or(0),
        "src_mid": b.src_mid.unwrap_or(0),
        "src_user": b.src_user.unwrap_or(0),
        "fp_system": b.fp_system.unwrap_or_default(),
        "fp_skills": b.fp_skills.unwrap_or_default(),
        "fp_mid": b.fp_mid.unwrap_or_default(),
        "fp_user": b.fp_user.unwrap_or_default(),
    });
    let path = s.kb_root.join(".context-log.jsonl");
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut f) => {
            let mut line = line.to_string();
            line.push('\n');
            if f.write_all(line.as_bytes()).is_err() {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "写入失败" }))).into_response();
            }
        }
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
        }
    }
    Json(json!({ "ok": true })).into_response()
}

async fn context_stats(State(s): State<AppState>) -> Response {
    let path = s.kb_root.join(".context-log.jsonl");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            return Json(json!({
                "total": 0, "by_kind": {}, "input_tokens": 0, "output_tokens": 0,
                "cache_read": 0, "cache_creation": 0, "cache_read_ratio": null,
                "avg_tool_count": 0, "overflow_count": 0,
                "src": { "system": 0, "skills": 0, "mid": 0, "user": 0, "total": 0,
                         "pct": { "system": 0.0, "skills": 0.0, "mid": 0.0, "user": 0.0 } },
                "cache_breakers": {}
            }))
            .into_response()
        }
    };
    let mut total = 0u64;
    let mut input = 0u64;
    let mut output = 0u64;
    let mut cr = 0u64;
    let mut cc = 0u64;
    let mut tools = 0u64;
    let mut tool_n = 0u64;
    let mut src_sys = 0u64;
    let mut src_sk = 0u64;
    let mut src_mid = 0u64;
    let mut src_user = 0u64;
    let mut by_kind: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut entries: Vec<Value> = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            total += 1;
            entries.push(v);
        }
    }
    for v in &entries {
        let kind = v["kind"].as_str().unwrap_or("question").to_string();
        *by_kind.entry(kind).or_insert(0) += 1;
        input += v["input_tokens"].as_u64().unwrap_or(0);
        output += v["output_tokens"].as_u64().unwrap_or(0);
        cr += v["cache_read"].as_u64().unwrap_or(0);
        cc += v["cache_creation"].as_u64().unwrap_or(0);
        src_sys += v["src_system"].as_u64().unwrap_or(0);
        src_sk += v["src_skills"].as_u64().unwrap_or(0);
        src_mid += v["src_mid"].as_u64().unwrap_or(0);
        src_user += v["src_user"].as_u64().unwrap_or(0);
        let tc = v["tool_count"].as_u64().unwrap_or(0);
        if tc > 0 {
            tools += tc;
            tool_n += 1;
        }
    }
    // 命中率近似：有 cache_creation（miss）时 = cr/(cr+cc)；否则 = cr/(cr+input) 兜底
    let ratio = if cr + cc > 0 {
        Some(cr as f64 / (cr + cc) as f64)
    } else if cr + input > 0 {
        Some(cr as f64 / (cr + input) as f64)
    } else {
        None
    };
    let breakers = cache_breakers(&entries);
    let src_total = src_sys + src_sk + src_mid + src_user;
    Json(json!({
        "total": total,
        "by_kind": by_kind,
        "input_tokens": input,
        "output_tokens": output,
        "cache_read": cr,
        "cache_creation": cc,
        "cache_read_ratio": ratio,
        "avg_tool_count": if tool_n > 0 { tools as f64 / tool_n as f64 } else { 0.0 },
        "overflow_count": 0,
        // per-source 分桶（方向 2）：估算 token 累计 + 占比；cache_breakers = 缓存断裂归因分布
        "src": {
            "system": src_sys,
            "skills": src_sk,
            "mid": src_mid,
            "user": src_user,
            "total": src_total,
            "pct": {
                "system": if src_total > 0 { src_sys as f64 / src_total as f64 } else { 0.0 },
                "skills": if src_total > 0 { src_sk as f64 / src_total as f64 } else { 0.0 },
                "mid": if src_total > 0 { src_mid as f64 / src_total as f64 } else { 0.0 },
                "user": if src_total > 0 { src_user as f64 / src_total as f64 } else { 0.0 },
            },
        },
        "cache_breakers": breakers,
    }))
    .into_response()
}

/// 缓存断裂归因：对连续记账条目，按序找第一个指纹变化的源分桶（system→skills→mid→user）。
/// 首个变化的桶 = 缓存边界断裂点（其前为可缓存稳定段）；全变（含 user）属正常尾部增长。
fn cache_breakers(entries: &[Value]) -> std::collections::HashMap<String, u64> {
    let mut breakers: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut prev: Option<(Option<String>, Option<String>, Option<String>, Option<String>)> = None;
    for v in entries {
        let cur = (
            v["fp_system"].as_str().map(String::from),
            v["fp_skills"].as_str().map(String::from),
            v["fp_mid"].as_str().map(String::from),
            v["fp_user"].as_str().map(String::from),
        );
        if cur.0.is_none() {
            continue; // 旧格式条目无指纹，跳过归因
        }
        if let Some(p) = &prev {
            let b = if cur.0 != p.0 {
                "system"
            } else if cur.1 != p.1 {
                "skills"
            } else if cur.2 != p.2 {
                "mid"
            } else if cur.3 != p.3 {
                "user"
            } else {
                ""
            };
            if !b.is_empty() {
                *breakers.entry(b.to_string()).or_insert(0) += 1;
            }
        }
        prev = Some(cur);
    }
    breakers
}
