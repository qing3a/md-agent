//! MD 知识图谱：SQLite 元数据索引 + `[[双向链接]]` 图谱。
//! 不用向量、不用图数据库——documents/links 两张表 + 正则解析，构建可审计的知识网络。
//! 同步策略：全量重建（小规模 KB 足够快；增量/文件监听留作后续优化）。

use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const DB_FILE: &str = ".md-graph.db";

fn db_path(root: &Path) -> PathBuf {
    root.join(DB_FILE)
}

fn connect(root: &Path) -> Result<Connection, String> {
    let conn = Connection::open(db_path(root)).map_err(|e| format!("打开图谱库失败: {e}"))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS documents (
            path       TEXT PRIMARY KEY,
            project    TEXT NOT NULL,
            title      TEXT NOT NULL DEFAULT '',
            tags       TEXT NOT NULL DEFAULT '',
            summary    TEXT NOT NULL DEFAULT '',
            mtime      INTEGER NOT NULL DEFAULT 0,
            size       INTEGER NOT NULL DEFAULT 0,
            updated    TEXT NOT NULL DEFAULT '',
            indexed_at TEXT NOT NULL DEFAULT ''
         );
         CREATE TABLE IF NOT EXISTS links (
            src      TEXT NOT NULL,
            dst      TEXT NOT NULL,
            dst_path TEXT,
            PRIMARY KEY (src, dst)
         );
         CREATE INDEX IF NOT EXISTS idx_links_dst ON links(dst_path);
         CREATE INDEX IF NOT EXISTS idx_docs_project ON documents(project);",
    )
    .map_err(|e| format!("初始化图谱库失败: {e}"))?;
    Ok(conn)
}

/// 项目归属：相对 kb 根的第一段目录（无目录的 L1 文件归 root）
fn project_of(rel: &str) -> String {
    match rel.split_once('/') {
        Some((head, _)) => head.to_string(),
        None => "root".to_string(),
    }
}

static LINK_RE: OnceLock<regex::Regex> = OnceLock::new();

fn link_re() -> &'static regex::Regex {
    LINK_RE.get_or_init(|| regex::Regex::new(r"\[\[([^\[\]]+)\]\]").unwrap())
}

/// 提取 `[[链接]]` 目标：支持 [[文件]]、[[文件.md]]、[[文件|别名]]、[[路径/文件]]
pub fn parse_links(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for cap in link_re().captures_iter(content) {
        if let Some(m) = cap.get(1) {
            let target = m.as_str().trim().split('|').next().unwrap_or("").trim();
            if !target.is_empty() {
                out.push(target.to_string());
            }
        }
    }
    out
}

/// 解析链接目标 → 文档相对路径（精确路径 > 文件名 stem 匹配）
fn resolve_link(target: &str, all_paths: &[String], stem_index: &HashMap<String, Vec<String>>) -> Option<String> {
    let t = target.trim().trim_start_matches("./").to_string();
    for cand in [t.clone(), format!("{t}.md")] {
        if all_paths.iter().any(|p| *p == cand) {
            return Some(cand);
        }
    }
    let stem = Path::new(&t)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&t)
        .to_string();
    if let Some(cands) = stem_index.get(&stem) {
        if let Some(dir) = Path::new(&t).parent().map(|d| d.to_path_buf()) {
            if let Some(c) = cands.iter().find(|c| Path::new(c).parent() == Some(dir.as_path())) {
                return Some(c.clone());
            }
        }
        if cands.len() == 1 {
            return Some(cands[0].clone());
        }
        // 多候选：取第一个（简单策略）
        return cands.first().cloned();
    }
    None
}

fn collect_md_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .build();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        let Some(ft) = entry.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        let p = entry.path();
        // 待审目录不进图谱（Phase 3 前置：pending 待确认后才落地）
        if p.components().any(|c| c.as_os_str() == "pending") {
            continue;
        }
        if p.extension().and_then(|e| e.to_str()) == Some("md") {
            files.push(p.to_path_buf());
        }
    }
    files
}

#[derive(Debug, Serialize)]
pub struct GraphSyncReport {
    pub docs: usize,
    pub links: usize,
    pub dangling: usize,
    pub db: String,
}

