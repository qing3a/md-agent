//! 双层知识库布局与元数据：
//! - L1（kb 根目录）：规范 / 索引 / 记忆层（CLAUDE.md 模式，启动时注入）
//! - L2（kb/notes/）：内容层（按需检索）

use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// L1 常驻文件（每次会话注入上下文）
pub const L1_FILES: [&str; 4] = ["KB.md", "FRAMEWORK.md", "RULES.md", "MEMORY.md"];
/// L1 索引文件（扫描 L2 自动生成，勿手改）
pub const INDEX_FILE: &str = "INDEX.md";
/// L2 内容层目录
pub const NOTES_DIR: &str = "notes";

/// 解析 KB 根目录：env MD_AGENT_KB > 工作目录 ./kb > 可执行文件旁 kb
pub fn kb_root() -> PathBuf {
    if let Ok(p) = std::env::var("MD_AGENT_KB") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    if Path::new("kb").is_dir() {
        return PathBuf::from("kb");
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    exe_dir.join("kb")
}

/// 确保双层目录与 L1 文件存在；缺失时从内嵌模板写入（首次运行自举）
pub fn ensure_layout(root: &Path) -> std::io::Result<()> {
    fs::create_dir_all(root.join(NOTES_DIR))?;
    let templates: &[(&str, &str)] = &[
        ("KB.md", include_str!("templates/KB.md")),
        ("FRAMEWORK.md", include_str!("templates/FRAMEWORK.md")),
        ("RULES.md", include_str!("templates/RULES.md")),
        ("MEMORY.md", include_str!("templates/MEMORY.md")),
    ];
    for (name, content) in templates {
        let p = root.join(name);
        if !p.exists() {
            fs::write(p, content)?;
        }
    }
    let idx = root.join(INDEX_FILE);
    if !idx.exists() {
        fs::write(
            &idx,
            "# INDEX\n\n<!-- 本文件由 `POST /api/kb/sync` 自动生成，请勿手改。 -->\n\n(尚未同步)\n",
        )?;
    }
    Ok(())
}

/// 极简 frontmatter 解析：开头 `---` 与闭合 `---` 之间的 `key: value` 行
/// 返回 (元数据, 正文)
pub fn parse_frontmatter(content: &str) -> (BTreeMap<String, String>, &str) {
    let mut meta = BTreeMap::new();
    let t = content.trim_start();
    if !t.starts_with("---") {
        return (meta, content);
    }
    let after_open = &t[3..];
    let after_open = after_open
        .strip_prefix('\n')
        .or_else(|| after_open.strip_prefix("\r\n"))
        .unwrap_or(after_open);

    let mut offset = 0usize;
    let mut closed = false;
    for line in after_open.split_inclusive('\n') {
        let line_trim = line.trim_end().trim_end_matches('\r');
        if line_trim == "---" {
            closed = true;
            offset += line.len();
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            meta.insert(
                k.trim().to_string(),
                v.trim().trim_matches('"').trim().to_string(),
            );
        }
        offset += line.len();
    }
    if !closed {
        return (BTreeMap::new(), content);
    }
    (meta, &after_open[offset..])
}

/// 正文第一条标题（`#` 开头）
pub fn first_heading(body: &str) -> Option<String> {
    body.lines()
        .map(|l| l.trim())
        .find(|l| l.starts_with('#'))
        .map(|l| l.trim_start_matches('#').trim().to_string())
}

/// 正文首个有效行的摘要（跳过空行 / 标题 / 代码围栏）
pub fn summary(body: &str, max_chars: usize) -> String {
    body.lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("```"))
        .unwrap_or("")
        .chars()
        .take(max_chars)
        .collect()
}

#[derive(Debug, Serialize)]
pub struct SyncReport {
    pub index_path: String,
    pub files: usize,
}

/// 扫描 L2（notes/）→ 重建 INDEX.md（自动索引，解决索引腐化问题）
pub fn sync_index(root: &Path) -> std::io::Result<SyncReport> {
    let root = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf());
    let notes = root.join(NOTES_DIR);
    if !notes.is_dir() {
        fs::create_dir_all(&notes)?;
    }

    let mut rows: Vec<(String, String, String, String)> = Vec::new(); // 路径/标题/标签/摘要
    let walker = ignore::WalkBuilder::new(&notes)
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
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(content) = fs::read_to_string(path) else { continue };
        let (meta, body) = parse_frontmatter(&content);
        let title = meta
            .get("title")
            .cloned()
            .or_else(|| first_heading(body))
            .unwrap_or_default();
        let tags = meta.get("tags").cloned().unwrap_or_default();
        let summ = summary(body, 80);
        rows.push((rel, title, tags, summ));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let esc = |s: &str| s.replace('|', "\\|");
    let mut out = String::new();
    out.push_str("---\ntype: index\ntitle: 内容索引\nupdated: ");
    out.push_str(&now.to_string());
    out.push_str("\n---\n\n# 内容索引（MOC）\n\n> 由 `POST /api/kb/sync` 自动生成，请勿手改。\n\n| 路径 | 标题 | 标签 | 摘要 |\n|---|---|---|---|\n");
    for (rel, title, tags, summ) in &rows {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            esc(rel),
            esc(title),
            esc(tags),
            esc(summ)
        ));
    }
    out.push_str(&format!("\n共 {} 篇\n", rows.len()));

    fs::write(root.join(INDEX_FILE), out)?;
    Ok(SyncReport {
        index_path: INDEX_FILE.to_string(),
        files: rows.len(),
    })
}

#[derive(Debug)]
pub struct L1File {
    pub name: String,
    pub path: String,
    pub head: String,
    /// full=true 时附带完整内容（Prompt 注入用）
    pub content: String,
}

/// 列出 L1 层文件；full=true 时附带完整正文
pub fn list_l1(root: &Path, full: bool) -> Vec<L1File> {
    let mut out = Vec::new();
    for name in L1_FILES.iter().chain(std::iter::once(&INDEX_FILE)) {
        let p = root.join(name);
        if !p.exists() {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&p) {
            let head = content.lines().take(20).collect::<Vec<_>>().join("\n");
            out.push(L1File {
                name: name.to_string(),
                path: name.to_string(),
                head,
                content: if full { content } else { String::new() },
            });
        }
    }
    out
}

/// 路径安全校验：目标（或其最近已存在祖先）必须在 KB 根内。
/// 读/写共用：文件不存在时沿父链上溯到 KB 根，因此新建文件也能通过。
pub fn resolve_in_kb(root: &Path, rel: &str) -> Option<PathBuf> {
    let p = root.join(rel);
    let root_canon = root.canonicalize().ok()?;
    // 沿父链上溯到最近已存在祖先并规范化，校验其落在 KB 根内
    let mut cur: &Path = p.as_path();
    loop {
        if cur.exists() {
            let anc_canon = cur.canonicalize().ok()?;
            return if anc_canon.starts_with(&root_canon) {
                Some(p)
            } else {
                None
            };
        }
        cur = cur.parent()?;
    }
}
