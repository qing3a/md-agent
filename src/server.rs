//! Axum HTTP 服务：托管 xterm.js 前端 + 知识库接口（同源，免 CORS）。

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
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
        .route("/api/memory/touch", post(memory_touch))
        .route("/api/memory/heat", get(memory_heat))
        .route("/api/experience/propose", post(experience_propose))
        .route("/api/dev/read", get(dev_read))
        .route("/api/dev/status", get(dev_status))
        .route("/api/dev/diff", get(dev_diff))
        .route("/api/dev/patch", post(dev_patch))
        .route("/api/dev/apply", post(dev_apply))
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
        },
        {
            "name": "dev.read",
            "desc": "读取项目自身源码（src/web/scripts/Cargo.toml/README.md/.zcode/plans，白名单内）——回答涉及本项目代码实现、需要查阅自身源码时使用",
            "params": [
                {"name": "path", "type": "string", "required": true, "desc": "项目根相对路径，如 src/server.rs 或 web/app.js"}
            ],
            "example": "{\"path\":\"src/server.rs\"}"
        },
        {
            "name": "dev.status",
            "desc": "查看项目 git 工作区状态（git status --short，只读）",
            "params": [],
            "example": "{}"
        },
        {
            "name": "dev.diff",
            "desc": "查看项目未提交改动 diff（git diff，只读；可选 path 限定单文件）",
            "params": [
                {"name": "path", "type": "string", "required": false, "desc": "限定文件路径，如 src/server.rs；空=全部"}
            ],
            "example": "{\"path\":\"src/server.rs\"}"
        },
        {
            "name": "dev.patch",
            "desc": "提交代码修改提案（进待审人审）：指定要修改的文件路径与完整新内容——发现问题需要改代码时，生成提案而非直接改动",
            "params": [
                {"name": "reason", "type": "string", "required": true, "desc": "修改原因"},
                {"name": "files", "type": "array", "required": true, "desc": "文件列表：[{path: 项目根相对路径, content: 新内容全文}]"}
            ],
            "example": "{\"reason\":\"修复 xx\",\"files\":[{\"path\":\"src/server.rs\",\"content\":\"...\"}]}"
        },
        {
            "name": "dev.apply",
            "desc": "应用代码提案并构建验证（dev.patch 生成的提案）：备份→写入→cargo build→失败自动回滚",
            "params": [
                {"name": "path", "type": "string", "required": true, "desc": "待审提案路径，如 pending/code/20260805-120000.md"}
            ],
            "example": "{\"path\":\"pending/code/20260805-120000.md\"}"
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

// ---------- 读时整理（B1）：热度 + 规则层（零 LLM，fire-and-forget 旁路） ----------
// kb/.memory-heat.json 三排除：json 非 md，天然不入图谱/检索/指纹。
// 触发：前端 runTool 成功后旁路 touch；规则层只在「读满阈值且内容指纹变化」时跑一次（防噪音/防重复）
const HEAT_THRESHOLD: u64 = 3;
const LLM_HEAT_THRESHOLD: u64 = 5; // B2：读满 5 次 → LLM 推理层（矛盾/盲区/整合）

fn heat_path(root: &Path) -> PathBuf {
    root.join(".memory-heat.json")
}

fn load_heat(root: &Path) -> Value {
    std::fs::read_to_string(heat_path(root))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({ "paths": {} }))
}