/// 全量重建图谱：扫描 kb 全部 .md → documents + links
pub fn sync_graph(root: &Path) -> Result<GraphSyncReport, String> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let conn = connect(&root)?;
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let mut docs: Vec<(String, String, String, String, String, i64, i64, String)> = Vec::new();
    let mut contents: Vec<(String, String)> = Vec::new(); // (rel, content)

    for path in collect_md_files(&root) {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        let md = std::fs::metadata(&path).ok();
        let mtime = md
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let size = md.as_ref().map(|m| m.len() as i64).unwrap_or(0);

        let (meta, _) = crate::kb::parse_frontmatter(&content);
        let title = meta
            .get("title")
            .cloned()
            .or_else(|| crate::kb::first_heading(&content))
            .unwrap_or_default();
        let tags = meta.get("tags").cloned().unwrap_or_default();
        let summary = crate::kb::summary(&content, 80);
        let updated = meta.get("updated").cloned().unwrap_or_default();
        let project = project_of(&rel);
        docs.push((rel.clone(), project, title, tags, summary, mtime, size, updated));
        contents.push((rel, content));
    }

    // 重建（全量）
    conn.execute("DELETE FROM documents", [])
        .map_err(|e| format!("清空 documents 失败: {e}"))?;
    conn.execute("DELETE FROM links", [])
        .map_err(|e| format!("清空 links 失败: {e}"))?;

    {
        let mut stmt = conn
            .prepare("INSERT INTO documents (path, project, title, tags, summary, mtime, size, updated, indexed_at)
                      VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)")
            .map_err(|e| format!("prepare 失败: {e}"))?;
        for (p, project, title, tags, summary, mtime, size, updated) in &docs {
            stmt.execute(params![p, project, title, tags, summary, mtime, size, updated, now])
                .map_err(|e| format!("插入文档失败 {p}: {e}"))?;
        }
    }

    let all_paths: Vec<String> = docs.iter().map(|d| d.0.clone()).collect();
    let mut stem_index: HashMap<String, Vec<String>> = HashMap::new();
    for p in &all_paths {
        let stem = Path::new(p)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(p)
            .to_string();
        stem_index.entry(stem).or_default().push(p.clone());
    }

    let mut link_rows: Vec<(String, String)> = Vec::new();
    for (src, content) in &contents {
        for tgt in parse_links(content) {
            link_rows.push((src.clone(), tgt));
        }
    }

    let mut dangling = 0usize;
    {
        let mut stmt = conn
            .prepare("INSERT INTO links (src, dst, dst_path) VALUES (?1, ?2, ?3)")
            .map_err(|e| format!("prepare links 失败: {e}"))?;
        for (src, tgt) in &link_rows {
            let dst_path = resolve_link(tgt, &all_paths, &stem_index);
            if dst_path.is_none() {
                dangling += 1;
            }
            stmt.execute(params![src, tgt, dst_path])
                .map_err(|e| format!("插入链接失败: {e}"))?;
        }
    }

    Ok(GraphSyncReport {
        docs: docs.len(),
        links: link_rows.len(),
        dangling,
        db: DB_FILE.to_string(),
    })
}

// ---------- 查询 ----------

