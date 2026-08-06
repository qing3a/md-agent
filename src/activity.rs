//! 活动记录：kb/.activity.db（独立于 graph/tasks 库）。
//! 「你的 AI 做了什么，永远可查」——工具/待审/任务/会话/图谱/检索的落盘流水，
//! 为运营面板时间线、洞察趋势、评测/自证供数。
//! 全部即时落盘（rusqlite 事务提交），无缓存层；record 失败静默（旁路，不阻塞主流程）。

use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::path::Path;

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

fn open(root: &Path) -> Result<Connection, String> {
    let db = root.join(".activity.db");
    let conn = Connection::open(&db).map_err(|e| format!("打开活动库失败: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS activity (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts TEXT NOT NULL,              -- ISO8601 本地时间
            kind TEXT NOT NULL,            -- tool|task|pending|doc|session|search|sys
            text TEXT NOT NULL,
            meta TEXT NOT NULL DEFAULT '{}'
        );",
    )
    .map_err(|e| format!("初始化活动库失败: {e}"))?;
    Ok(conn)
}

/// 落盘一条活动（fire-and-forget：失败静默，不阻塞调用方）
pub fn record(root: &Path, kind: &str, text: &str, meta: Value) {
    let conn = match open(root) {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = conn.execute(
        "INSERT INTO activity (ts, kind, text, meta) VALUES (?1, ?2, ?3, ?4)",
        params![now(), kind, text, meta.to_string()],
    );
}

fn meta_str_to_value(it: &mut Value) {
    if let Some(m) = it["meta"].as_str() {
        if let Ok(v) = serde_json::from_str::<Value>(m) {
            it["meta"] = v;
        }
    }
}

/// 最近 N 条（倒序，时间线新→旧展示）
pub fn list(root: &Path, limit: i64) -> Result<Value, String> {
    let conn = open(root)?;
    let mut stmt = conn
        .prepare("SELECT id, ts, kind, text, meta FROM activity ORDER BY id DESC LIMIT ?1")
        .map_err(|e| format!("查询活动失败: {e}"))?;
    let rows = stmt
        .query_map(params![limit], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "ts": r.get::<_, String>(1)?,
                "kind": r.get::<_, String>(2)?,
                "text": r.get::<_, String>(3)?,
                "meta": r.get::<_, String>(4)?,
            }))
        })
        .map_err(|e| format!("查询活动失败: {e}"))?;
    let mut items: Vec<Value> = Vec::new();
    for row in rows {
        match row {
            Ok(v) => items.push(v),
            Err(e) => return Err(format!("读取活动失败: {e}")),
        }
    }
    for it in items.iter_mut() {
        meta_str_to_value(it);
    }
    Ok(json!({ "items": items }))
}

/// 增量拉取：id 之后的新记录（运营面板轮询用，前端每 3s 传最后已见 id）
pub fn since(root: &Path, id: i64, limit: i64) -> Result<Value, String> {
    let conn = open(root)?;
    let mut stmt = conn
        .prepare("SELECT id, ts, kind, text, meta FROM activity WHERE id > ?1 ORDER BY id ASC LIMIT ?2")
        .map_err(|e| format!("查询活动失败: {e}"))?;
    let rows = stmt
        .query_map(params![id, limit], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "ts": r.get::<_, String>(1)?,
                "kind": r.get::<_, String>(2)?,
                "text": r.get::<_, String>(3)?,
                "meta": r.get::<_, String>(4)?,
            }))
        })
        .map_err(|e| format!("查询活动失败: {e}"))?;
    let mut items: Vec<Value> = Vec::new();
    for row in rows {
        match row {
            Ok(v) => items.push(v),
            Err(e) => return Err(format!("读取活动失败: {e}")),
        }
    }
    for it in items.iter_mut() {
        meta_str_to_value(it);
    }
    Ok(json!({ "items": items }))
}