fn save_heat(root: &Path, v: &Value) -> Result<(), String> {
    std::fs::write(heat_path(root), serde_json::to_string_pretty(v).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

/// djb2 内容指纹（与前端 fpOf 同构；规则层"内容变了才重新整理"的判据）
fn simple_fp(s: &str) -> String {
    let mut h: u64 = 5381;
    for b in s.bytes() {
        h = (h.wrapping_mul(33)).wrapping_add(b as u64);
    }
    format!("{:x}", h)
}

#[derive(Deserialize)]
struct MemoryTouchBody {
    query: Option<String>,
    paths: Option<Vec<String>>,
}

async fn memory_touch(State(st): State<AppState>, Json(b): Json<MemoryTouchBody>) -> Response {
    let paths = b.paths.unwrap_or_default();
    let (mut heat, created) = apply_memory_touch(&st.kb_root, &paths);
    // B2 LLM 推理层（阈值 5 + 已配置 + 未做过）：后台分析热文档 → 矛盾/盲区/整合提案
    let cfg = crate::config::load();
    if !cfg.llm.endpoint.trim().is_empty() {
        for p in &paths {
            let rel = p.trim().trim_start_matches('/');
            if rel.is_empty() || !rel.starts_with("notes/") {
                continue;
            }
            let e = heat["paths"].get(rel).cloned().unwrap_or_default();
            let count = e["read_count"].as_u64().unwrap_or(0);
            let fp = e["fp"].as_str().unwrap_or("").to_string();
            if should_run_llm_analysis(&heat, rel, count, &fp) {
                // 先标记防重复（后台任务只写提案，不再动 heat）
                heat["paths"][rel]["organized_llm_fp"] = json!(fp);
                let root = st.kb_root.clone();
                let rel2 = rel.to_string();
                tokio::spawn(async move { analyze_hot_doc(&root, &rel2).await });
            }
        }
    }
    let _ = save_heat(&st.kb_root, &heat);
    Json(json!({ "ok": true, "touched": paths.len(), "created": created })).into_response()
}

/// B2 触发条件（纯函数）：notes/ 内容层 + 读满阈值 + 内容指纹未分析过
fn should_run_llm_analysis(heat: &Value, rel: &str, count: u64, fp: &str) -> bool {
    if !rel.starts_with("notes/") || count < LLM_HEAT_THRESHOLD {
        return false;
    }
    let done = heat["paths"].get(rel).and_then(|v| v.get("organized_llm_fp")).and_then(Value::as_str).unwrap_or("");
    done != fp
}

/// 从 LLM 输出提取 JSON（容忍 ```json 包裹、前后文字；括号配对截取）
fn extract_json_from_text(text: &str) -> Option<Value> {
    let t = text.trim();
    let inner = if t.starts_with("```") {
        t.trim_start_matches("```")
            .trim_start_matches("json")
            .trim()
            .trim_end_matches("```")
            .trim()
    } else {
        t
    };
    let start = inner.find('{')?;
    // 括号配对：找到匹配的 }（跳过字符串内的括号）
    let bytes = inner[start..].as_bytes();
    let mut depth = 0i32;
    let mut end = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in bytes.iter().enumerate() {
        if esc {
            esc = false;
            continue;
        }
        if b == b'\\' && in_str {
            esc = true;
            continue;
        }
        if b == b'"' {
            in_str = !in_str;
            continue;
        }
        if in_str {
            continue;
        }
        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                end = i + 1;
                break;
            }
        }
    }
    if end == 0 {
        return None;
    }
    serde_json::from_str(&inner[start..start + end]).ok()
}

