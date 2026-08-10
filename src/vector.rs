//! 向量索引（语义召回 Phase 4 M1）：SQLite 存 chunk 向量 + cosine 全表扫描。
//! 与图谱同策略：全量重建（小规模 KB 足够快）；向量索引是派生产物（可从 MD 全量重建，不进待审）。
//! 不引入 ANN——本地 KB 万级 chunk 内全表扫描足够；Phase 4 可选加 ANN 索引。
//!
//! 结构：网络（embed.rs，异步）与本模块（纯同步 DB/数学）分离——
//! 调用方（server 路由）先 collect_chunks → 异步 embed → store_embeddings；检索方只吃向量。

use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const DB_FILE: &str = ".vector.db";
/// 单 chunk 嵌入文本上限（字符，控制 token 成本；按 char 截断不劈 CJK）
const CHUNK_MAX_CHARS: usize = 1000;

fn db_path(root: &Path) -> PathBuf {
    root.join(DB_FILE)
}

fn connect(root: &Path) -> Result<Connection, String> {
    let conn = Connection::open(db_path(root)).map_err(|e| format!("打开向量库失败: {e}"))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS doc_embeddings (
            chunk_id TEXT PRIMARY KEY,   -- {file}#{line}（幂等 upsert）
            file     TEXT NOT NULL,
            line     INTEGER NOT NULL,
            model    TEXT NOT NULL,
            dim      INTEGER NOT NULL,
            vec      BLOB NOT NULL       -- f32 小端连续数组
         );
         CREATE INDEX IF NOT EXISTS idx_emb_file ON doc_embeddings(file);",
    )
    .map_err(|e| format!("初始化向量库失败: {e}"))?;
    Ok(conn)
}

/// 一个待嵌入的文本块（`##` 小节切分，与 read_l1/preview 一致，不发明新元数据）
#[derive(Debug, Clone)]
pub struct Chunk {
    /// `{file}#{line}` 稳定主键
    pub chunk_id: String,
    /// 相对 KB 根的路径（`/` 分隔）
    pub file: String,
    /// 小节起始行号（1 基）
    pub line: u64,
    /// 块文本（含 `##` 标题行；文件头 `#` 标题并入首个 `##` 小节）
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct SyncReport {
    pub chunks: usize,
    pub files: usize,
    pub model: String,
    pub dim: usize,
}

/// 语义检索命中（每 chunk 一条；跨文件聚合/融合由 search.rs 负责）
#[derive(Debug, Clone, Serialize)]
pub struct SemanticHit {
    pub file: String,
    pub line: u64,
    pub text: String,
    /// cosine 相似度（1=最相似）
    pub score: f64,
}

// ---------- 分块 ----------

/// 全库扫描 .md → `##` 小节分块（过滤规则与 search.rs 一致：跳过 pending/sessions/apps/INDEX）
pub fn collect_chunks(root: &Path) -> Vec<Chunk> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut out: Vec<Chunk> = Vec::new();
    if !root.is_dir() {
        return out;
    }
    let mut b = ignore::WalkBuilder::new(&root);
    b.hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false);
    // 项目制隔离区（projects/）与应用空间（apps/）不参与全局向量索引
    b.filter_entry(|e| e.file_name() != "projects" && e.file_name() != "apps");
    for entry in b.build() {
        let Ok(entry) = entry else { continue };
        let Some(ft) = entry.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        let path = entry.path();
        if path.components().any(|c| {
            let n = c.as_os_str();
            n == "pending" || n == "sessions" || n == "apps"
        }) {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("INDEX.md") {
            continue;
        }
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(content) = std::fs::read_to_string(path) else { continue };
        out.extend(chunks_of(&rel, &content));
    }
    out
}

