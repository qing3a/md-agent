//! Phase 3-B 任务引擎：kb/.tasks.db（独立于 graph.db，避免全量重建互相干扰）。
//! 轻量看板式任务：goal + 状态机（todo/doing/done/dropped）+ 依赖 + 推进日志。
//! 全部操作即时落盘（rusqlite 事务提交），无缓存层。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Task {
    pub id: i64,
    pub goal: String,
    pub title: String,
    pub status: String,
    pub deps: Vec<String>,
    pub log: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub const STATUSES: [&str; 4] = ["todo", "doing", "done", "dropped"];

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

fn open(root: &Path) -> Result<Connection, String> {
    let db = root.join(".tasks.db");
    let conn = Connection::open(&db).map_err(|e| format!("打开任务库失败: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tasks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            goal TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'todo',
            deps TEXT NOT NULL DEFAULT '[]',
            log TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );",
    )
    .map_err(|e| format!("初始化任务库失败: {e}"))?;
    Ok(conn)
}

fn row_to_task(r: &rusqlite::Row) -> rusqlite::Result<Task> {
    let deps_raw: String = r.get(4)?;
    let log_raw: String = r.get(5)?;
    let deps: Vec<String> = serde_json::from_str(&deps_raw).unwrap_or_default();
    let log: Vec<String> = serde_json::from_str(&log_raw).unwrap_or_default();
    Ok(Task {
        id: r.get(0)?,
        goal: r.get(1)?,
        title: r.get(2)?,
        status: r.get(3)?,
        deps,
        log,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

pub fn list(root: &Path) -> Result<Vec<Task>, String> {
    let conn = open(root)?;
    let mut stmt = conn
        .prepare("SELECT * FROM tasks ORDER BY status = 'done', status = 'dropped', created_at")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], row_to_task)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn create(root: &Path, goal: &str, title: &str) -> Result<Task, String> {
    let goal = goal.trim();
    if goal.is_empty() {
        return Err("goal 不能为空".to_string());
    }
    let title = title.trim();
    let t = now();
    let conn = open(root)?;
    conn.execute(
        "INSERT INTO tasks (goal, title, status, deps, log, created_at, updated_at)
         VALUES (?1, ?2, 'todo', '[]', '[]', ?3, ?3)",
        params![goal, title, t],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    get(&conn, id).ok_or_else(|| "读取新任务失败".to_string())
}

fn get(conn: &Connection, id: i64) -> Option<Task> {
    conn.query_row("SELECT * FROM tasks WHERE id = ?1", params![id], row_to_task).ok()
}

/// 变更状态/追加日志/设置依赖；note 追加进推进日志（带时间戳）
pub fn update(
    root: &Path,
    id: i64,
    status: Option<&str>,
    note: Option<&str>,
    deps: Option<&[String]>,
) -> Result<Task, String> {
    let conn = open(root)?;
    let mut task = get(&conn, id).ok_or_else(|| format!("任务 #{id} 不存在"))?;
    let t = now();
    if let Some(s) = status {
        if !STATUSES.contains(&s) {
            return Err(format!("非法状态 {s}，可选: {}", STATUSES.join("/")));
        }
        task.status = s.to_string();
    }
    if let Some(n) = note {
        let n = n.trim();
        if !n.is_empty() {
            task.log.push(format!("[{t}] {n}"));
        }
    }
    if let Some(d) = deps {
        task.deps = d.to_vec();
    }
    task.updated_at = t.clone();
    conn.execute(
        "UPDATE tasks SET status = ?2, deps = ?3, log = ?4, updated_at = ?5 WHERE id = ?1",
        params![
            id,
            task.status,
            serde_json::to_string(&task.deps).unwrap_or_else(|_| "[]".into()),
            serde_json::to_string(&task.log).unwrap_or_else(|_| "[]".into()),
            t,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(task)
}

pub fn remove(root: &Path, id: i64) -> Result<bool, String> {
    let conn = open(root)?;
    let n = conn.execute("DELETE FROM tasks WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// 看板统计（供 /board 顶部与 /task 摘要）
pub fn stats(root: &Path) -> Result<serde_json::Value, String> {
    let conn = open(root)?;
    let mut out = serde_json::Map::new();
    for s in STATUSES {
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks WHERE status = ?1", params![s], |r| r.get(0))
            .unwrap_or(0);
        out.insert(s.to_string(), serde_json::json!(n));
    }
    Ok(serde_json::Value::Object(out))
}