/// B2 后台任务：LLM 分析热文档 + 同目录相关文档（矛盾/盲区/整合）→ 提案进 pending/notes/自组织/
async fn analyze_hot_doc(root: &Path, rel: &str) {
    let Some(fb) = crate::kb::resolve_in_kb(root, rel) else { return };
    let Ok(main) = std::fs::read_to_string(&fb) else { return };
    // 相关文档：同目录 .md（排除自身，最多 3 个，各取 2000 字符）
    let mut related = String::new();
    if let Some(parent) = fb.parent() {
        if let Ok(rd) = std::fs::read_dir(parent) {
            let mut n = 0;
            for e in rd.flatten() {
                if n >= 3 {
                    break;
                }
                let p = e.path();
                if p == fb || p.extension().and_then(|x| x.to_str()) != Some("md") {
                    continue;
                }
                if let Ok(c) = std::fs::read_to_string(&p) {
                    if let Some(name) = p.file_name().and_then(|x| x.to_str()) {
                        related.push_str(&format!("### {name}\n{}\n\n", c.chars().take(2000).collect::<String>()));
                        n += 1;
                    }
                }
            }
        }
    }
    let cfg = crate::config::load();
    let system = "你是知识库自组织分析器。分析主文档及其同目录相关文档，找出：\n\
        1. conflicts：文档间互相冲突的表述\n\
        2. gaps：该主题明显缺失、值得补充的知识点\n\
        3. merges：重复/碎片可合并的文档\n\
        只输出 JSON（不要其它文字）：{\"conflicts\":[{\"a\":\"文档A\",\"b\":\"文档B\",\"note\":\"冲突说明\"}],\"gaps\":[{\"topic\":\"缺什么\",\"why\":\"为什么值得补\"}],\"merges\":[{\"docs\":[\"a.md\",\"b.md\"],\"why\":\"合并理由\"}]}\n\
        没有发现则对应数组为空。";
    let user = format!(
        "### 主文档 {rel}\n{}\n\n### 相关文档\n{}",
        main.chars().take(4000).collect::<String>(),
        related
    );
    let body = serde_json::json!({ "messages": [
        { "role": "system", "content": system },
        { "role": "user", "content": user },
    ]});
    let Ok(resp) = crate::llm::chat(&cfg.llm.endpoint, &cfg.llm.model, &cfg.llm.api_key, body).await else { return };
    let full = resp["choices"][0]["message"]["content"].as_str().unwrap_or("");
    let Some(v) = extract_json_from_text(full) else { return };
    let conflicts: Vec<Value> = v["conflicts"].as_array().cloned().unwrap_or_default();
    let gaps: Vec<Value> = v["gaps"].as_array().cloned().unwrap_or_default();
    let merges: Vec<Value> = v["merges"].as_array().cloned().unwrap_or_default();
    if conflicts.is_empty() && gaps.is_empty() && merges.is_empty() {
        return;
    }
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let safe = rel.replace(['/', '\\'], "_");
    let ppath = format!("pending/notes/自组织/推理建议-{}-{}.md", date, safe);
    let dst = root.join(&ppath);
    if dst.exists() {
        return;
    }
    let mut body_text = format!("# 读时推理建议\n\n> 来源：`{rel}` 被高频读取触发的 LLM 分析（派生产物，人工核对）\n\n");
    if !conflicts.is_empty() {
        body_text.push_str("\n## 矛盾\n");
        for c in &conflicts {
            body_text.push_str(&format!(
                "- **{}** vs **{}**：{}\n",
                c["a"].as_str().unwrap_or("?"),
                c["b"].as_str().unwrap_or("?"),
                c["note"].as_str().unwrap_or("")
            ));
        }
    }
    if !gaps.is_empty() {
        body_text.push_str("\n## 盲区\n");
        for g in &gaps {
            body_text.push_str(&format!(
                "- **{}**：{}\n",
                g["topic"].as_str().unwrap_or("?"),
                g["why"].as_str().unwrap_or("")
            ));
        }
    }
    if !merges.is_empty() {
        body_text.push_str("\n## 整合建议\n");
        for m in &merges {
            let docs = m["docs"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|d| d.as_str().unwrap_or("?").to_string())
                .collect::<Vec<_>>()
                .join("、");
            body_text.push_str(&format!("- `{docs}`：{}\n", m["why"].as_str().unwrap_or("")));
        }
    }
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&dst, body_text);
}