fn find_doc(conn: &Connection, input: &str) -> Result<Option<String>, String> {
    let t = input.trim().trim_start_matches("./").to_string();
    // 精确路径（带 .md）
    for cand in [t.clone(), format!("{t}.md")] {
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents WHERE path = ?1", params![cand], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if n > 0 {
            return Ok(Some(cand));
        }
    }
    // 文件名 stem 匹配
    let stem = Path::new(&t).file_stem().and_then(|s| s.to_str()).unwrap_or(&t).to_string();
    let mut stmt = conn
        .prepare("SELECT path FROM documents WHERE substr(path, 1, ?1) = ?2 OR path = ?2")
        .map_err(|e| e.to_string())?;
    let paths: Vec<String> = stmt
        .query_map(
            params![stem.len() as i64, t.clone()],
            |r| r.get::<_, String>(0),
        )
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    if paths.is_empty() {
        // 按 basename stem 精确匹配
        let mut stmt2 = conn
            .prepare("SELECT path FROM documents")
            .map_err(|e| e.to_string())?;
        let all: Vec<String> = stmt2
            .query_map([], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        let matched: Vec<String> = all
            .iter()
            .filter(|p| {
                Path::new(p)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s == stem)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        if matched.len() == 1 {
            return Ok(matched.first().cloned());
        }
        if matched.len() > 1 {
            // 多候选歧义：返回空（前端提示用精确路径）
            return Ok(None);
        }
        return Ok(None);
    }
    Ok(paths.first().cloned())
}

#[derive(Debug, Serialize)]
pub struct GraphStats {
    pub docs: usize,
    pub projects: usize,
    pub links: usize,
    pub resolved: usize,
    pub dangling: usize,
    pub orphans: usize,
}

pub fn stats(root: &Path) -> Result<GraphStats, String> {
    let conn = connect(root)?;
    let cnt = |sql: &str| -> Result<usize, String> {
        conn.query_row(sql, [], |r| r.get::<_, i64>(0))
            .map(|v| v as usize)
            .map_err(|e| e.to_string())
    };
    let docs = cnt("SELECT COUNT(*) FROM documents")?;
    let links = cnt("SELECT COUNT(*) FROM links")?;
    let resolved = cnt("SELECT COUNT(*) FROM links WHERE dst_path IS NOT NULL")?;
    let dangling = links - resolved;
    let orphans = cnt(
        "SELECT COUNT(*) FROM documents d
         WHERE NOT EXISTS (SELECT 1 FROM links l WHERE l.src = d.path)
           AND NOT EXISTS (SELECT 1 FROM links l WHERE l.dst_path = d.path)",
    )?;
    let projects = cnt("SELECT COUNT(DISTINCT project) FROM documents")?;
    Ok(GraphStats {
        docs,
        projects,
        links,
        resolved,
        dangling,
        orphans,
    })
}

pub fn backlinks(root: &Path, path: &str) -> Result<Vec<String>, String> {
    let conn = connect(root)?;
    let Some(doc) = find_doc(&conn, path)? else {
        return Err(format!("图谱中找不到文档: {path}"));
    };
    let mut stmt = conn
        .prepare("SELECT DISTINCT src FROM links WHERE dst_path = ?1 ORDER BY src")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![doc], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

#[derive(Debug, Serialize)]
pub struct LinkEntry {
    pub dst: String,
    pub dst_path: Option<String>,
    pub resolved: bool,
}

pub fn linked(root: &Path, path: &str) -> Result<Vec<LinkEntry>, String> {
    let conn = connect(root)?;
    let Some(doc) = find_doc(&conn, path)? else {
        return Err(format!("图谱中找不到文档: {path}"));
    };
    let mut stmt = conn
        .prepare("SELECT dst, dst_path FROM links WHERE src = ?1 ORDER BY dst")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![doc], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })
        .map_err(|e| e.to_string())?;
    Ok(rows
        .filter_map(Result::ok)
        .map(|(dst, dst_path)| LinkEntry {
            resolved: dst_path.is_some(),
            dst,
            dst_path,
        })
        .collect())
}

#[derive(Debug, Serialize)]
pub struct NodeInfo {
    pub path: String,
    pub title: String,
    pub project: String,
    pub tags: String,
    pub in_degree: usize,
    pub out_degree: usize,
}

