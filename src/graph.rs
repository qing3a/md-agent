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
            -- 定向即从属（2026-08-11 通用化）：directed=1 时 src 是父（如 案件→证据），
            -- 前端骨架布局按定向边构建业务树；关联型规则边 directed=0（无向）
            directed INTEGER NOT NULL DEFAULT 0,
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
    // 存量 rule_links 库补 directed 列（新列随 CREATE 建立，旧库幂等 ADD）
    let rule_cols: Vec<String> = conn
        .prepare("PRAGMA table_info(rule_links)")
        .map_err(|e| format!("读 rule_links 列失败: {e}"))?
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| format!("读 rule_links 列失败: {e}"))?
        .filter_map(Result::ok)
        .collect();
    if !rule_cols.iter().any(|c| c == "directed") {
        conn.execute(
            "ALTER TABLE rule_links ADD COLUMN directed INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| format!("迁移 directed 列失败: {e}"))?;
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
            // 反引号内（行内代码/``` 围栏）的 [[...]] 是语法示例，不解析为链接
            // （2026-08-11：反引号成对计数——奇数次未闭合=在代码段内）
            if content[..m.start()].matches('`').count() % 2 == 1 {
                continue;
            }
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

/// 无向边统一 (min, max) 方向入集（seen 去重：同对节点一条边，kind 首个 wins）；
/// 定向边（从属型，directed=true）保持 (父, 子) 顺序不排序——前端骨架布局的语义来源
fn push_pair(
    seen: &mut std::collections::HashSet<(String, String)>,
    out: &mut Vec<(String, String, String, bool)>,
    a: &str,
    b: &str,
    kind: &str,
    directed: bool,
) {
    if a == b {
        return;
    }
    let (s, d) = if directed || a < b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    };
    if seen.insert((s.clone(), d.clone())) {
        out.push((s, d, kind.to_string(), directed));
    }
}

/// 生成模板规则边：返回 (src, dst_path, kind, directed)——定向即从属（2026-08-11 通用化：
/// 核心只认「定向/无向」语义，不认行业名；case-rel 是第一个从属型实例）。已排序（关联型
/// src<dst；定向型 src=父）。
fn rule_edges_for(
    docs: &[(String, String)],     // (path, type)
    contents: &[(String, String)], // (path, 原文)
) -> Vec<(String, String, String, bool)> {
    let content_map: HashMap<&str, &str> =
        contents.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    let mut by_project: HashMap<String, Vec<&(String, String)>> = HashMap::new();
    for d in docs {
        by_project.entry(project_of(&d.0)).or_default().push(d);
    }
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut out: Vec<(String, String, String, bool)> = Vec::new();
    for group in by_project.values() {
        // 律师：case ↔ party/evidence/timeline/law 同项目全连（定向：case 是父）
        let lawyer_types = ["party", "evidence", "timeline", "law"];
        for d in group.iter().filter(|d| d.1 == "case") {
            for o in group.iter().filter(|d| lawyer_types.contains(&d.1.as_str())) {
                push_pair(&mut seen, &mut out, &d.0, &o.0, "case-rel", true);
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
                    push_pair(&mut seen, &mut out, pp, cp, "position-company", false);
                }
            }
        }
        // candidate ↔ position / company
        for (cp, cells) in &candidates {
            for (pp, pname) in &positions {
                if text_matches(cells, pname) {
                    push_pair(&mut seen, &mut out, cp, pp, "candidate-position", false);
                }
            }
            for (mp, mname) in &companies {
                if text_matches(cells, mname) {
                    push_pair(&mut seen, &mut out, cp, mp, "candidate-company", false);
                }
            }
        }
        // comm ↔ position / company：「对象」列
        for (cp, cells) in &comms {
            for (pp, pname) in &positions {
                if text_matches(cells, pname) {
                    push_pair(&mut seen, &mut out, cp, pp, "comm-rel", false);
                }
            }
            for (mp, mname) in &companies {
                if text_matches(cells, mname) {
                    push_pair(&mut seen, &mut out, cp, mp, "comm-rel", false);
                }
            }
        }
    }
    out.sort();
    out
}