/// 纯逻辑：热度 +1、阈值触发规则层（补链建议进 pending）。返回更新后的 heat 与新建提案路径。
fn apply_memory_touch(root: &Path, paths: &[String]) -> (Value, Vec<String>) {
    let mut heat = load_heat(root);
    let mut created: Vec<String> = Vec::new();
    let now = chrono::Local::now().timestamp();
    for p in paths {
        let rel = p.trim().trim_start_matches('/');
        if rel.is_empty() {
            continue;
        }
        let mut e = heat["paths"].get(rel).cloned().unwrap_or_else(|| {
            json!({ "read_count": 0, "last_read": 0, "fp": "" })
        });
        e["read_count"] = json!(e["read_count"].as_u64().unwrap_or(0) + 1);
        e["last_read"] = json!(now);
        if let Some(fb) = crate::kb::resolve_in_kb(root, rel) {
            if let Ok(c) = std::fs::read_to_string(&fb) {
                e["fp"] = json!(simple_fp(&c));
            }
        }
        // 规则层（仅 L2 内容层 notes/，且读满阈值 + 内容指纹变化）：补链建议进 pending（人工 /link 应用）
        let count = e["read_count"].as_u64().unwrap_or(0);
        let cur_fp = e["fp"].as_str().unwrap_or("").to_string();
        let organized_fp = e.get("organized_fp").and_then(Value::as_str).unwrap_or("");
        if count >= HEAT_THRESHOLD && organized_fp != cur_fp && rel.starts_with("notes/") {
            if let Some(fb) = crate::kb::resolve_in_kb(root, rel) {
                if let Ok(content) = std::fs::read_to_string(&fb) {
                    if let Ok(links) = crate::search::suggest_links(root, &content, 3) {
                        if !links.is_empty() {
                            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
                            let safe = rel.replace(['/', '\\'], "_");
                            let ppath = format!("pending/notes/自组织/补链建议-{}-{}.md", date, safe);
                            let dst = root.join(&ppath);
                            if !dst.exists() {
                                let body = format!(
                                    "---\ntype: link-suggestion\nsource: {rel}\ndate: {date}\n---\n\n# 补链建议\n\n检测到 `{rel}` 被频繁读取（{count} 次），建议补链：\n\n{}\n\n（人工用 /link <源> <目标> 应用，或 /link-all 一键应用）\n",
                                    links.iter().map(|l| format!("- [[{}]]", l.path)).collect::<Vec<_>>().join("\n")
                                );
                                if let Some(parent) = dst.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                if std::fs::write(&dst, body).is_ok() {
                                    created.push(ppath);
                                }
                            }
                        }
                    }
                }
            }
            e["organized_fp"] = json!(cur_fp);
        }
        heat["paths"][rel] = e;
    }
    (heat, created)
}

async fn memory_heat(State(st): State<AppState>) -> Json<Value> {
    Json(load_heat(&st.kb_root))
}

// ---------- 经验闭环 C1（审视层）：触发信号 → LLM 审视 → 经验提案进 pending ----------
// 信号源：前端纠错关键词（correction）/ 工具失败（tool_failure）；零 token 触发，LLM 只在信号出现时调用一次

#[derive(Deserialize)]
struct ExperienceProposeBody {
    signal: Option<String>,
    context: Option<String>,
}

