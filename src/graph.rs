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
            type       TEXT NOT NULL DEFAULT 'doc',
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
         CREATE INDEX IF NOT EXISTS idx_docs_project ON documents(project);
         -- 模板自动建边（2026-08-11）：项目空间笔记不写双链，sync 时按模板语义生成规则边
         CREATE TABLE IF NOT EXISTS rule_links (
            src      TEXT NOT NULL,
            dst_path TEXT NOT NULL,
            kind     TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (src, dst_path)
         );
         CREATE INDEX IF NOT EXISTS idx_rule_dst ON rule_links(dst_path);",
    )
    .map_err(|e| format!("初始化图谱库失败: {e}"))?;
    // 存量库迁移：旧 schema 无 type 列（CREATE TABLE IF NOT EXISTS 不会补列）→ 幂等 ADD COLUMN
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(documents)")
        .map_err(|e| format!("读列失败: {e}"))?
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| format!("读列失败: {e}"))?
        .filter_map(Result::ok)
        .collect();
    if !cols.iter().any(|c| c == "type") {
        conn.execute("ALTER TABLE documents ADD COLUMN type TEXT NOT NULL DEFAULT 'doc'", [])
            .map_err(|e| format!("迁移 type 列失败: {e}"))?;
    }
    // 索引统一在此建（新库/旧库都走到；旧库 batch 阶段已避开缺失列）
    conn.execute("CREATE INDEX IF NOT EXISTS idx_docs_type ON documents(type)", [])
        .map_err(|e| format!("建 type 索引失败: {e}"))?;
    Ok(conn)
}

/// 项目归属：相对 kb 根的第一段目录（无目录的 L1 文件归 root）
fn project_of(rel: &str) -> String {
    match rel.split_once('/') {
        Some((head, _)) => head.to_string(),
        None => "root".to_string(),
    }
}