#[derive(Debug, Serialize)]
pub struct EdgeInfo {
    pub src: String,
    pub dst_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GraphData {
    pub nodes: Vec<NodeInfo>,
    pub edges: Vec<EdgeInfo>,
}

/// 全量图谱数据（供 /view 可视化视图）
pub fn graph_data(root: &Path) -> Result<GraphData, String> {
    let conn = connect(root)?;
    let mut stmt = conn
        .prepare("SELECT path, title, project, tags FROM documents ORDER BY path")
        .map_err(|e| e.to_string())?;
    let mut nodes: Vec<NodeInfo> = Vec::new();
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows.flatten() {
        nodes.push(NodeInfo {
            path: row.0,
            title: row.1,
            project: row.2,
            tags: row.3,
            in_degree: 0,
            out_degree: 0,
        });
    }

    let mut stmt = conn
        .prepare("SELECT src, dst_path FROM links ORDER BY src")
        .map_err(|e| e.to_string())?;
    let mut edges: Vec<EdgeInfo> = Vec::new();
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))
        .map_err(|e| e.to_string())?;
    for row in rows.flatten() {
        edges.push(EdgeInfo {
            src: row.0,
            dst_path: row.1,
        });
    }

    // 度数：入链/出链（孤立 = 双零）
    let mut outd: HashMap<&str, usize> = HashMap::new();
    let mut ind: HashMap<&str, usize> = HashMap::new();
    for e in &edges {
        *outd.entry(e.src.as_str()).or_insert(0) += 1;
        if let Some(d) = &e.dst_path {
            *ind.entry(d.as_str()).or_insert(0) += 1;
        }
    }
    for n in &mut nodes {
        n.out_degree = outd.get(n.path.as_str()).copied().unwrap_or(0);
        n.in_degree = ind.get(n.path.as_str()).copied().unwrap_or(0);
    }
    Ok(GraphData { nodes, edges })
}

/// 按名称/路径解析文档（供 /link 等命令）
pub fn resolve_doc(root: &Path, name: &str) -> Result<Option<String>, String> {
    let conn = connect(root)?;
    find_doc(&conn, name)
}

// ---------- 健康审计（Phase 3-A 记忆自组织：盲区/冲突/补链接建议） ----------

#[derive(Debug, Serialize)]
pub struct Mention {
    pub src: String,
    pub dst: String,
    pub dst_path: String,
}

#[derive(Debug, Serialize)]
pub struct AuditReport {
    pub docs: usize,
    pub links: usize,
    pub dangling: Vec<(String, String)>,
    pub orphans: Vec<String>,
    pub no_out: Vec<String>,
    pub duplicates: Vec<(String, usize)>,
    pub mentions: Vec<Mention>,
}