async fn experience_propose(State(st): State<AppState>, Json(b): Json<ExperienceProposeBody>) -> Response {
    let signal = b.signal.unwrap_or_default();
    let context = b.context.unwrap_or_default();
    if signal.is_empty() || context.is_empty() {
        return Json(json!({ "ok": false, "error": "缺 signal/context" })).into_response();
    }
    // 防刷：同信号高频忽略（简单记录，不重复提案）
    let cfg = crate::config::load();
    if cfg.llm.endpoint.trim().is_empty() {
        // 无 LLM：规则占位提案（信号本身即记录）
        let created = write_experience_proposal(&st.kb_root, &signal, &context, None);
        return Json(json!({ "ok": true, "created": created })).into_response();
    }
    let root = st.kb_root.clone();
    let signal2 = signal.clone();
    let context2 = context.clone();
    tokio::spawn(async move {
        let system = "你是经验审视器。基于以下摩擦信号判断是否值得沉淀为经验，只输出 JSON：\
            {\"worth\":true或false,\"type\":\"memory|behavior|code\",\"problem\":\"问题描述（≤80字）\",\"improve\":\"改进建议（≤120字）\"}\n\
            只有真实摩擦（用户纠错/工具失败/反复踩坑）才 worth=true；泛泛的抱怨不算。";
        let user = format!("信号：{signal2}\n上下文：{context2}");
        let body = serde_json::json!({ "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ]});
        if let Ok(resp) = crate::llm::chat(&cfg.llm.endpoint, &cfg.llm.model, &cfg.llm.api_key, body).await {
            let full = resp["choices"][0]["message"]["content"].as_str().unwrap_or("");
            if let Some(v) = extract_json_from_text(full) {
                if v["worth"].as_bool().unwrap_or(false) {
                    let _ = write_experience_proposal(&root, &signal2, &context2, Some(&v));
                }
            }
        }
    });
    Json(json!({ "ok": true, "created": 0, "async": true })).into_response()
}

/// 写经验提案（C2 三分通道）：文件名前缀 EXPERIENCE.<TYPE> 供审批路由（MEMORY 进记忆 / BEHAVIOR 落行为建议 / CODE 落代码 backlog）
fn write_experience_proposal(root: &Path, signal: &str, context: &str, review: Option<&Value>) -> Option<String> {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let safe = format!("{}-{}", date, simple_fp(&format!("{signal}:{context}")).chars().take(8).collect::<String>());
    let (typ, problem, improve) = match review {
        Some(v) => (
            v["type"].as_str().unwrap_or("behavior").to_string(),
            v["problem"].as_str().unwrap_or("").to_string(),
            v["improve"].as_str().unwrap_or("").to_string(),
        ),
        None => ("behavior".to_string(), "疑似摩擦信号（未配置 LLM，规则占位）".to_string(), "待人工判断".to_string()),
    };
    let typ_up = match typ.as_str() {
        "memory" => "MEMORY",
        "code" => "CODE",
        _ => "BEHAVIOR",
    };
    let ppath = format!("pending/EXPERIENCE.{typ_up}.{safe}.md");
    let dst = root.join(&ppath);
    if dst.exists() {
        return None;
    }
    let body = format!(
        "---\ntype: experience\nsignal: {signal}\ndate: {date}\n---\n\n# 经验提案\n\n- 类型：{typ}\n- 信号：{signal}\n- 问题：{problem}\n- 改进：{improve}\n- 上下文：{context}\n"
    );
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::write(&dst, body).is_ok() {
        Some(ppath)
    } else {
        None
    }
}

// ---------- C0 dev 工具链（自我开发执行层：让 agent 能读自己代码） ----------
// 项目根 = 服务进程 CWD（cargo run 从项目根启动）；dev.read 白名单目录 + 路径规范化防逃逸（复用 resolve_in_kb 锚定项目根）
fn dev_project_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    if cwd.join("Cargo.toml").is_file() {
        Some(cwd)
    } else {
        // exe 旁（release dist/ 场景，向上找 Cargo.toml 到项目根）
        let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        let mut cur = Some(exe_dir.as_path());
        while let Some(d) = cur {
            if d.join("Cargo.toml").is_file() {
                return Some(d.to_path_buf());
            }
            cur = d.parent();
        }
        None
    }
}

fn is_dev_allowed(rel: &str) -> bool {
    rel == "Cargo.toml" || rel == "README.md" || rel == ".gitignore"
        || rel.starts_with("src/") || rel.starts_with("web/") || rel.starts_with("scripts/")
        || rel.starts_with(".zcode/plans/")
}

async fn dev_read(Query(p): Query<FileParams>) -> Response {
    let Some(root) = dev_project_root() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "无法定位项目根" }))).into_response();
    };
    let rel = p.path.trim().trim_start_matches(['/', '\\']);
    if !is_dev_allowed(rel) {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "路径不在 dev 白名单内（src/web/scripts/Cargo.toml/README.md/.zcode/plans）" }))).into_response();
    }
    match crate::kb::resolve_in_kb(&root, rel) {
        Some(pb) if pb.is_file() => match tokio::fs::read_to_string(&pb).await {
            Ok(content) => Json(json!({ "path": rel, "content": content })).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
        },
        _ => (StatusCode::NOT_FOUND, Json(json!({ "error": "文件不存在或超出项目根" }))).into_response(),
    }
}