/// 单个文件 → 小节块：以 `## ` 行为块起点，到下一个 `## ` 前；文件头 `#` 标题并入首个块。
/// 纯行级切分，无解析器依赖；与 read_l1 的 `##` 小节单位一致。
fn chunks_of(file: &str, content: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<Chunk> = Vec::new();
    // 找所有 `## ` 小节起点（1 基行号）
    let starts: Vec<(usize, u64)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.trim_start().starts_with("## "))
        .map(|(i, _)| (i, (i + 1) as u64))
        .collect();
    if starts.is_empty() {
        // 无 `##` 小节：整篇一个块（含文件头标题），起点 1
        let text: String = lines.join("\n");
        if !text.trim().is_empty() {
            out.push(Chunk {
                chunk_id: format!("{file}#1"),
                file: file.to_string(),
                line: 1,
                text: truncate_chars(&text, CHUNK_MAX_CHARS),
            });
        }
        return out;
    }
    for (idx, (start_i, start_line)) in starts.iter().enumerate() {
        let end_i = starts
            .get(idx + 1)
            .map(|(i, _)| *i)
            .unwrap_or(lines.len());
        let mut text: String = String::new();
        // 首个小节并入文件头（`# 标题` 与 frontmatter 行）
        if idx == 0 {
            let head_end = starts[0].0.min(lines.len());
            for l in &lines[..head_end] {
                if l.starts_with("---") {
                    continue; // 跳过 frontmatter 定界符
                }
                if l.trim().is_empty() {
                    continue;
                }
                text.push_str(l.trim_end());
                text.push('\n');
            }
        }
        for l in &lines[*start_i..end_i] {
            text.push_str(l.trim_end());
            text.push('\n');
        }
        let text = text.trim().to_string();
        if !text.is_empty() {
            out.push(Chunk {
                chunk_id: format!("{file}#{start_line}"),
                file: file.to_string(),
                line: *start_line,
                text: truncate_chars(&text, CHUNK_MAX_CHARS),
            });
        }
    }
    out
}

fn truncate_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
}

// ---------- 数学 ----------

/// cosine 相似度；零向量返回 0（避免 NaN）
pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    for i in 0..a.len() {
        let x = a[i] as f64;
        let y = b[i] as f64;
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let d = na.sqrt() * nb.sqrt();
    if d <= f64::EPSILON {
        0.0
    } else {
        dot / d
    }
}

// ---------- 写入 / 检索 ----------

fn to_blob(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for f in v {
        b.extend_from_slice(&f.to_le_bytes());
    }
    b
}

fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// 全量重建向量索引（与图谱 sync 同策略）：清空旧行 → 批量 upsert。
/// chunks 与 vecs 一一对应；维度以首块为准，块间维度不一致时报错（防脏数据）。
pub fn store_embeddings(
    root: &Path,
    model: &str,
    chunks: &[Chunk],
    vecs: &[Vec<f32>],
) -> Result<SyncReport, String> {
    if chunks.len() != vecs.len() {
        return Err(format!(
            "chunks({}) 与 vecs({}) 数量不一致",
            chunks.len(),
            vecs.len()
        ));
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut conn = connect(&root)?;
    conn.execute("DELETE FROM doc_embeddings", [])
        .map_err(|e| format!("清空向量表失败: {e}"))?;
    let mut dim = 0usize;
    let mut files: HashSet<String> = HashSet::new();
    {
        let tx = conn
            .transaction()
            .map_err(|e| format!("开启事务失败: {e}"))?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO doc_embeddings (chunk_id, file, line, model, dim, vec)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(|e| format!("prepare 失败: {e}"))?;
            for (c, v) in chunks.iter().zip(vecs.iter()) {
                if v.is_empty() {
                    continue;
                }
                if dim == 0 {
                    dim = v.len();
                } else if v.len() != dim {
                    return Err(format!(
                        "维度不一致: {}({}) != {}({})",
                        c.chunk_id,
                        v.len(),
                        dim,
                        dim
                    ));
                }
                stmt.execute(params![c.chunk_id, c.file, c.line as i64, model, v.len() as i64, to_blob(v)])
                    .map_err(|e| format!("写入向量失败 {}: {e}", c.chunk_id))?;
                files.insert(c.file.clone());
            }
        }
        tx.commit().map_err(|e| format!("提交失败: {e}"))?;
    }
    Ok(SyncReport {
        chunks: chunks.len(),
        files: files.len(),
        model: model.to_string(),
        dim,
    })
}

/// 语义检索：cosine 全表扫描 → 按相似度降序取 top k。
/// q_vec 由调用方（server 路由）先 embed 好再传入——本模块零网络。
pub fn semantic_search(
    root: &Path,
    q_vec: &[f32],
    k: usize,
    model: Option<&str>,
) -> Result<Vec<SemanticHit>, String> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let conn = connect(&root)?;
    let mut hits: Vec<SemanticHit> = Vec::new();
    match model {
        Some(m) => {
            let mut stmt = conn
                .prepare(
                    "SELECT chunk_id, file, line, dim, vec FROM doc_embeddings WHERE model = ?1",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![m], row_to_raw)
                .map_err(|e| e.to_string())?;
            for row in rows {
                let Ok((_chunk_id, file, line, _dim, blob)) = row else { continue };
                collect_hit(&mut hits, q_vec, &file, line, &blob);
            }
        }
        None => {
            let mut stmt = conn
                .prepare("SELECT chunk_id, file, line, dim, vec FROM doc_embeddings")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], row_to_raw)
                .map_err(|e| e.to_string())?;
            for row in rows {
                let Ok((_chunk_id, file, line, _dim, blob)) = row else { continue };
                collect_hit(&mut hits, q_vec, &file, line, &blob);
            }
        }
    }
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    hits.truncate(k);
    Ok(hits)
}

