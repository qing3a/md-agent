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

// ---------- 待审机制（Phase 3 前置：生成 → 预览 → 确认） ----------

#[derive(Debug, Serialize)]
pub struct PendingItem {
    /// 相对 kb 根，含 pending/ 前缀
    pub path: String,
    /// note（新笔记）| memory（记忆条目）
    pub kind: String,
    pub title: String,
}

/// 列出待审文件：pending/ 下的 .md；文件名 MEMORY.* 前缀 = 记忆条目，其余 = 新笔记
pub fn list_pending(root: &Path) -> Vec<PendingItem> {
    let dir = root.join("pending");
    let mut out = Vec::new();
    if !dir.is_dir() {
        return out;
    }
    let walker = ignore::WalkBuilder::new(&dir)
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
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let kind = if name.starts_with("MEMORY.") { "memory" } else { "note" };
        let content = fs::read_to_string(path).unwrap_or_default();
        let (meta, body) = parse_frontmatter(&content);
        let title = meta
            .get("title")
            .cloned()
            .or_else(|| first_heading(body))
            .unwrap_or_default();
        out.push(PendingItem {
            path: rel,
            kind: kind.to_string(),
            title,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// 批准待审：新笔记移动到目标路径；记忆条目合并进 MEMORY.md。
/// 返回 (落地路径, 备注)。成功后由调用方重建 INDEX 与图谱。
pub fn approve_pending(root: &Path, rel: &str, edited: Option<&str>) -> Result<(String, Option<String>), String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let pending_dir = root.join("pending");
    let src = root.join(rel);
    let src_canon = src.canonicalize().map_err(|e| format!("待审文件不存在: {e}"))?;
    if !src_canon.starts_with(&pending_dir) {
        return Err("路径超出待审目录".to_string());
    }
    if !src_canon.is_file() {
        return Err("不是文件".to_string());
    }

    let stripped = rel
        .strip_prefix("pending/")
        .unwrap_or(rel)
        .to_string();
    if stripped.starts_with("MEMORY.") {
        // 记忆条目 → 合并进 MEMORY.md（edited 覆盖原内容，支持「编辑后批准」）
        let content = match edited {
            Some(c) => c.to_string(),
            None => fs::read_to_string(&src_canon).map_err(|e| e.to_string())?,
        };
        append_memory_entry(&root, &content)?;
        fs::remove_file(&src_canon).map_err(|e| e.to_string())?;
        Ok(("MEMORY.md".to_string(), Some("记忆条目已按当日小节合并".to_string())))
    } else {
        // 新笔记 → 移动到目标路径（保留 pending/ 后的相对结构）；edited 覆盖内容
        let dst = root.join(&stripped);
        if let Some(edited) = edited {
            fs::write(&dst, edited).map_err(|e| format!("写入失败: {e}"))?;
            fs::remove_file(&src_canon).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::rename(&src_canon, &dst).map_err(|e| format!("移动失败: {e}"))?;
        }
        Ok((stripped, None))
    }
}

/// 拒绝待审：删除；rel 支持 "all" 批量。返回删除数量。
pub fn reject_pending(root: &Path, rel: &str) -> Result<usize, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let pending_dir = root.join("pending");
    let mut removed = 0usize;
    if rel == "all" {
        for item in list_pending(&root) {
            let p = root.join(&item.path);
            if p.is_file() {
                let _ = fs::remove_file(&p);
                removed += 1;
            }
        }
        return Ok(removed);
    }
    let src = root.join(rel);
    let src_canon = src.canonicalize().map_err(|e| format!("待审文件不存在: {e}"))?;
    if !src_canon.starts_with(&pending_dir) {
        return Err("路径超出待审目录".to_string());
    }
    fs::remove_file(&src_canon).map_err(|e| e.to_string())?;
    Ok(1)
}

/// 计算记忆条目将追加的文本（行级 diff 预览与落盘共用）
fn memory_added_text(old: &str, content: &str) -> String {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let entry = content.trim();
    let strip = |s: &str| -> String { s.trim_start_matches(['-', '#', ' ']).trim().to_string() };
    if entry.starts_with("## ") {
        format!("\n\n{entry}\n")
    } else if old.contains(&format!("## {today}")) {
        format!("\n- {}\n", strip(entry))
    } else if old.trim().is_empty() {
        format!("# 记忆\n\n## {today}\n- {}\n", strip(entry))
    } else {
        format!("\n\n## {today}\n- {}\n", strip(entry))
    }
}

/// 记忆条目合并进 MEMORY.md（按当日小节；自带 ## 标题的原样追加）
fn append_memory_entry(root: &Path, content: &str) -> Result<(), String> {
    let mem = root.join("MEMORY.md");
    let old = fs::read_to_string(&mem).unwrap_or_default();
    let added = memory_added_text(&old, content);
    fs::write(mem, format!("{}{}", old.trim_end(), added)).map_err(|e| e.to_string())
}

#[derive(Debug, Serialize)]
pub struct PendingPreview {
    pub path: String,
    /// 落地目标（MEMORY.md 或 notes/xxx.md）
    pub target: String,
    pub kind: String,
    /// 将新增的内容（memory=追加行；note=整篇）
    pub added: String,
}

/// 行级预览：确认待审文件批准后"将写入什么"（不落盘）
pub fn preview_pending(root: &Path, rel: &str) -> Result<PendingPreview, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let pending_dir = root.join("pending");
    let src = root.join(rel);
    let src_canon = src.canonicalize().map_err(|e| format!("待审文件不存在: {e}"))?;
    if !src_canon.starts_with(&pending_dir) {
        return Err("路径超出待审目录".to_string());
    }
    let content = fs::read_to_string(&src_canon).map_err(|e| e.to_string())?;
    let stripped = rel.strip_prefix("pending/").unwrap_or(rel).to_string();
    if stripped.starts_with("MEMORY.") {
        let old = fs::read_to_string(root.join("MEMORY.md")).unwrap_or_default();
        let added = memory_added_text(&old, &content);
        Ok(PendingPreview {
            path: rel.to_string(),
            target: "MEMORY.md".to_string(),
            kind: "memory".to_string(),
            added,
        })
    } else {
        Ok(PendingPreview {
            path: rel.to_string(),
            target: stripped,
            kind: "note".to_string(),
            added: content,
        })
    }
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