#[derive(Deserialize)]
struct DevDiffParams {
    #[serde(default)]
    path: String,
}

fn git_capture(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("git 执行失败: {e}"))?;
    let mut s = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.stderr.is_empty() {
        s.push_str(&String::from_utf8_lossy(&out.stderr));
    }
    Ok(s)
}

async fn dev_status() -> Response {
    match git_capture(&["status", "--short"]) {
        Ok(out) => Json(json!({ "ok": true, "output": out })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

async fn dev_diff(Query(q): Query<DevDiffParams>) -> Response {
    let args: Vec<String> = if q.path.trim().is_empty() {
        vec!["diff".to_string()]
    } else {
        vec!["diff".to_string(), "--".to_string(), q.path.trim().to_string()]
    };
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    match git_capture(&args_ref) {
        Ok(out) => Json(json!({ "ok": true, "output": out })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

// ---------- C3 代码提案通道：dev.patch 生成提案（进 pending/code/）→ 人审 → dev.apply 应用+构建验证+回滚 ----------
// 务实调整：plan 的"先验证再送审"（临时副本 build）对单用户开销大；改为 apply 时验证 + 备份回滚（能力等价，成本低）

#[derive(Deserialize)]
struct DevPatchFile {
    path: String,
    content: String,
}

#[derive(Deserialize)]
struct DevPatchBody {
    reason: Option<String>,
    files: Option<Vec<DevPatchFile>>,
}

async fn dev_patch(State(st): State<AppState>, Json(b): Json<DevPatchBody>) -> Response {
    let files = b.files.unwrap_or_default();
    if files.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "缺 files" }))).into_response();
    }
    for f in &files {
        let rel = f.path.trim().trim_start_matches(['/', '\\']);
        if !is_dev_allowed(rel) {
            return (StatusCode::FORBIDDEN, Json(json!({ "error": format!("路径不在 dev 白名单内: {}", f.path) }))).into_response();
        }
    }
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let ppath = format!("pending/code/{ts}.md");
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut content = format!(
        "---\ntype: code-patch\nreason: {}\nfiles: {}\ndate: {date}\n---\n\n# 代码提案（{date} {ts}）\n\n修改文件：{}\n\n> 应用：/dev apply {ppath}（先备份，构建失败自动回滚）\n",
        b.reason.unwrap_or_default(),
        files.iter().map(|f| f.path.clone()).collect::<Vec<_>>().join(" | "),
        files.iter().map(|f| format!("- `{}`", f.path)).collect::<Vec<_>>().join("\n")
    );
    for f in &files {
        content.push_str(&format!("\n### FILE: {}\n{}\n", f.path, f.content));
    }
    let dst = st.kb_root.join(&ppath);
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&dst, content) {
        Ok(()) => Json(json!({ "ok": true, "path": ppath, "files": files.len(), "hint": "/dev apply 应用+构建验证" })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 解析代码提案：frontmatter files + 正文 `### FILE: <path>` 块 → Vec<(path, content)>
fn parse_code_patch(content: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let body_start = content.find("---\n\n").map(|i| i + 4).unwrap_or(0);
    let body = &content[body_start..];
    let mut cur: Option<(String, String)> = None;
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("### FILE: ") {
            if let Some((p, c)) = cur.take() {
                out.push((p, c.trim_end().to_string()));
            }
            cur = Some((rest.trim().to_string(), String::new()));
        } else if let Some((_, acc)) = cur.as_mut() {
            acc.push_str(line);
            acc.push('\n');
        }
    }
    if let Some((p, c)) = cur {
        out.push((p, c.trim_end().to_string()));
    }
    out
}

#[derive(Deserialize)]
struct DevApplyBody {
    path: String,
}

async fn dev_apply(State(st): State<AppState>, Json(b): Json<DevApplyBody>) -> Response {
    let Some(proj) = dev_project_root() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "无法定位项目根" }))).into_response();
    };
    let rel = b.path.trim().trim_start_matches(['/', '\\']);
    let pb = match crate::kb::resolve_in_kb(&st.kb_root, rel) {
        Some(p) if p.is_file() => p,
        _ => return (StatusCode::NOT_FOUND, Json(json!({ "error": "提案不存在" }))).into_response(),
    };
    let content = match std::fs::read_to_string(&pb) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    let files = parse_code_patch(&content);
    if files.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "提案无可应用文件" }))).into_response();
    }
    // 白名单校验（防提案逃逸）
    for (path, _) in &files {
        let rel2 = path.trim().trim_start_matches(['/', '\\']);
        if !is_dev_allowed(rel2) {
            return (StatusCode::FORBIDDEN, Json(json!({ "error": format!("路径不在 dev 白名单内: {path}") }))).into_response();
        }
    }
    // 备份（写 .dev-bak-<ts> 到同目录）
    let ts = chrono::Local::now().format("%Y%m%d%H%M%S").to_string();
    let mut backups: Vec<(String, PathBuf)> = Vec::new();
    for (path, _) in &files {
        let rel2 = path.trim().trim_start_matches(['/', '\\']);
        let target = proj.join(rel2);
        let bak = target.with_file_name(format!(
            "{}.dev-bak-{ts}",
            target.file_name().and_then(|x| x.to_str()).unwrap_or("file")
        ));
        if target.exists() {
            let _ = std::fs::copy(&target, &bak);
        }
        backups.push((rel2.to_string(), bak));
    }
    // 写新内容
    let mut applied: Vec<String> = Vec::new();
    for (path, new_content) in &files {
        let rel2 = path.trim().trim_start_matches(['/', '\\']);
        let target = proj.join(rel2);
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&target, new_content) {
            restore_backups(&proj, &backups);
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("写入 {rel2} 失败: {e}（已回滚）") }))).into_response();
        }
        applied.push(rel2.to_string());
    }
    // 编译验证（cargo check：只编译不链接，避开运行实例 exe 锁；冷缓存首次可能 1-3 分钟，超时 300s）
    let build = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        tokio::process::Command::new("cargo").arg("check").current_dir(&proj).output(),
    )
    .await;
    match build {
        Ok(Ok(out)) if out.status.success() => {
            // 成功：清理备份
            for (_, bak) in &backups {
                let _ = std::fs::remove_file(bak);
            }
            Json(json!({ "ok": true, "applied": applied, "build": "ok" })).into_response()
        }
        Ok(Ok(out)) => {
            let log = String::from_utf8_lossy(&out.stderr).chars().take(1500).collect::<String>();
            restore_backups(&proj, &backups);
            Json(json!({ "ok": false, "applied": [], "build": "failed", "rolled_back": true, "error": log })).into_response()
        }
        _ => {
            restore_backups(&proj, &backups);
            Json(json!({ "ok": false, "applied": [], "build": "timeout/error", "rolled_back": true })).into_response()
        }
    }
}