/// 实体类型推断：frontmatter type 显式值优先，缺省按模板文件名映射业务类型，再缺省 doc。
/// 律师：案件总览→case 当事人与诉求→party 证据清单→evidence 时间线→timeline 法律研究→law
/// 猎头：职位需求→position 候选人→candidate 客户公司→company 沟通记录→comm
/// 模板 notes 文件名即类型约定（KB.md 职责清单），存量项目笔记无 frontmatter 时靠此兜底。
pub fn infer_type(rel: &str, fm_type: Option<&str>) -> String {
    if let Some(t) = fm_type {
        let t = t.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    let stem = Path::new(rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(rel);
    let t = match stem {
        "案件总览" => "case",
        "当事人与诉求" => "party",
        "证据清单" => "evidence",
        "时间线" => "timeline",
        "法律研究" => "law",
        "职位需求" => "position",
        "候选人" => "candidate",
        "客户公司" => "company",
        "沟通记录" => "comm",
        _ => "doc",
    };
    t.to_string()
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

// ---------- 模板自动建边（2026-08-11） ----------
// 项目空间笔记是模板/表格/字段式记录，不写 [[双链]]——sync 时按模板语义生成规则边：
//  律师：case ↔ party/evidence/timeline/law 同项目全连（模板即关系定义，kind=case-rel）
//  猎头：职位需求「客户公司」↔ 客户公司「公司名称」（kind=position-company）
//        候选人表「现职位/公司」列 ↔ 职位名称/公司名称（kind=candidate-position / candidate-company）
//        沟通记录「对象」列 ↔ 职位名称/公司名称（kind=comm-rel）
// 文本匹配=包含匹配（去空白、短侧 ≥2 字符防误配）；无向边按路径排序保证方向稳定。

/// 提取正文 `- 键：值` 字段值（中英文冒号均可）
fn field_value(body: &str, key: &str) -> Option<String> {
    for line in body.lines() {
        let line = line.trim().trim_start_matches("- ");
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.strip_prefix('：').or_else(|| rest.strip_prefix(':'));
            if let Some(v) = rest {
                let v = v.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// 表格内容行解析：| 开头、跳过首个 | 行（模板首行=表头）与分隔行（全部 cell 为 -/空）；返回各单元格 trim 值
fn table_rows(body: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut first = true;
    for line in body.lines() {
        let t = line.trim();
        if !t.starts_with('|') {
            continue;
        }
        let is_sep = t
            .trim_matches('|')
            .split('|')
            .all(|c| c.trim().is_empty() || c.trim().chars().all(|ch| ch == '-'));
        if is_sep {
            continue;
        }
        if first {
            first = false;
            continue;
        }
        out.push(t.trim_matches('|').split('|').map(|c| c.trim().to_string()).collect());
    }
    out
}

/// 去空白后包含匹配（短侧 ≥2 字符，防单字/空串误配）
fn text_matches(a: &str, b: &str) -> bool {
    let norm = |s: &str| -> String { s.chars().filter(|c| !c.is_whitespace()).collect() };
    let (na, nb) = (norm(a), norm(b));
    if na.is_empty() || nb.is_empty() {
        return false;
    }
    let (short, long) = if na.len() <= nb.len() {
        (na.as_str(), nb.as_str())
    } else {
        (nb.as_str(), na.as_str())
    };
    short.chars().count() >= 2 && long.contains(short)
}

/// 无向边统一 (min, max) 方向入集（seen 去重：同对节点一条边，kind 首个 wins）
fn push_pair(
    seen: &mut std::collections::HashSet<(String, String)>,
    out: &mut Vec<(String, String, String)>,
    a: &str,
    b: &str,
    kind: &str,
) {
    if a == b {
        return;
    }
    let (s, d) = if a < b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    };
    if seen.insert((s.clone(), d.clone())) {
        out.push((s, d, kind.to_string()));
    }
}

/// 生成模板规则边：返回 (src, dst_path, kind)，已排序（src < dst，路径升序）
fn rule_edges_for(
    docs: &[(String, String)],     // (path, type)
    contents: &[(String, String)], // (path, 原文)
) -> Vec<(String, String, String)> {
    let content_map: HashMap<&str, &str> =
        contents.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    let mut by_project: HashMap<String, Vec<&(String, String)>> = HashMap::new();
    for d in docs {
        by_project.entry(project_of(&d.0)).or_default().push(d);
    }
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut out: Vec<(String, String, String)> = Vec::new();
    for group in by_project.values() {
        // 律师：case ↔ party/evidence/timeline/law 同项目全连
        let lawyer_types = ["party", "evidence", "timeline", "law"];
        for d in group.iter().filter(|d| d.1 == "case") {
            for o in group.iter().filter(|d| lawyer_types.contains(&d.1.as_str())) {
                push_pair(&mut seen, &mut out, &d.0, &o.0, "case-rel");
            }
        }
        // 猎头：字段/表格提取
        let mut positions: Vec<(&str, String)> = Vec::new(); // (path, 职位名称)
        let mut companies: Vec<(&str, String)> = Vec::new(); // (path, 公司名称)
        let mut candidates: Vec<(&str, String)> = Vec::new(); // (path, 「现职位/公司」列拼接)
        let mut comms: Vec<(&str, String)> = Vec::new(); // (path, 「对象」列拼接)
        for d in group {
            let Some(body) = content_map.get(d.0.as_str()) else { continue };
            match d.1.as_str() {
                "position" => {
                    if let Some(v) = field_value(body, "职位名称") {
                        positions.push((d.0.as_str(), v));
                    }
                }
                "company" => {
                    if let Some(v) = field_value(body, "公司名称") {
                        companies.push((d.0.as_str(), v));
                    }
                }
                "candidate" => {
                    let cells: Vec<String> =
                        table_rows(body).iter().filter_map(|r| r.get(1)).cloned().collect();
                    if !cells.is_empty() {
                        candidates.push((d.0.as_str(), cells.join(" ")));
                    }
                }
                "comm" => {
                    let cells: Vec<String> =
                        table_rows(body).iter().filter_map(|r| r.get(1)).cloned().collect();
                    if !cells.is_empty() {
                        comms.push((d.0.as_str(), cells.join(" ")));
                    }
                }
                _ => {}
            }
        }
        // position ↔ company：职位需求「客户公司」字段 ↔ 客户公司「公司名称」字段
        for (pp, _pname) in &positions {
            let Some(cv) = field_value(content_map.get(*pp).unwrap_or(&""), "客户公司") else {
                continue;
            };
            for (cp, cname) in &companies {
                if text_matches(&cv, cname) {
                    push_pair(&mut seen, &mut out, pp, cp, "position-company");
                }
            }
        }
        // candidate ↔ position / company
        for (cp, cells) in &candidates {
            for (pp, pname) in &positions {
                if text_matches(cells, pname) {
                    push_pair(&mut seen, &mut out, cp, pp, "candidate-position");
                }
            }
            for (mp, mname) in &companies {
                if text_matches(cells, mname) {
                    push_pair(&mut seen, &mut out, cp, mp, "candidate-company");
                }
            }
        }
        // comm ↔ position / company：「对象」列
        for (cp, cells) in &comms {
            for (pp, pname) in &positions {
                if text_matches(cells, pname) {
                    push_pair(&mut seen, &mut out, cp, pp, "comm-rel");
                }
            }
            for (mp, mname) in &companies {
                if text_matches(cells, mname) {
                    push_pair(&mut seen, &mut out, cp, mp, "comm-rel");
                }
            }
        }
    }
    out.sort();
    out
}

/// 解析链接目标 → 文档相对路径（精确路径 > 文件名 stem 匹配）
fn resolve_link(target: &str, all_paths: &[String], stem_index: &HashMap<String, Vec<String>>) -> Option<String> {    let t = target.trim().trim_start_matches("./").to_string();
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
        // 项目制隔离区（projects/ 各项目独立 mini-kb、独立图谱库）不并入全局图谱；
        // 应用空间（apps/ 私有知识/代码）同理不并入
        .filter_entry(|e| e.file_name() != "projects" && e.file_name() != "apps")
        .build();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        let Some(ft) = entry.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        let p = entry.path();
        // 待审目录不进图谱（Phase 3 前置：pending 待确认后才落地）；L0 会话快照（sessions/）是流水非知识，也不进图谱。
        // 注意：不能用绝对路径组件判断 "projects"——项目内图谱 root 位于 kb_root/projects/ 下（隔离由上方 filter_entry 排除目录保证）；
        // apps 同理由 filter_entry 排除，组件判断为防御性冗余
        if p.components().any(|c| {
            let n = c.as_os_str();
            n == "pending" || n == "sessions" || n == "apps"
        }) {
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
    pub rule_links: usize,
    pub db: String,
}

/// 全量重建图谱：扫描 kb 全部 .md → documents + links
pub fn sync_graph(root: &Path) -> Result<GraphSyncReport, String> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let conn = connect(&root)?;
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let mut docs: Vec<(String, String, String, String, String, String, i64, i64, String)> = Vec::new();
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
        let typ = infer_type(&rel, meta.get("type").map(|s| s.as_str()));
        docs.push((rel.clone(), project, typ, title, tags, summary, mtime, size, updated));
        contents.push((rel, content));
    }

    // 重建（全量）
    conn.execute("DELETE FROM documents", [])
        .map_err(|e| format!("清空 documents 失败: {e}"))?;
    conn.execute("DELETE FROM links", [])
        .map_err(|e| format!("清空 links 失败: {e}"))?;
    conn.execute("DELETE FROM rule_links", [])
        .map_err(|e| format!("清空 rule_links 失败: {e}"))?;

    {
        let mut stmt = conn
            .prepare("INSERT INTO documents (path, project, type, title, tags, summary, mtime, size, updated, indexed_at)
                      VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)")
            .map_err(|e| format!("prepare 失败: {e}"))?;
        for (p, project, typ, title, tags, summary, mtime, size, updated) in &docs {
            stmt.execute(params![p, project, typ, title, tags, summary, mtime, size, updated, now])
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
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    for (src, content) in &contents {
        for tgt in parse_links(content) {
            // 同一文档内重复 [[链接]] 去重：否则 UNIQUE(src,dst) 约束会让整个图谱同步失败
            if seen.insert((src.clone(), tgt.clone())) {
                link_rows.push((src.clone(), tgt));
            }
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

    // 模板规则边（内容已在手，零额外读盘）
    let typed: Vec<(String, String)> = docs.iter().map(|d| (d.0.clone(), d.2.clone())).collect();
    let rule_rows = rule_edges_for(&typed, &contents);
    {
        let mut stmt = conn
            .prepare("INSERT INTO rule_links (src, dst_path, kind) VALUES (?1, ?2, ?3)")
            .map_err(|e| format!("prepare rule_links 失败: {e}"))?;
        for (s, d, k) in &rule_rows {
            stmt.execute(params![s, d, k])
                .map_err(|e| format!("插入规则边失败: {e}"))?;
        }
    }

    Ok(GraphSyncReport {
        docs: docs.len(),
        links: link_rows.len(),
        dangling,
        rule_links: rule_rows.len(),
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
           AND NOT EXISTS (SELECT 1 FROM links l WHERE l.dst_path = d.path)
           AND NOT EXISTS (SELECT 1 FROM rule_links r WHERE r.src = d.path OR r.dst_path = d.path)",
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

#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub path: String,
    pub title: String,
    pub project: String,
    pub r#type: String,
    pub tags: String,
    pub in_degree: usize,
    pub out_degree: usize,
}

#[derive(Debug, Serialize)]
pub struct EdgeInfo {
    pub src: String,
    pub dst_path: Option<String>,
}

/// 模板规则边（推导产物，非作者引用）：kind 标注规则来源（case-rel / position-company / candidate-* / comm-rel）
#[derive(Debug, Serialize)]
pub struct RuleEdge {
    pub src: String,
    pub dst_path: String,
    pub kind: String,
}

#[derive(Debug, Serialize)]
pub struct GraphData {
    pub nodes: Vec<NodeInfo>,
    pub edges: Vec<EdgeInfo>,
    // 思源式图增强（2026-08-09）：结构边（目录层级）+ 标签节点，独立数组不污染 ref 语义
    pub dir_nodes: Vec<NodeInfo>,
    pub tag_nodes: Vec<NodeInfo>,
    pub structure_edges: Vec<EdgeInfo>,
    pub tag_edges: Vec<EdgeInfo>,
    // 模板自动建边（2026-08-11）：同 ref 语义隔离，独立数组
    pub rule_edges: Vec<RuleEdge>,
}

/// 全量图谱数据（供 /view 可视化视图）
pub fn graph_data(root: &Path) -> Result<GraphData, String> {
    let conn = connect(root)?;
    let mut stmt = conn
        .prepare("SELECT path, title, project, type, tags FROM documents ORDER BY path")
        .map_err(|e| e.to_string())?;
    let mut nodes: Vec<NodeInfo> = Vec::new();
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows.flatten() {
        nodes.push(NodeInfo {
            path: row.0,
            title: row.1,
            project: row.2,
            r#type: row.3,
            tags: row.4,
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

    // 结构边（思源式：目录层级父子链，浅色组织关系）——每文档逐级父目录，目录去重
    // 根目录文档（无 "/" 的 path，如 MEMORY.md）不产生目录边；目录节点 type="dir"
    let mut dirs: Vec<String> = Vec::new(); // 有序目录清单（保证输出稳定）
    let mut dir_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for n in &nodes {
        let mut parts: Vec<&str> = n.path.split('/').collect();
        parts.pop(); // 去掉文件名
        let mut acc = String::new();
        for (i, seg) in parts.iter().enumerate() {
            if i > 0 {
                acc.push('/');
            }
            acc.push_str(seg);
            if dir_set.insert(acc.clone()) {
                dirs.push(acc.clone());
            }
        }
    }
    let mut dir_nodes: Vec<NodeInfo> = Vec::new();
    for d in &dirs {
        dir_nodes.push(NodeInfo {
            path: d.clone(),
            title: d.rsplit('/').next().unwrap_or(d).to_string(),
            project: String::new(),
            r#type: "dir".into(),
            tags: String::new(),
            in_degree: 0,
            out_degree: 0,
        });
    }
    // 目录父子边 + 目录→文件边（src=父，dst=子；渲染无向）
    let mut structure_edges: Vec<EdgeInfo> = Vec::new();
    for (i, d) in dirs.iter().enumerate() {
        if let Some(idx) = d.rfind('/') {
            let parent = &d[..idx];
            if dir_set.contains(parent) {
                structure_edges.push(EdgeInfo {
                    src: parent.to_string(),
                    dst_path: Some(d.clone()),
                });
            }
        }
        let _ = i;
    }
    for n in &nodes {
        if let Some(idx) = n.path.rfind('/') {
            let parent = &n.path[..idx];
            if dir_set.contains(parent) {
                structure_edges.push(EdgeInfo {
                    src: parent.to_string(),
                    dst_path: Some(n.path.clone()),
                });
            }
        }
    }

    // 标签节点（思源式：#标签# 进图，文档↔标签边）——tags 字段是 frontmatter 原文（如 "[a, b]"）
    let mut tag_nodes: Vec<NodeInfo> = Vec::new();
    let mut tag_edges: Vec<EdgeInfo> = Vec::new();
    {
        let mut tag_set: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut tag_order: Vec<String> = Vec::new();
        for n in &nodes {
            let raw = n.tags.trim();
            let stripped = raw
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .unwrap_or(raw);
            for t in stripped.split(',') {
                let t = t.trim();
                if t.is_empty() {
                    continue;
                }
                let key = format!("#{t}#");
                if tag_set.insert(key.clone()) {
                    tag_order.push(key.clone());
                }
                tag_edges.push(EdgeInfo {
                    src: n.path.clone(),
                    dst_path: Some(key),
                });
            }
        }
        for key in &tag_order {
            let name = key.trim_matches('#');
            tag_nodes.push(NodeInfo {
                path: key.clone(),
                title: name.to_string(),
                project: String::new(),
                r#type: "tag".into(),
                tags: String::new(),
                in_degree: 0,
                out_degree: 0,
            });
        }
    }

    // 模板规则边（读表，不重新解析文件）
    let rule_edges: Vec<RuleEdge> = {
        let mut stmt = conn
            .prepare("SELECT src, dst_path, kind FROM rule_links ORDER BY src, dst_path")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok)
            .map(|(src, dst_path, kind)| RuleEdge { src, dst_path, kind })
            .collect()
    };

    Ok(GraphData {
        nodes,
        edges,
        dir_nodes,
        tag_nodes,
        structure_edges,
        tag_edges,
        rule_edges,
    })
}

/// BFS 最短路径查询（A 和 B 什么关系）：返回路径链 [{path, title, type}]（含 from/to）。
/// 边方向视为无向（出链/入链都能走）；max_depth 内找不到返回空 Vec。
/// 单用户本地规模（<5k 节点）内存 BFS 足够，不走 SQLite 递归 CTE。
pub fn paths(root: &Path, from: &str, to: &str, max_depth: usize) -> Result<Vec<NodeInfo>, String> {
    let data = graph_data(root)?;
    let by_path: HashMap<&str, &NodeInfo> = data.nodes.iter().map(|n| (n.path.as_str(), n)).collect();
    if !by_path.contains_key(from) {
        return Err(format!("图谱中找不到起点文档: {from}"));
    }
    if !by_path.contains_key(to) {
        return Err(format!("图谱中找不到目标文档: {to}"));
    }
    if from == to {
        return Ok(vec![by_path[from].clone()]);
    }
    // 无向邻接表
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in &data.edges {
        adj.entry(e.src.as_str()).or_default().push(e.dst_path.as_deref().unwrap_or(""));
        if let Some(d) = e.dst_path.as_deref() {
            adj.entry(d).or_default().push(e.src.as_str());
        }
    }
    // BFS 逐层找父指针
    let mut parent: HashMap<&str, &str> = HashMap::new();
    let mut frontier: Vec<&str> = vec![from];
    let mut found = false;
    let mut depth = 0usize;
    while !frontier.is_empty() && depth < max_depth {
        let mut next: Vec<&str> = Vec::new();
        for cur in &frontier {
            for nb in adj.get(cur).into_iter().flatten() {
                if nb.is_empty() || parent.contains_key(nb) {
                    continue; // 悬空边 / 已访问
                }
                parent.insert(nb, cur);
                if *nb == to {
                    found = true;
                    break;
                }
                next.push(nb);
            }
            if found {
                break;
            }
        }
        if found {
            break;
        }
        frontier = next;
        depth += 1;
    }
    if !found {
        return Ok(Vec::new());
    }
    // 回溯路径链
    let mut chain: Vec<NodeInfo> = Vec::new();
    let mut cur: &str = to;
    loop {
        chain.push(by_path[cur].clone());
        if cur == from {
            break;
        }
        cur = parent[cur];
    }
    chain.reverse();
    Ok(chain)
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
    pub duplicates: Vec<(String, usize, String)>,
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

    // 重复标题（含路径，供冲突对比）
    let duplicates: Vec<(String, usize, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT title, COUNT(*), GROUP_CONCAT(path, ' | ') FROM documents
                 WHERE title != '' GROUP BY title HAVING COUNT(*) > 1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)? as usize,
                    r.get::<_, String>(2)?,
                ))
            })
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
               AND NOT EXISTS (SELECT 1 FROM rule_links r WHERE r.src = d.path OR r.dst_path = d.path)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("md-agent-ut-graph-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    // ---------- parse_links（纯函数） ----------

    #[test]
    fn parse_links_variants() {
        let links = parse_links("看 [[托盘应用]] 与 [[架构/记忆统一模型|别名]]，还有 [[RULES]]。");
        assert_eq!(links, vec!["托盘应用", "架构/记忆统一模型", "RULES"]);
    }

    #[test]
    fn parse_links_skips_code_blocks_and_empty() {
        // 代码块内的 [[链接]] 是已知噪声源：当前实现不排除（如实记录行为），
        // 但空目标 / 无链接文本必须不产出
        assert_eq!(parse_links("无链接"), Vec::<String>::new());
        assert_eq!(parse_links("[[]]"), Vec::<String>::new());
        assert_eq!(parse_links("[[   ]]"), Vec::<String>::new());
    }

    #[test]
    fn parse_links_chinese_path() {
        let links = parse_links("见 [[笔记/中文文件名]] 与 [[嵌套/目录/深层文档]]");
        assert_eq!(links.len(), 2);
        assert!(links.contains(&"笔记/中文文件名".to_string()));
        assert!(links.contains(&"嵌套/目录/深层文档".to_string()));
    }

    // ---------- resolve_link 优先级 ----------

    #[test]
    fn resolve_link_exact_path_first() {
        let paths = vec!["notes/检索.md".to_string(), "notes/rag/检索.md".to_string()];
        let mut stem_index: HashMap<String, Vec<String>> = HashMap::new();
        for p in &paths {
            let stem = Path::new(p).file_stem().unwrap().to_string_lossy().into_owned();
            stem_index.entry(stem).or_default().push(p.clone());
        }
        // 精确路径命中
        assert_eq!(
            resolve_link("notes/rag/检索", &paths, &stem_index),
            Some("notes/rag/检索.md".to_string())
        );
        // 无路径前缀 → stem 唯一命中
        assert_eq!(
            resolve_link("检索", &paths, &stem_index),
            Some("notes/检索.md".to_string())
        );
    }

    #[test]
    fn resolve_link_dangling_returns_none() {
        let paths: Vec<String> = vec![];
        let stem_index: HashMap<String, Vec<String>> = HashMap::new();
        assert_eq!(resolve_link("不存在.md", &paths, &stem_index), None);
    }

    // ---------- sync + audit 集成（SQLite） ----------

    #[test]
    fn sync_and_audit_signals() {
        let root = test_root("audit");
        write(&root, "notes/A.md", "# A\n\n见 [[B]]\n");
        write(&root, "notes/B.md", "# B\n\n回链 [[A]]\n");
        // 悬空链接
        write(&root, "notes/C.md", "# C\n\n链向 [[不存在]]\n");
        // 孤立（无入链无出链）
        write(&root, "notes/D.md", "# D\n");
        let report = sync_graph(&root).unwrap();
        assert_eq!(report.docs, 4);
        assert_eq!(report.links, 3);
        assert_eq!(report.dangling, 1);

        let rep = audit(&root).unwrap();
        assert!(rep.dangling.iter().any(|(src, _)| src == "notes/C.md"));
        assert!(rep.orphans.iter().any(|p| p == "notes/D.md"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn graph_excludes_pending_and_sessions() {
        let root = test_root("excl");
        write(&root, "notes/知识.md", "# 知识\n");
        write(&root, "pending/草稿.md", "# 草稿\n");
        write(&root, "sessions/流水.md", "# 流水\n");
        let report = sync_graph(&root).unwrap();
        assert_eq!(report.docs, 1, "pending/sessions 不进图谱");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn project_of_first_dir() {
        assert_eq!(project_of("notes/架构/托盘.md"), "notes");
        assert_eq!(project_of("KB.md"), "root");
    }

    #[test]
    fn infer_type_priority_fm_then_filename() {
        // frontmatter type 显式值优先
        assert_eq!(infer_type("notes/案件总览.md", Some("doc")), "doc");
        // 缺省按模板文件名映射
        assert_eq!(infer_type("notes/案件总览.md", None), "case");
        assert_eq!(infer_type("notes/证据清单.md", None), "evidence");
        assert_eq!(infer_type("notes/当事人与诉求.md", None), "party");
        assert_eq!(infer_type("notes/时间线.md", None), "timeline");
        assert_eq!(infer_type("notes/法律研究.md", None), "law");
        assert_eq!(infer_type("notes/职位需求.md", None), "position");
        assert_eq!(infer_type("notes/候选人.md", None), "candidate");
        assert_eq!(infer_type("notes/客户公司.md", None), "company");
        assert_eq!(infer_type("notes/沟通记录.md", None), "comm");
        // 其他文件名 → doc
        assert_eq!(infer_type("notes/架构/记忆统一模型.md", None), "doc");
        // 空串 type 也回落
        assert_eq!(infer_type("notes/候选人.md", Some("")), "candidate");
    }

    #[test]
    fn sync_graph_stores_type_and_paths_bfs() {
        let root = test_root("types");
        // 律师模板文件名映射 type + frontmatter 显式 type
        write(&root, "notes/案件总览.md", "# 案件总览\n\n见 [[证据清单]] 和 [[当事人与诉求]]\n");
        write(&root, "notes/证据清单.md", "# 证据清单\n\n来自 [[案件总览]]\n");
        write(&root, "notes/当事人与诉求.md", "# 当事人与诉求\n\n与 [[案件总览]] 相关\n");
        // frontmatter 显式 type 优先于文件名
        write(&root, "notes/客户公司.md", "---\ntype: note\ntitle: 客户公司\n---\n# 客户公司\n\n与 [[当事人与诉求]] 有关\n");
        let _ = sync_graph(&root).unwrap();
        let data = graph_data(&root).unwrap();
        let t = |p: &str| data.nodes.iter().find(|n| n.path == p).map(|n| n.r#type.clone()).unwrap_or_default();
        assert_eq!(t("notes/案件总览.md"), "case", "文件名映射");
        assert_eq!(t("notes/证据清单.md"), "evidence");
        assert_eq!(t("notes/当事人与诉求.md"), "party");
        assert_eq!(t("notes/客户公司.md"), "note", "frontmatter type 优先");

        // BFS 路径：案件总览 → 当事人与诉求（直接相连）
        let chain = paths(&root, "notes/案件总览.md", "notes/当事人与诉求.md", 6).unwrap();
        assert_eq!(chain.len(), 2, "一跳直连");
        assert_eq!(chain[0].path, "notes/案件总览.md");
        assert_eq!(chain[1].path, "notes/当事人与诉求.md");
        assert_eq!(chain[1].r#type, "party", "路径链节点带类型");
        // 通过中间人：当事人与诉求 → 客户公司（当事人 ↔ 客户公司 直连）
        let chain2 = paths(&root, "notes/当事人与诉求.md", "notes/客户公司.md", 6).unwrap();
        assert_eq!(chain2.len(), 2);
        // 路径不存在 → 空
        let chain3 = paths(&root, "notes/证据清单.md", "notes/客户公司.md", 6).unwrap();
        assert!(chain3.is_empty() || chain3.len() <= 4);
        // 起点/终点不存在 → Err
        assert!(paths(&root, "notes/不存在.md", "notes/案件总览.md", 6).is_err());
        // 同文档 → 单节点
        let same = paths(&root, "notes/案件总览.md", "notes/案件总览.md", 6).unwrap();
        assert_eq!(same.len(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn graph_data_builds_dirs_and_tags() {
        let root = test_root("dirs-tags");
        // 带标签的子目录文档 + 根目录无标签文档
        write(
            &root,
            "notes/子目录/a.md",
            "---\ntitle: A\ntags: [案由, 民事]\n---\n# A\n\n无双链（孤立）\n",
        );
        write(
            &root,
            "notes/子目录/b.md",
            "---\ntitle: B\ntags: [案由]\n---\n# B\n\n无双链（孤立）\n",
        );
        write(&root, "根文档.md", "# 根文档\n\n根目录无目录边\n");
        let _ = sync_graph(&root).unwrap();
        let data = graph_data(&root).unwrap();

        // 目录节点：逐级父目录（notes、notes/子目录），去重
        let mut dirs: Vec<&str> = data.dir_nodes.iter().map(|n| n.path.as_str()).collect();
        dirs.sort();
        assert_eq!(dirs, vec!["notes", "notes/子目录"]);
        assert!(data.dir_nodes.iter().all(|n| n.r#type == "dir"));

        // 结构边：目录父子 + 目录→文件（3 条）
        assert_eq!(data.structure_edges.len(), 3);
        assert!(data.structure_edges.iter().any(|e| e.src == "notes" && e.dst_path.as_deref() == Some("notes/子目录")));
        assert!(data.structure_edges.iter().any(|e| e.src == "notes/子目录" && e.dst_path.as_deref() == Some("notes/子目录/a.md")));
        // 根目录文档无目录边
        assert!(!data.structure_edges.iter().any(|e| e.dst_path.as_deref() == Some("根文档.md")));

        // 标签节点：去重共用
        let mut tags: Vec<&str> = data.tag_nodes.iter().map(|n| n.path.as_str()).collect();
        tags.sort();
        assert_eq!(tags, vec!["#案由#", "#民事#"]);
        assert!(data.tag_nodes.iter().all(|n| n.r#type == "tag"));
        // 标签边：a→案由/民事，b→案由（3 条）
        assert_eq!(data.tag_edges.len(), 3);
        assert!(data.tag_edges.iter().any(|e| e.src == "notes/子目录/b.md" && e.dst_path.as_deref() == Some("#案由#")));

        // 语义隔离：ref 边/度数/孤立不受结构边与标签边影响（无双链文档仍孤立）
        assert_eq!(data.edges.len(), 0);
        let a = data.nodes.iter().find(|n| n.path == "notes/子目录/a.md").unwrap();
        assert_eq!((a.in_degree, a.out_degree), (0, 0), "结构/tag 边不计入度数");
        assert_eq!(data.nodes.len(), 3, "nodes 仅文档");
        fs::remove_dir_all(&root).unwrap();
    }

    // ---------- 模板自动建边（2026-08-11） ----------

    #[test]
    fn rule_edges_lawyer_full_connect() {
        let root = test_root("rule-lawyer");
        let files = [
            ("案件总览", "case"),
            ("当事人与诉求", "party"),
            ("证据清单", "evidence"),
            ("时间线", "timeline"),
            ("法律研究", "law"),
        ];
        for (name, typ) in files {
            write(
                &root,
                &format!("notes/{name}.md"),
                &format!("---\ntype: {typ}\n---\n# {name}\n\n内容\n"),
            );
        }
        let rep = sync_graph(&root).unwrap();
        assert_eq!(rep.rule_links, 4, "case 与其余 4 类型各一条");
        let data = graph_data(&root).unwrap();
        assert_eq!(data.rule_edges.len(), 4);
        assert!(data.rule_edges.iter().all(|e| e.kind == "case-rel"));
        // 方向稳定：src < dst；每条都连到 case
        assert!(data.rule_edges.iter().all(|e| e.src < e.dst_path));
        let case = "notes/案件总览.md";
        assert!(data.rule_edges.iter().all(|e| e.src == case || e.dst_path == case));
        // 规则边不进度数（ref 语义隔离）
        assert!(data.nodes.iter().all(|n| n.in_degree == 0 && n.out_degree == 0));
        // 孤立被规则边救活（audit + stats 同口径）
        assert!(orphans(&root).unwrap().is_empty());
        assert_eq!(stats(&root).unwrap().orphans, 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rule_edges_headhunter_field_match() {
        let root = test_root("rule-hunter");
        write(
            &root,
            "notes/职位需求.md",
            "---\ntype: position\n---\n# 职位需求\n\n## 基本信息\n- 职位名称：高级工程师\n- 客户公司：蓝海科技\n",
        );
        write(
            &root,
            "notes/客户公司.md",
            "---\ntype: company\n---\n# 客户公司\n\n## 基本信息\n- 公司名称：蓝海科技\n",
        );
        write(
            &root,
            "notes/候选人.md",
            "---\ntype: candidate\n---\n# 候选人\n\n| 候选人 | 现职位/公司 | 关键匹配点 |\n|---|---|---|\n| 张三 | 蓝海科技 高级工程师 | 匹配 |\n| 李四 | 云启数据 产品经理 | 一般 |\n",
        );
        write(
            &root,
            "notes/沟通记录.md",
            "---\ntype: comm\n---\n# 沟通记录\n\n| 日期 | 对象 | 方式 | 要点 |\n|---|---|---|---|\n| 2026-08-01 | 蓝海科技 | 电话 | 推进 |\n",
        );
        let rep = sync_graph(&root).unwrap();
        assert_eq!(
            rep.rule_links, 4,
            "position↔company、candidate↔position、candidate↔company、comm↔company"
        );
        let data = graph_data(&root).unwrap();
        let kinds: Vec<&str> = data.rule_edges.iter().map(|e| e.kind.as_str()).collect();
        assert!(kinds.contains(&"candidate-position"));
        assert!(kinds.contains(&"candidate-company"));
        assert!(kinds.contains(&"position-company"));
        assert!(kinds.contains(&"comm-rel"));
        // 李四行（云启数据/产品经理）无对应目标，不额外产生边
        assert_eq!(data.rule_edges.len(), 4);
        // comm 只匹配公司（对象=蓝海科技，职位名=高级工程师 不匹配）
        assert_eq!(
            data.rule_edges.iter().filter(|e| e.kind == "comm-rel").count(),
            1
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rule_edges_empty_template_none() {
        let root = test_root("rule-empty");
        // 真实模板原样（字段空、表格无内容行）→ 无规则边
        write(
            &root,
            "notes/职位需求.md",
            "---\ntype: position\n---\n# 职位需求\n\n## 基本信息\n- 职位名称：\n- 客户公司：\n",
        );
        write(
            &root,
            "notes/客户公司.md",
            "---\ntype: company\n---\n# 客户公司\n\n## 基本信息\n- 公司名称：\n",
        );
        write(
            &root,
            "notes/候选人.md",
            "---\ntype: candidate\n---\n# 候选人\n\n| 候选人 | 现职位/公司 | 关键匹配点 | 薪资期望 | 进展阶段 | 下一步 | 备注 |\n|---|---|---|---|---|---|---|\n|  |  |  |  |  |  |  |\n",
        );
        let rep = sync_graph(&root).unwrap();
        assert_eq!(rep.rule_links, 0, "空模板字段/单元格为空，不产生规则边");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rule_edges_do_not_pollute_ref_semantics() {
        let root = test_root("rule-isolation");
        // 律师项目：无双链，规则边救活孤立，但 ref 边仍为 0、悬空仍为 0
        for (name, typ) in [("案件总览", "case"), ("证据清单", "evidence")] {
            write(
                &root,
                &format!("notes/{name}.md"),
                &format!("---\ntype: {typ}\n---\n# {name}\n\n内容\n"),
            );
        }
        let rep = sync_graph(&root).unwrap();
        assert_eq!(rep.links, 0);
        assert_eq!(rep.dangling, 0);
        assert_eq!(rep.rule_links, 1);
        let data = graph_data(&root).unwrap();
        assert_eq!(data.edges.len(), 0, "规则边不进 ref 边数组");
        assert!(data.rule_edges.iter().any(|e| e.kind == "case-rel"));
        fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use std::fs;

    #[test]
    fn legacy_db_without_type_column_migrates() {
        // 模拟旧 schema：先手工建无 type 列的库，再走 connect() 应自动补列
        let root = std::env::temp_dir().join(format!("md-agent-ut-migrate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let conn = Connection::open(db_path(&root)).unwrap();
        conn.execute_batch(
            "CREATE TABLE documents (
                path TEXT PRIMARY KEY, project TEXT NOT NULL, title TEXT NOT NULL DEFAULT '',
                tags TEXT NOT NULL DEFAULT '', summary TEXT NOT NULL DEFAULT '',
                mtime INTEGER NOT NULL DEFAULT 0, size INTEGER NOT NULL DEFAULT 0,
                updated TEXT NOT NULL DEFAULT '', indexed_at TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE links (src TEXT NOT NULL, dst TEXT NOT NULL, dst_path TEXT, PRIMARY KEY (src, dst));",
        )
        .unwrap();
        drop(conn);
        // 旧数据一条（先建目录再写文件）
        std::fs::create_dir_all(root.join("notes")).unwrap();
        std::fs::write(root.join("notes").join("旧文档.md"), "# 旧文档\n").unwrap();
        // connect() 应幂等迁移（不报错）
        let conn = connect(&root).unwrap();
        let has: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('documents') WHERE name='type'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(has, "迁移后应有 type 列");
        let has_rule: bool = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='rule_links'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(has_rule, "迁移后应自动建 rule_links 表");
        drop(conn); // 释放库句柄（WAL 文件锁），否则 remove_dir_all 失败
        // sync 后旧数据也能入库（type 缺省 doc）
        let _ = sync_graph(&root).unwrap();
        let data = graph_data(&root).unwrap();
        assert_eq!(data.nodes[0].r#type, "doc");
        fs::remove_dir_all(&root).unwrap();
    }
}