/// 提及未链接检测：正文出现其他文档的「文件名 stem / 标题」但未建 [[链接]]。
/// 关键词策略：中文文件名直接用 stem；ASCII stem（如 MEMORY）用其标题（长度≥3 才用，控噪声）。
pub fn audit(root: &Path) -> Result<AuditReport, String> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let conn = connect(&root)?;

    let docs: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare("SELECT path, title FROM documents ORDER BY path")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };

    // 已存在链接：src -> 链接目标集合（dst_path + 原始 dst）
    let mut existing: HashMap<String, HashSet<String>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT src, dst, dst_path FROM links")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            let set = existing.entry(row.0).or_default();
            set.insert(row.1);
            if let Some(p) = row.2 {
                set.insert(p);
            }
        }
    }

    // 悬空链接
    let dangling: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare("SELECT src, dst FROM links WHERE dst_path IS NULL ORDER BY src")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };

    // 孤立（无入链无出链）
    let orphans: Vec<String> = orphans(&root)?;

    // 无出链（有入链或无，但从不链向别人）
    let no_out: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT path FROM documents d
                 WHERE NOT EXISTS (SELECT 1 FROM links l WHERE l.src = d.path)
                 ORDER BY path",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };

    // 重复标题
    let duplicates: Vec<(String, usize)> = {
        let mut stmt = conn
            .prepare("SELECT title, COUNT(*) FROM documents WHERE title != '' GROUP BY title HAVING COUNT(*) > 1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize)))
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };

    // 提及未链接
    let mut mentions: Vec<Mention> = Vec::new();
    {
        // 关键词表：other_path -> keyword
        let mut keywords: Vec<(String, String)> = Vec::new();
        for (path, title) in &docs {
            let stem = Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(path)
                .to_string();
            if stem.chars().any(|c| !c.is_ascii()) {
                keywords.push((path.clone(), stem));
            } else if title.chars().count() >= 3 {
                // ASCII 文件名（如 MEMORY/INDEX）→ 用标题做关键词
                keywords.push((path.clone(), title.clone()));
            }
        }
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for (src, _) in &docs {
            // 自动生成的 INDEX.md 不参与补链接建议（改动会被下次 /sync 覆盖）
            if src == "INDEX.md" {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(root.join(src)) else {
                continue;
            };
            for (dst_path, kw) in &keywords {
                if dst_path == src {
                    continue;
                }
                if kw.chars().count() < 2 {
                    continue;
                }
                if existing.get(src).map(|s| s.contains(dst_path)).unwrap_or(false) {
                    continue;
                }
                if content.contains(kw.as_str()) && seen.insert((src.clone(), dst_path.clone())) {
                    mentions.push(Mention {
                        src: src.clone(),
                        dst: kw.clone(),
                        dst_path: dst_path.clone(),
                    });
                }
            }
        }
        mentions.sort_by(|a, b| a.src.cmp(&b.src).then(a.dst_path.cmp(&b.dst_path)));
    }

    Ok(AuditReport {
        docs: docs.len(),
        links: conn
            .query_row("SELECT COUNT(*) FROM links", [], |r| r.get::<_, i64>(0))
            .map(|v| v as usize)
            .unwrap_or(0),
        dangling,
        orphans,
        no_out,
        duplicates,
        mentions,
    })
}

/// 关联簇：出链 + 入链（去重，按文档路径）
pub fn related(root: &Path, path: &str) -> Result<Vec<String>, String> {
    let conn = connect(root)?;
    let Some(doc) = find_doc(&conn, path)? else {
        return Err(format!("图谱中找不到文档: {path}"));
    };
    let mut out = HashSet::new();
    let mut stmt = conn
        .prepare("SELECT dst_path FROM links WHERE src = ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![doc], |r| r.get::<_, Option<String>>(0))
        .map_err(|e| e.to_string())?;
    for r in rows.flatten().flatten() {
        out.insert(r);
    }
    let mut stmt = conn
        .prepare("SELECT src FROM links WHERE dst_path = ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![doc], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    for r in rows.filter_map(Result::ok) {
        out.insert(r);
    }
    let mut v: Vec<String> = out.into_iter().collect();
    v.sort();
    Ok(v)
}

pub fn orphans(root: &Path) -> Result<Vec<String>, String> {
    let conn = connect(root)?;
    let mut stmt = conn
        .prepare(
            "SELECT path FROM documents d
             WHERE NOT EXISTS (SELECT 1 FROM links l WHERE l.src = d.path)
               AND NOT EXISTS (SELECT 1 FROM links l WHERE l.dst_path = d.path)
             ORDER BY path",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

#[derive(Debug, Serialize)]
pub struct TagCount {
    pub tag: String,
    pub docs: usize,
}

/// 标签统计：解析 documents.tags（"[a, b]" 逗号分隔）
pub fn tags(root: &Path) -> Result<Vec<TagCount>, String> {
    let conn = connect(root)?;
    let mut stmt = conn
        .prepare("SELECT tags FROM documents WHERE tags != ''")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for tags_str in rows.filter_map(Result::ok) {
        let cleaned = tags_str.trim().trim_start_matches('[').trim_end_matches(']');
        for t in cleaned.split(',').map(|s| s.trim().trim_matches('"')).filter(|s| !s.is_empty()) {
            *counts.entry(t.to_string()).or_insert(0) += 1;
        }
    }
    let mut v: Vec<TagCount> = counts
        .into_iter()
        .map(|(tag, docs)| TagCount { tag, docs })
        .collect();
    v.sort_by(|a, b| b.docs.cmp(&a.docs).then(a.tag.cmp(&b.tag)));
    Ok(v)
}

#[derive(Debug, Serialize)]
pub struct ProjectCount {
    pub project: String,
    pub docs: usize,
}

pub fn projects(root: &Path) -> Result<Vec<ProjectCount>, String> {
    let conn = connect(root)?;
    let mut stmt = conn
        .prepare("SELECT project, COUNT(*) FROM documents GROUP BY project ORDER BY project")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize))
        })
        .map_err(|e| e.to_string())?;
    Ok(rows
        .filter_map(Result::ok)
        .map(|(project, docs)| ProjectCount { project, docs })
        .collect())
}