fn restore_backups(proj: &Path, backups: &[(String, PathBuf)]) {
    for (rel2, bak) in backups {
        let target = proj.join(rel2);
        if bak.is_file() {
            let _ = std::fs::copy(bak, &target);
        }
        let _ = std::fs::remove_file(bak);
    }
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

    #[test]
    fn memory_touch_heat_and_rule_threshold() {
        let root = temp_root("heatt");
        crate::kb::ensure_layout(&root).unwrap();
        // 两个文档：A 提及 B 的标题但未链接 → suggest_links 应给出建议
        let notes = root.join("notes").join("架构");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(notes.join("托盘应用.md"), "# 托盘应用\n\n托盘是 Rust 常驻 + Axum。").unwrap();
        std::fs::write(notes.join("记忆统一模型.md"), "# 记忆统一模型\n\n双链是记忆的签名。").unwrap();
        std::fs::write(notes.join("无向量库检索.md"), "# 无向量库检索\n\n托盘应用 与 记忆统一模型 相关。").unwrap();
        let path = "notes/架构/无向量库检索.md".to_string();
        // 3 次 touch → 热度 3 + 触发规则层（补链建议进 pending）；每次保存（模拟 handler 行为）
        for i in 0..3 {
            let (heat, created) = apply_memory_touch(&root, &[path.clone()]);
            save_heat(&root, &heat).unwrap();
            assert_eq!(heat["paths"][&path]["read_count"].as_u64().unwrap(), i as u64 + 1);
            if i == 2 {
                // 第 3 次触发阈值：suggest_links 有命中 → 提案；无命中（无提及未链接）→ organized_fp 已设
                if created.is_empty() {
                    assert!(heat["paths"][&path].get("organized_fp").is_some());
                } else {
                    assert!(!created.is_empty());
                }
            }
        }
        // 第 4 次 touch（内容未变）→ 不重复提案
        let (_, created4) = apply_memory_touch(&root, &[path.clone()]);
        assert!(created4.is_empty());
        // 非 notes/ 路径（L1）只记热度不触发规则层
        let (heat2, created2) = apply_memory_touch(&root, &["MEMORY.md".to_string()]);
        assert_eq!(heat2["paths"]["MEMORY.md"]["read_count"].as_u64().unwrap(), 1);
        assert!(created2.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn llm_analysis_trigger_and_json_extract() {
        // should_run_llm_analysis：阈值 + notes/ + organized_llm_fp 防重复
        let heat = serde_json::json!({
            "paths": {
                "notes/a.md": { "read_count": 5, "fp": "f1", "organized_llm_fp": "f1" },
                "notes/b.md": { "read_count": 5, "fp": "f2" },
                "notes/c.md": { "read_count": 3, "fp": "f3" },
                "MEMORY.md": { "read_count": 9, "fp": "f4" },
            }
        });
        assert!(!should_run_llm_analysis(&heat, "notes/a.md", 5, "f1")); // 已分析（同 fp）
        assert!(should_run_llm_analysis(&heat, "notes/b.md", 5, "f2")); // 未分析
        assert!(!should_run_llm_analysis(&heat, "notes/b.md", 4, "f2")); // 未达阈值
        assert!(!should_run_llm_analysis(&heat, "notes/c.md", 3, "f3")); // 阈值 5
        assert!(!should_run_llm_analysis(&heat, "MEMORY.md", 9, "f4")); // 非 notes/
        // extract_json_from_text：纯 JSON / ```json 包裹 / 前后文字
        let j1 = extract_json_from_text(r#"{"conflicts":[],"gaps":[],"merges":[]}"#).unwrap();
        assert_eq!(j1["conflicts"].as_array().unwrap().len(), 0);
        let j2 = extract_json_from_text("```json\n{\"gaps\":[{\"topic\":\"x\"}]}\n```").unwrap();
        assert_eq!(j2["gaps"][0]["topic"], "x");
        let j3 = extract_json_from_text("分析结果：{\"merges\":[{\"docs\":[\"a.md\"]}]} 完毕").unwrap();
        assert_eq!(j3["merges"][0]["docs"][0], "a.md");
        assert!(extract_json_from_text("无发现").is_none());
    }

    #[test]
    fn code_patch_parse() {
        let content = "---\ntype: code-patch\nreason: 修复\nfiles: src/a.rs|web/b.js\ndate: 2026-08-05\n---\n\n# 代码提案\n\n### FILE: src/a.rs\nfn main() {}\n\n### FILE: web/b.js\nconsole.log('x');\n";
        let files = parse_code_patch(content);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "src/a.rs");
        assert!(files[0].1.contains("fn main() {}"));
        assert_eq!(files[1].0, "web/b.js");
        assert!(files[1].1.contains("console.log"));
        // 空提案
        assert!(parse_code_patch("---\ntype: code-patch\n---\n\n无文件").is_empty());
    }
}

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("md-agent-srv-{}-{}", name, std::process::id()))
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