/// 行映射命名函数（两个分支共用同一闭包类型，避免 if/else 类型不兼容）
fn row_to_raw(
    r: &rusqlite::Row,
) -> rusqlite::Result<(String, String, i64, i64, Vec<u8>)> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
    ))
}

fn collect_hit(hits: &mut Vec<SemanticHit>, q_vec: &[f32], file: &str, line: i64, blob: &[u8]) {
    let v = from_blob(blob);
    let s = cosine(q_vec, &v);
    if s <= 0.0 {
        return;
    }
    hits.push(SemanticHit {
        file: file.to_string(),
        line: line as u64,
        text: String::new(), // 文本不回填：命中片段由 search.rs 从 MD 原文取（与 grep 命中同一来源）
        score: s,
    });
}

/// 向量索引统计（/api/embed/stats）
#[derive(Debug, Serialize)]
pub struct Stats {
    pub chunks: usize,
    pub files: usize,
    pub model: Option<String>,
    pub dim: Option<usize>,
    pub db_exists: bool,
}

pub fn stats(root: &Path) -> Result<Stats, String> {
    let db = db_path(root);
    if !db.exists() {
        return Ok(Stats {
            chunks: 0,
            files: 0,
            model: None,
            dim: None,
            db_exists: false,
        });
    }
    let conn = connect(root)?;
    let chunks: i64 = conn
        .query_row("SELECT COUNT(*) FROM doc_embeddings", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let files: i64 = conn
        .query_row("SELECT COUNT(DISTINCT file) FROM doc_embeddings", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let meta: Option<(String, i64)> = conn
        .query_row(
            "SELECT model, MAX(dim) FROM doc_embeddings LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    Ok(Stats {
        chunks: chunks as usize,
        files: files as usize,
        model: meta.as_ref().map(|(m, _)| m.clone()),
        dim: meta.map(|(_, d)| d as usize),
        db_exists: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn tmp_kb(name: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("md-agent-vec-test-{name}-{n}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("notes")).unwrap();
        d
    }

    fn write(d: &Path, rel: &str, content: &str) {
        let p = d.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }

    #[test]
    fn cosine_basic_and_zero() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!((cosine(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-6);
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0, "维度不一致返回 0");
    }

    #[test]
    fn chunks_split_on_sections() {
        let d = tmp_kb("chunk");
        write(
            &d,
            "notes/a.md",
            "---\ntype: note\n---\n# 标题\n\n## 小节一\n内容 A\n\n## 小节二\n内容 B\n",
        );
        let chunks = chunks_of("notes/a.md", &std::fs::read_to_string(d.join("notes/a.md")).unwrap());
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].line, 6); // `## 小节一` 行号（第 1 行 frontmatter 起始）
        assert!(chunks[0].text.contains("小节一"));
        assert!(chunks[0].text.contains("标题"), "文件头标题并入首个块");
        assert!(!chunks[0].text.contains("frontmatter") && !chunks[0].text.contains("---"), "跳过 frontmatter");
        assert!(chunks[0].text.contains("内容 A"));
        assert!(!chunks[0].text.contains("内容 B"));
        assert_eq!(chunks[1].line, 9);
        assert!(chunks[1].text.contains("内容 B"));
    }

    #[test]
    fn chunks_single_block_when_no_sections() {
        let d = tmp_kb("nosc");
        write(&d, "notes/x.md", "# 无小节\n一段话\n");
        let chunks = chunks_of("notes/x.md", &std::fs::read_to_string(d.join("notes/x.md")).unwrap());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].line, 1);
    }

    #[test]
    fn chunks_truncate_by_char_no_cjk_split() {
        let long = format!("## 甲\n{}", "甲".repeat(3000));
        let d = tmp_kb("trunc");
        write(&d, "notes/t.md", &long);
        let chunks = chunks_of("notes/t.md", &std::fs::read_to_string(d.join("notes/t.md")).unwrap());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text.chars().count(), CHUNK_MAX_CHARS, "按字符截断");
        assert!(chunks[0].text.chars().all(|c| c == '甲' || c == '#' || c == ' ' || c == '\n'), "不劈 CJK（char 级安全）");
    }

    #[test]
    fn collect_skips_pending_sessions_apps_index() {
        let d = tmp_kb("skip");
        write(&d, "notes/good.md", "## 好文\n正文\n");
        write(&d, "notes/sub/ok.md", "## 子文\n正文\n");
        write(&d, "pending/待审.md", "## 待审\n正文\n");
        write(&d, "sessions/流水.md", "## 流水\n正文\n");
        write(&d, "apps/x/app.md", "## 应用\n正文\n");
        write(&d, "INDEX.md", "## 索引\n正文\n");
        write(&d, "KB.md", "## L1\n正文\n");
        let chunks = collect_chunks(&d);
        let files: Vec<&str> = chunks.iter().map(|c| c.file.as_str()).collect();
        assert!(files.contains(&"notes/good.md"), "L2 正常索引");
        assert!(files.contains(&"notes/sub/ok.md"));
        assert!(files.contains(&"KB.md"), "L1 也索引（recall 需要）");
        assert!(!files.contains(&"pending/待审.md"));
        assert!(!files.contains(&"sessions/流水.md"));
        assert!(!files.contains(&"apps/x/app.md"));
        assert!(!files.contains(&"INDEX.md"));
    }

    #[test]
    fn store_then_search_roundtrip() {
        let d = tmp_kb("rt");
        let chunks = vec![
            Chunk { chunk_id: "notes/a.md#1".into(), file: "notes/a.md".into(), line: 1, text: "狗".into() },
            Chunk { chunk_id: "notes/b.md#1".into(), file: "notes/b.md".into(), line: 1, text: "猫".into() },
        ];
        // 手工向量：a 比 b 更接近查询向量 [1, 0.5]（两者相似度均为正，验证排序与过滤）
        let vecs = vec![vec![1.0f32, 0.0], vec![0.0f32, 1.0]];
        let rep = store_embeddings(&d, "test-m", &chunks, &vecs).unwrap();
        assert_eq!(rep.chunks, 2);
        assert_eq!(rep.files, 2);
        assert_eq!(rep.dim, 2);
        let hits = semantic_search(&d, &[1.0f32, 0.5], 2, Some("test-m")).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].file, "notes/a.md");
        assert!(hits[0].score > hits[1].score, "a 相似度应高于 b");
        assert_eq!(hits[1].file, "notes/b.md");
        // 与 a 正交、与 b 负相关的查询 → 空结果（cosine<=0 过滤）
        let hits = semantic_search(&d, &[0.0f32, -1.0], 2, Some("test-m")).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn store_rebuild_replaces_old_rows() {
        let d = tmp_kb("rebuild");
        let c = Chunk { chunk_id: "notes/a.md#1".into(), file: "notes/a.md".into(), line: 1, text: "x".into() };
        store_embeddings(&d, "m1", &[c.clone()], &[vec![1.0f32]]).unwrap();
        store_embeddings(&d, "m2", &[c], &[vec![1.0f32, 0.0]]).unwrap();
        let s = stats(&d).unwrap();
        assert_eq!(s.chunks, 1, "重建清空旧行");
        assert_eq!(s.model.as_deref(), Some("m2"));
        assert_eq!(s.dim, Some(2));
    }

    #[test]
    fn search_respects_model_filter() {
        let d = tmp_kb("mf");
        let c = Chunk { chunk_id: "notes/a.md#1".into(), file: "notes/a.md".into(), line: 1, text: "x".into() };
        store_embeddings(&d, "m1", &[c], &[vec![1.0f32]]).unwrap();
        assert_eq!(semantic_search(&d, &[1.0f32], 5, Some("m1")).unwrap().len(), 1);
        assert_eq!(semantic_search(&d, &[1.0f32], 5, Some("nope")).unwrap().len(), 0);
    }

    #[test]
    fn store_rejects_mismatched_counts_and_dims() {
        let d = tmp_kb("mismatch");
        let c = Chunk { chunk_id: "a#1".into(), file: "a".into(), line: 1, text: "x".into() };
        assert!(store_embeddings(&d, "m", &[c.clone()], &[]).is_err());
        assert!(store_embeddings(&d, "m", &[c.clone(), c.clone()], &[vec![1.0f32]]).is_err());
        let c2 = Chunk { chunk_id: "b#1".into(), file: "b".into(), line: 1, text: "y".into() };
        assert!(store_embeddings(&d, "m", &[c, c2], &[vec![1.0f32], vec![1.0f32, 0.0]]).is_err(), "维度不一致报错");
    }

    #[test]
    fn stats_absent_db() {
        let d = tmp_kb("nostats");
        let s = stats(&d).unwrap();
        assert!(!s.db_exists);
        assert_eq!(s.chunks, 0);
    }
}