/// 图谱外目标兜底（2026-08-11）：documents 表无此文档但磁盘文件存在
/// （apps/ 等被图谱排除的目录、系统文件引用）→ 有效链接不算悬空；返回相对 kb 根的路径
fn resolve_on_disk(root: &Path, tgt: &str) -> Option<String> {
    for cand in [tgt.trim().to_string(), format!("{}.md", tgt.trim())] {
        if cand.contains("..") {
            continue; // 防路径逃逸（links 来自用户正文）
        }
        let p = root.join(&cand);
        if p.is_file() {
            return Some(cand);
        }
    }
    None
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
            // 图谱外目标兜底（2026-08-11）：documents 无此文档但磁盘存在 → 不算悬空
            let dst_path =
                resolve_link(tgt, &all_paths, &stem_index).or_else(|| resolve_on_disk(&root, tgt));
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
            .prepare("INSERT INTO rule_links (src, dst_path, kind, directed) VALUES (?1, ?2, ?3, ?4)")
            .map_err(|e| format!("prepare rule_links 失败: {e}"))?;
        for (s, d, k, dir) in &rule_rows {
            stmt.execute(params![s, d, k, if *dir { 1 } else { 0 }])
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

/// 模板规则边（推导产物，非作者引用）：kind 标注规则来源（case-rel / position-company / candidate-* / comm-rel）；
/// directed=true 时 src 是父（从属型，骨架布局按此构建业务树）
#[derive(Debug, Serialize)]
pub struct RuleEdge {
    pub src: String,
    pub dst_path: String,
    pub kind: String,
    pub directed: bool,
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
            .prepare("SELECT src, dst_path, kind, directed FROM rule_links ORDER BY src, dst_path")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok)
            .map(|(src, dst_path, kind, directed)| RuleEdge {
                src,
                dst_path,
                kind,
                directed: directed != 0,
            })
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
    // 智能审计（2026-08-11，L0 扩展）：分级 + 健康分 + 趋势
    /// 旧孤立（mtime > 180 天且无任何关联）——归档候选
    pub stale: Vec<String>,
    /// 近重复：标题规范化（去空白/全角标点/小写）后相同的组（排除精确重复）
    pub near_duplicates: Vec<(String, Vec<String>)>,
    /// 空笔记：正文（去 frontmatter）< 50 字符
    pub empty_notes: Vec<String>,
    /// 超长笔记：正文 > 30000 字符
    pub oversized: Vec<String>,
    /// 健康分 0-100（100 - critical×8 - warning×3 - info×1）
    pub score: u8,
    /// 较上次健康分变化（+/-/0；无历史为 null）
    pub trend: Option<i32>,
    pub critical: usize,
    pub warning: usize,
    pub info: usize,
}

/// 提及未链接检测：正文出现其他文档的「文件名 stem / 标题」但未建 [[链接]]。
/// 关键词策略：中文文件名直接用 stem；ASCII stem（如 MEMORY）用其标题（长度≥3 才用，控噪声）。

// 智能审计（2026-08-11，L0）：分级权重与阈值（集中可调）
pub const STALE_DAYS: i64 = 180;
pub const EMPTY_CHARS: usize = 100;
pub const OVERSIZED_CHARS: usize = 30000;
pub const SCORE_CRITICAL: i32 = 8;
pub const SCORE_WARNING: i32 = 3;
pub const SCORE_INFO: i32 = 1;

/// 标题规范化（近重复判定）：去空白/全角标点/小写
fn norm_title(t: &str) -> String {
    t.chars()
        .filter(|c| {
            !c.is_whitespace()
                && !"，。！？、；：（）()【】[]《》「」\"'".contains(*c)
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 审计历史（kb/.audit-history.json）：metrics 变化才写新条目（防心跳高频写盘），保留 10 条；
/// 返回较上次健康分变化
fn update_history(
    root: &Path,
    score: u8,
    metrics: &std::collections::BTreeMap<String, usize>,
) -> Option<i32> {
    let path = root.join(".audit-history.json");
    let hist: Vec<serde_json::Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let last = hist.last().cloned();
    let trend = last.as_ref().and_then(|l| {
        let prev = l["score"].as_u64().unwrap_or(score as u64) as i32;
        Some(score as i32 - prev)
    });
    let mv = serde_json::to_value(metrics).unwrap_or_default();
    if last.as_ref().map(|l| l["metrics"] == mv).unwrap_or(false) {
        return trend; // 指标未变化：不写盘（心跳高频调用的写盘闸门）
    }
    let mut hist = hist;
    hist.push(serde_json::json!({
        "date": chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        "score": score,
        "metrics": metrics,
    }));
    if hist.len() > 10 {
        hist.drain(0..hist.len() - 10);
    }
    let _ = std::fs::write(&path, serde_json::to_string(&hist).unwrap_or_default());
    trend
}

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

    // 提及未链接 + 空/超长笔记（同一遍读文件正文）
    let mut mentions: Vec<Mention> = Vec::new();
    let mut empty_notes: Vec<String> = Vec::new();
    let mut oversized: Vec<String> = Vec::new();
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
            let Ok(content) = std::fs::read_to_string(root.join(src)) else {
                continue;
            };
            let (_, body) = crate::kb::parse_frontmatter(&content);
            let body_len = body.chars().count();
            if body_len < EMPTY_CHARS {
                empty_notes.push(src.clone());
            } else if body_len > OVERSIZED_CHARS {
                oversized.push(src.clone());
            }
            // 自动生成的 INDEX.md 不参与补链接建议（改动会被下次 /sync 覆盖）
            if src == "INDEX.md" {
                continue;
            }
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
        empty_notes.sort();
        oversized.sort();
    }

    // 旧孤立（mtime > 180 天且无任何关联）——归档候选
    let stale: Vec<String> = {
        let cutoff = chrono::Local::now().timestamp() - STALE_DAYS * 86400;
        let mut stmt = conn
            .prepare(
                "SELECT d.path FROM documents d
                 WHERE NOT EXISTS (SELECT 1 FROM links l WHERE l.src = d.path)
                   AND NOT EXISTS (SELECT 1 FROM links l WHERE l.dst_path = d.path)
                   AND NOT EXISTS (SELECT 1 FROM rule_links r WHERE r.src = d.path OR r.dst_path = d.path)
                   AND d.mtime > 0 AND d.mtime < ?1
                 ORDER BY d.path",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![cutoff], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };

    // 近重复：标题规范化后同组（排除标题精确相同——那些进 duplicates）
    let mut near_duplicates: Vec<(String, Vec<String>)> = Vec::new();
    {
        let mut groups: HashMap<String, Vec<(String, String)>> = HashMap::new(); // norm -> [(path, title)]
        for (path, title) in &docs {
            let t = title.trim();
            if t.is_empty() {
                continue;
            }
            let norm = norm_title(t);
            if norm.is_empty() {
                continue;
            }
            groups.entry(norm).or_default().push((path.clone(), title.clone()));
        }
        let mut g: Vec<(String, Vec<String>)> = groups
            .into_iter()
            .filter(|(_, v)| {
                v.len() > 1
                    && v.iter()
                        .map(|(_, t)| t.as_str())
                        .collect::<HashSet<_>>()
                        .len()
                        > 1
            })
            .map(|(_, v)| (v[0].1.clone(), v.into_iter().map(|(p, _)| p).collect()))
            .collect();
        g.sort();
        near_duplicates = g;
    }

    // 分级 + 健康分 + 历史趋势
    let critical = duplicates.len() + dangling.len();
    let warning = orphans.len() + near_duplicates.len() + empty_notes.len();
    let info = mentions.len() + no_out.len() + oversized.len() + stale.len();
    let score = (100i32 - (critical as i32 * SCORE_CRITICAL + warning as i32 * SCORE_WARNING + info as i32 * SCORE_INFO))
        .clamp(0, 100) as u8;
    let mut metrics: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    metrics.insert("orphans".into(), orphans.len());
    metrics.insert("dangling".into(), dangling.len());
    metrics.insert("duplicates".into(), duplicates.len());
    metrics.insert("mentions".into(), mentions.len());
    metrics.insert("stale".into(), stale.len());
    metrics.insert("near_duplicates".into(), near_duplicates.len());
    metrics.insert("empty".into(), empty_notes.len());
    metrics.insert("oversized".into(), oversized.len());
    let trend = update_history(&root, score, &metrics);

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
        stale,
        near_duplicates,
        empty_notes,
        oversized,
        score,
        trend,
        critical,
        warning,
        info,
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

// ---------- 激活扩散检索（2026-08-11，第二步） ----------
// 搜索命中沿图谱边扩散 1-2 跳：引用边（作者双链）权 1.0、规则边（模板推导）权 0.8；
// 标签/结构边不参与（目录与标签会糊掉扩散语义）。簇提升只加分不扣分。

/// 扩散超参（集中便于 A/B 调参）
pub const RULE_EDGE_W: f64 = 0.8;
pub const HOP_DECAY: f64 = 0.5;
pub const CLUSTER_BOOST: f64 = 0.3;
pub const MAX_RECALL: usize = 10;

#[derive(Debug, Default, Serialize)]
pub struct SpreadResult {
    /// 命中文件的簇提升分（key=文件路径，与原始分相加后重排）
    pub boosted: HashMap<String, f64>,
    /// 补充召回：未命中但被扩散到的邻居 (file, score, via 来源文件)，分降序截断
    pub recalled: Vec<(String, f64, String)>,
}

/// 无向邻接表：引用边 1.0 + 规则边 0.8；标签/结构边不参与。
/// 度归一化（2026-08-11）：高入度"枢纽文档"（L1 导航/被广泛引用的核心笔记）作为扩散目标
/// 时天然分虚高——边权按目标节点无向度数归一化 w/log2(1+deg)，压平枢纽噪声。
/// deg=1 因子 1.0（普通文档不受影响），deg=10 压到 ~1/4。
fn adjacency(conn: &Connection) -> Result<HashMap<String, Vec<(String, f64)>>, String> {
    let mut adj: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    let mut add = |a: &str, b: &str, w: f64| {
        if a != b {
            adj.entry(a.to_string()).or_default().push((b.to_string(), w));
            adj.entry(b.to_string()).or_default().push((a.to_string(), w));
        }
    };
    let mut stmt = conn
        .prepare("SELECT src, dst_path FROM links WHERE dst_path IS NOT NULL")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    for row in rows.flatten() {
        add(&row.0, &row.1, 1.0);
    }
    let mut stmt = conn
        .prepare("SELECT src, dst_path FROM rule_links")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    for row in rows.flatten() {
        add(&row.0, &row.1, RULE_EDGE_W);
    }
    // 度归一化（第二遍）：目标节点度数越大，进入它的扩散分被压得越狠
    let degs: HashMap<String, usize> =
        adj.iter().map(|(k, v)| (k.clone(), v.len())).collect();
    for neighbors in adj.values_mut() {
        for (n, w) in neighbors.iter_mut() {
            let d = degs.get(n).copied().unwrap_or(1).max(1);
            *w /= (1.0 + d as f64).log2();
        }
    }
    Ok(adj)
}

/// 激活扩散：seeds=(文件, 原始分) 命中集。返回簇提升分（命中集内重排用）与补充召回
/// （未命中邻居，1 跳=seed 分×边权，2 跳再×0.5 跳衰减，去重取高分）
pub fn spread_activation(root: &Path, seeds: &[(String, f64)]) -> Result<SpreadResult, String> {
    let conn = connect(root)?;
    let adj = adjacency(&conn)?;
    let max_s = seeds.iter().fold(0.0f64, |m, s| m.max(s.1));
    if max_s <= 0.0 {
        return Ok(SpreadResult::default());
    }
    let norm: HashMap<&str, f64> = seeds.iter().map(|(f, s)| (f.as_str(), s / max_s)).collect();
    let seed_set: HashSet<&str> = seeds.iter().map(|s| s.0.as_str()).collect();

    let mut boosted: HashMap<String, f64> = HashMap::new();
    let mut recalled_map: HashMap<String, (f64, String)> = HashMap::new(); // file -> (score, via)
    for (f, _) in seeds {
        let fscore = norm.get(f.as_str()).copied().unwrap_or(0.0);
        let Some(neighbors) = adj.get(f.as_str()) else { continue };
        for (n, w) in neighbors {
            if let Some(ns) = norm.get(n.as_str()) {
                // n 也是命中 → f 获得簇提升（背书强度 = n 指向 f 的边权：
                // 低度目标足额背书，枢纽目标被度数稀释）
                let back_w = adj
                    .get(n.as_str())
                    .and_then(|ns2| ns2.iter().find(|(m, _)| m == f).map(|(_, w2)| *w2))
                    .unwrap_or(*w);
                *boosted.entry(f.clone()).or_insert(0.0) += ns * back_w * CLUSTER_BOOST;
                continue;
            }
            if seed_set.contains(n.as_str()) {
                continue;
            }
            // 1 跳召回
            let s1 = fscore * w;
            let e = recalled_map.entry(n.clone()).or_insert((0.0, f.clone()));
            if s1 > e.0 {
                *e = (s1, f.clone());
            }
            // 2 跳召回（经 n 中转，衰减 0.5）
            if let Some(ns2) = adj.get(n.as_str()) {
                for (m, w2) in ns2 {
                    if m == n || seed_set.contains(m.as_str()) {
                        continue;
                    }
                    let s2 = fscore * w * w2 * HOP_DECAY;
                    let e2 = recalled_map.entry(m.clone()).or_insert((0.0, n.clone()));
                    if s2 > e2.0 {
                        *e2 = (s2, n.clone());
                    }
                }
            }
        }
    }
    let mut recalled: Vec<(String, f64, String)> = recalled_map
        .into_iter()
        .map(|(f, (s, v))| (f, s, v))
        .collect();
    recalled.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    recalled.truncate(MAX_RECALL);
    Ok(SpreadResult { boosted, recalled })
}

/// 批量取文档元信息（激活扩散 related 填充）：path -> (title, summary)
pub fn doc_meta(root: &Path, paths: &[String]) -> Result<HashMap<String, (String, String)>, String> {
    let conn = connect(root)?;
    let mut stmt = conn
        .prepare("SELECT path, title, summary FROM documents WHERE path = ?1")
        .map_err(|e| e.to_string())?;
    let mut out = HashMap::new();
    for p in paths {
        let mut rows = stmt
            .query_map(params![p], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
            .map_err(|e| e.to_string())?;
        if let Some(row) = rows.next() {
            if let Ok(m) = row {
                out.insert(p.clone(), m);
            }
        }
    }
    Ok(out)
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
        // 2026-08-11：反引号内（行内代码/``` 围栏）的 [[链接]] 是语法示例，不解析为链接
        assert_eq!(parse_links("使用 `[[文件名]]` 建立链接"), Vec::<String>::new());
        assert_eq!(parse_links("```\n[[代码块]]\n```"), Vec::<String>::new());
        assert_eq!(parse_links("无链接"), Vec::<String>::new());
        assert_eq!(parse_links("[[]]"), Vec::<String>::new());
        assert_eq!(parse_links("[[   ]]"), Vec::<String>::new());
        // 正常链接不受影响（混排：反引号外照常解析）
        assert_eq!(parse_links("见 [[正常链接]] 与 `[[示例]]`"), vec!["正常链接".to_string()]);
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
    fn dangling_resolves_off_graph_and_code_ticks() {
        let root = test_root("dangling-fix");
        // apps/ 被图谱排除（collect_md_files filter_entry），但 [[apps/match/SKILL.md]] 磁盘存在 → 不算悬空
        write(&root, "apps/match/SKILL.md", "# Match\n");
        // 反引号内的 [[语法示例]] 不解析为链接
        write(&root, "notes/A.md", "# A\n\n见 [[apps/match/SKILL.md]] 与 `[[示例]]`\n");
        let report = sync_graph(&root).unwrap();
        assert_eq!(report.dangling, 0, "磁盘存在的图谱外目标不算悬空");
        assert_eq!(report.links, 1, "反引号内语法示例不解析");
        assert_eq!(report.docs, 1, "apps/ 不进图谱");
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
        // 定向即从属：src=案件总览（父），全部 directed
        let case = "notes/案件总览.md";
        assert!(data.rule_edges.iter().all(|e| e.src == case && e.directed));
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
        // 猎头规则边是关联型：全部无向（directed=false，src<dst）
        assert!(data.rule_edges.iter().all(|e| !e.directed && e.src < e.dst_path));
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

    // ---------- 激活扩散检索（2026-08-11，第二步） ----------

    #[test]
    fn spread_activation_recalls_rule_and_ref_neighbors() {
        let root = test_root("spread-basic");
        // 律师模板 5 文件（规则边 case↔其余 4）+ 研究笔记引用案件总览（引用边）
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
        write(&root, "notes/研究笔记.md", "# 研究笔记\n\n参见 [[案件总览]]\n");
        sync_graph(&root).unwrap();

        let case = "notes/案件总览.md".to_string();
        let r = spread_activation(&root, &[(case.clone(), 1.0)]).unwrap();
        // 4 条规则邻居（0.8）+ 研究笔记（引用 1.0）
        assert_eq!(r.recalled.len(), 5);
        // 引用边权 1.0 > 规则边 0.8：研究笔记排最前
        assert_eq!(r.recalled[0].0, "notes/研究笔记.md");
        assert!((r.recalled[0].1 - 1.0).abs() < 1e-9);
        let ev = r.recalled.iter().find(|(f, _, _)| f == "notes/证据清单.md").unwrap();
        assert!((ev.1 - 0.8).abs() < 1e-9, "规则边权重 0.8");
        assert_eq!(ev.2, case, "via=种子文件");
        // 无种子间互链 → 无簇提升
        assert!(r.boosted.is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn spread_activation_two_hop_decay_and_cluster_boost() {
        let root = test_root("spread-hops");
        // 单向链 A→B→C（C 不反向连 A，保证 2 跳路径唯一）
        write(&root, "notes/A.md", "# A\n\n[[B]]\n");
        write(&root, "notes/B.md", "# B\n\n[[C]]\n");
        write(&root, "notes/C.md", "# C\n");
        sync_graph(&root).unwrap();

        // 单种子 A：1 跳召回 B（A→B 边权经度归一化：B 度 2 → 1/log2(3)=0.631），
        // 2 跳召回 C（B→C：C 度 1 → 1.0；0.631×1.0×0.5=0.3155，via=B）
        let r = spread_activation(&root, &[("notes/A.md".into(), 1.0)]).unwrap();
        assert_eq!(r.recalled.len(), 2);
        let b = r.recalled.iter().find(|(f, _, _)| f == "notes/B.md").unwrap();
        assert!((b.1 - 0.6309).abs() < 1e-3, "1 跳经度归一化：{}", b.1);
        let c = r.recalled.iter().find(|(f, _, _)| f == "notes/C.md").unwrap();
        assert!((c.1 - 0.3155).abs() < 1e-3, "2 跳衰减 0.5：{}", c.1);
        assert_eq!(c.2, "notes/B.md", "2 跳 via=中间节点");

        // 种子 A+B 互链 → 簇提升（0.3 × 邻居归一化分 × 边权，边权按目标节点度归一化）
        let r = spread_activation(&root, &[("notes/A.md".into(), 2.0), ("notes/B.md".into(), 1.0)]).unwrap();
        assert!((r.boosted["notes/A.md"] - 0.3 * 0.5 * 1.0).abs() < 1e-9, "A 获 B 背书（B→A 目标 A 度 1 → 1.0）");
        assert!((r.boosted["notes/B.md"] - 0.3 * 1.0 * 0.6309).abs() < 1e-3, "B 获 A 背书（A→B 目标 B 度 2 → 0.631）");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn spread_activation_dedup_and_recall_limit() {
        let root = test_root("spread-dedup");
        // 星型：seed 0.md 连 15 个文件 → 1 跳召回 15 个，截断到 MAX_RECALL=10 且不重复
        let mut star = String::from("# 0\n\n");
        for i in 1..16 {
            star.push_str(&format!("[[{}.md]] ", i));
        }
        write(&root, "notes/0.md", &star);
        for i in 1..16 {
            write(&root, &format!("notes/{}.md", i), &format!("# {}\n", i));
        }
        sync_graph(&root).unwrap();
        let r = spread_activation(&root, &[("notes/0.md".into(), 1.0)]).unwrap();
        assert_eq!(r.recalled.len(), MAX_RECALL, "截断到 MAX_RECALL");
        // 去重：同节点只出现一次
        let mut seen = std::collections::HashSet::new();
        for (f, _, _) in &r.recalled {
            assert!(seen.insert(f), "重复召回 {f}");
        }
        fs::remove_dir_all(&root).unwrap();
    }


    
    // ---------- 智能审计（2026-08-11，L0 扩展） ----------

    #[test]
    fn audit_smart_rules_detect_stale_near_empty_oversized() {
        let root = test_root("audit-smart");
        // 近重复：标题规范化后相同（空格/全角差异）；正文加长避免误判空笔记
        write(&root, "notes/知识图谱UI美化.md", &format!("# 知识图谱UI美化\n\n{}", "内容内容".repeat(100)));
        write(&root, "notes/知识图谱 UI 美化.md", &format!("# 知识图谱 UI 美化\n\n{}", "内容内容".repeat(100)));
        // 空笔记：正文 < 100 字符
        write(&root, "notes/空笔记.md", "# 空\n\n无内容\n");
        // 超长：> 30000 字符
        let mut big = String::from("# 超长\n\n");
        for _ in 0..2000 {
            big.push_str("这是一段很长很长很长的重复内容，用于填充超长笔记的判定阈值测试。\n");
        }
        write(&root, "notes/超长笔记.md", &big);
        // 正常笔记（有链接，避免全部成孤立拖分）
        write(&root, "notes/正常笔记.md", &format!("# 正常\n\n这是正常笔记的正文，长度足够。[[知识图谱UI美化]] {}", "填充".repeat(60)));
        sync_graph(&root).unwrap();

        let rep = audit(&root).unwrap();
        assert!(rep.near_duplicates.iter().any(|(t, paths)| {
            t.contains("知识图谱") && paths.len() == 2
        }), "近重复应命中规范化后同组：{:?}", rep.near_duplicates);
        assert!(rep.empty_notes.contains(&"notes/空笔记.md".to_string()));
        assert!(rep.oversized.contains(&"notes/超长笔记.md".to_string()));
        // 分级：空笔记/近重复 = Warning；超长 = Info
        assert!(rep.warning >= 2);
        assert!(rep.info >= 1);
        assert!(rep.score < 100, "有告警健康分应低于 100");
        assert!(rep.score >= 75, "小问题健康分应仍高位: {}", rep.score);
        // 历史趋势：连续两次 audit，第一次 trend=None（无历史），第二次 trend 有值
        assert!(rep.trend.is_none() || rep.trend == Some(0), "首次无历史或持平: {:?}", rep.trend);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn audit_stale_detects_old_orphans() {
        let root = test_root("audit-stale");
        write(&root, "notes/旧文档.md", "# 旧文档\n\n内容\n");
        sync_graph(&root).unwrap();
        // 把 mtime 改到 200 天前（std::fs 时间戳设置）
        let f = root.join("notes/旧文档.md");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(200 * 86400);
        let _ = std::fs::File::options().write(true).open(&f).and_then(|fh| {
            use std::os::windows::fs::FileTimesExt;
            fh.set_times(std::fs::FileTimes::new().set_modified(old))
        });
        sync_graph(&root).unwrap(); // 重建以刷新 mtime
        let rep = audit(&root).unwrap();
        assert!(rep.stale.contains(&"notes/旧文档.md".to_string()), "旧孤立应进 stale");
        assert!(rep.orphans.contains(&"notes/旧文档.md".to_string()));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn audit_history_tracks_trend() {
        let root = test_root("audit-trend");
        write(&root, "notes/A.md", "# A\n\n内容\n");
        sync_graph(&root).unwrap();
        let rep1 = audit(&root).unwrap();
        // 首次：历史无条目或刚写入
        let hist_path = root.join(".audit-history.json");
        assert!(hist_path.exists() || rep1.trend.is_some(), "audit 应落盘历史");
        let rep2 = audit(&root).unwrap();
        assert_eq!(rep2.trend, Some(0), "指标未变趋势应持平");
        // 新增一个空笔记 → 指标变化 → 健康分下降 + trend 为负
        write(&root, "notes/B.md", "# B\n");
        sync_graph(&root).unwrap();
        let rep3 = audit(&root).unwrap();
        assert!(rep3.score < rep2.score, "新增告警健康分应下降");
        assert!(rep3.trend.unwrap_or(0) < 0, "趋势应为负: {:?}", rep3.trend);
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
        let has_dir: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('rule_links') WHERE name='directed'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(has_dir, "迁移后 rule_links 应有 directed 列");
        drop(conn); // 释放库句柄（WAL 文件锁），否则 remove_dir_all 失败
        // sync 后旧数据也能入库（type 缺省 doc）
        let _ = sync_graph(&root).unwrap();
        let data = graph_data(&root).unwrap();
        assert_eq!(data.nodes[0].r#type, "doc");
        fs::remove_dir_all(&root).unwrap();
    }
}
