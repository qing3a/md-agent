//! 双层知识库布局与元数据：
//! - L1（kb 根目录）：规范 / 索引 / 记忆层（CLAUDE.md 模式，启动时注入）
//! - L2（kb/notes/）：内容层（按需检索）

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// L1 常驻文件（每次会话注入上下文）
pub const L1_FILES: [&str; 4] = ["KB.md", "FRAMEWORK.md", "RULES.md", "MEMORY.md"];
/// L1 索引文件（扫描 L2 自动生成，勿手改）
pub const INDEX_FILE: &str = "INDEX.md";
/// L2 内容层目录
pub const NOTES_DIR: &str = "notes";
/// 技能目录（Phase 3-C Step 2：程序性记忆，注册表 INDEX.md 自动生成）
pub const SKILLS_DIR: &str = "skills";
/// 应用目录（应用市场阶段 1：kb/apps/<id>/app.json = 每个应用的 manifest）
pub const APPS_DIR: &str = "apps";

/// 已安装应用（应用市场阶段 1）：来自 kb/apps/<id>/app.json
#[derive(Debug, Serialize)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub entry: String,
    pub permissions: Vec<String>,
    pub description: String,
}

/// 列出已安装应用：扫描 kb/apps/*/app.json（每个子目录 = 一个 app）
pub fn list_apps(root: &Path) -> Vec<AppInfo> {
    let apps_dir = root.join(APPS_DIR);
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(&apps_dir) else { return out };
    for e in rd.flatten() {
        if !e.path().is_dir() {
            continue;
        }
        let dir_name = e.file_name().to_string_lossy().to_string();
        let mf = e.path().join("app.json");
        let Ok(content) = fs::read_to_string(&mf) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&content) else { continue };
        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or(&dir_name).to_string();
        let name = v.get("name").and_then(|x| x.as_str()).unwrap_or(&dir_name).to_string();
        let version = v.get("version").and_then(|x| x.as_str()).unwrap_or("0.0.0").to_string();
        let entry = v.get("entry").and_then(|x| x.as_str()).unwrap_or("index.html").to_string();
        let description = v.get("description").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let permissions = v
            .get("permissions")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|p| p.as_str().map(String::from)).collect())
            .unwrap_or_default();
        out.push(AppInfo { id, name, version, entry, permissions, description });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

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
    fs::create_dir_all(root.join(SKILLS_DIR))?;
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

/// 生成 memory_summary.md（派生产物，如 INDEX.md：自动生成、无人审、可随时重建）。
/// 从 MEMORY.md 提取小节标题 + 关键 bullet（截断 100 字符），取最新 40 条 bullet；
/// 源文件 MEMORY.md 不受影响——人审只守源文件变更（如巩固提案），不守派生投影。
pub fn sync_memory_summary(root: &Path) -> std::io::Result<usize> {
    let mem = root.join("MEMORY.md");
    if !mem.is_file() {
        return Ok(0);
    }
    let content = fs::read_to_string(&mem)?;
    // 收集小节（heading → bullets）
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with("## ") || (t.starts_with("# ") && !t.starts_with("## ")) {
            sections.push((line.to_string(), Vec::new()));
        } else if (t.starts_with("- ") || t.starts_with("* ")) && !sections.is_empty() {
            let b = t.trim_start_matches(['-', '*', ' ']);
            let b: String = if b.chars().count() > 100 {
                b.chars().take(100).collect::<String>() + "…"
            } else {
                b.to_string()
            };
            sections.last_mut().unwrap().1.push(b);
        }
    }
    // 取最新 40 条 bullet（从尾部累计）
    let mut total = 0usize;
    let mut keep_from = sections.len();
    for (i, (_, bs)) in sections.iter().enumerate().rev() {
        total += bs.len();
        if total > 40 {
            break;
        }
        keep_from = i;
    }
    let mut out = String::from("# 记忆摘要（自动生成，勿手改；正文以 MEMORY.md 为准）\n\n");
    let mut bullets = 0usize;
    for (i, (h, bs)) in sections.iter().enumerate() {
        if i < keep_from {
            continue;
        }
        out.push_str(h);
        out.push('\n');
        for b in bs {
            out.push_str("  - ");
            out.push_str(b);
            out.push('\n');
            bullets += 1;
        }
    }
    if bullets == 0 {
        return Ok(0); // MEMORY 为空或格式不符，不生成空摘要
    }
    fs::write(root.join("memory_summary.md"), out)?;
    Ok(bullets)
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
    let _ = sync_memory_summary(&root); // 派生产物随 INDEX 一起刷新（/sync、心跳、approve、link 均经此）
    Ok(SyncReport {
        index_path: INDEX_FILE.to_string(),
        files: rows.len(),
    })
}

/// 重建技能注册表 skills/INDEX.md（扫描 skills/ 下 .md，排除 INDEX.md；frontmatter title/trigger/desc）
pub fn sync_skills(root: &Path) -> std::io::Result<usize> {
    let skills = root.join(SKILLS_DIR);
    fs::create_dir_all(&skills)?;
    let mut items: Vec<(String, String, String)> = Vec::new(); // 标题/触发词/描述
    if let Ok(rd) = fs::read_dir(&skills) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() || p.file_name().and_then(|n| n.to_str()) == Some("INDEX.md") {
                continue;
            }
            if p.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let content = fs::read_to_string(&p).unwrap_or_default();
            let (meta, body) = parse_frontmatter(&content);
            let title = meta
                .get("title")
                .cloned()
                .or_else(|| first_heading(body))
                .or_else(|| p.file_stem().and_then(|s| s.to_str()).map(String::from))
                .unwrap_or_default();
            let trigger = meta.get("trigger").cloned().unwrap_or_default();
            let desc = meta.get("desc").cloned().unwrap_or_default();
            items.push((title, trigger, desc));
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let mut out = String::new();
    out.push_str("---\ntype: index\ntitle: 技能注册表\nupdated: ");
    out.push_str(&now.to_string());
    out.push_str("\n---\n\n# 技能注册表（自动生成，勿手改）\n\n");
    if items.is_empty() {
        out.push_str("（暂无技能。Agent 生成的技能提案经 /approve 后安装于此。）\n");
    }
    for (title, trigger, desc) in &items {
        out.push_str(&format!(
            "- **{}**{}：{}\n",
            title,
            if trigger.is_empty() { String::new() } else { format!("（触发词 `{}`）", trigger) },
            if desc.is_empty() { "(无描述)" } else { desc }
        ));
    }
    out.push_str(&format!("\n共 {} 项\n", items.len()));
    fs::write(skills.join("INDEX.md"), out)?;
    Ok(items.len())
}

/// 技能条目（/api/skills 用，trigger 触发注入）
#[derive(Serialize, Debug)]
pub struct SkillInfo {
    pub name: String,
    pub title: String,
    pub trigger: String,
    pub desc: String,
}

/// 列出技能注册表条目（排除 INDEX.md）
pub fn list_skills(root: &Path) -> Vec<SkillInfo> {
    let skills = root.join(SKILLS_DIR);
    let mut out: Vec<SkillInfo> = Vec::new();
    if let Ok(rd) = fs::read_dir(&skills) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() || p.file_name().and_then(|n| n.to_str()) == Some("INDEX.md") {
                continue;
            }
            if p.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let content = fs::read_to_string(&p).unwrap_or_default();
            let (meta, body) = parse_frontmatter(&content);
            out.push(SkillInfo {
                name: p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(),
                title: meta.get("title").cloned()
                    .or_else(|| first_heading(body))
                    .or_else(|| p.file_stem().and_then(|s| s.to_str()).map(String::from))
                    .unwrap_or_default(),
                trigger: meta.get("trigger").cloned().unwrap_or_default(),
                desc: meta.get("desc").cloned().unwrap_or_default(),
            });
        }
    }
    out.sort_by(|a, b| a.title.cmp(&b.title));
    out
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
        // 待审四型：无前缀=新笔记；MEMORY.*=记忆；SKILL.*=技能；CONSOLIDATE.*=巩固（替换目标文件）
        let kind = if name.starts_with("MEMORY.") { "memory" }
            else if name.starts_with("SKILL.") { "skill" }
            else if name.starts_with("CONSOLIDATE.") { "consolidate" }
            else { "note" };
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
    } else if stripped.starts_with("SKILL.") {
        // 技能提案 → 移入 skills/（SKILL. 前缀去掉）+ 重建技能注册表
        let name = stripped.strip_prefix("SKILL.").unwrap_or(&stripped);
        let dst = root.join(SKILLS_DIR).join(name);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if let Some(edited) = edited {
            fs::write(&dst, edited).map_err(|e| format!("写入失败: {e}"))?;
            fs::remove_file(&src_canon).map_err(|e| e.to_string())?;
        } else {
            fs::rename(&src_canon, &dst).map_err(|e| format!("移动失败: {e}"))?;
        }
        let count = sync_skills(&root).map_err(|e| format!("技能注册表重建失败: {e}"))?;
        let rel = format!("{SKILLS_DIR}/{name}");
        Ok((rel, Some(format!("技能已安装（注册表 {count} 项）"))))
    } else if stripped.starts_with("CONSOLIDATE.") {
        // 巩固提案 → frontmatter {target} 指向目标文件，正文=替换后全文（edited 覆盖）
        let content = match edited {
            Some(c) => c.to_string(),
            None => fs::read_to_string(&src_canon).map_err(|e| e.to_string())?,
        };
        let (meta, body) = parse_frontmatter(&content);
        let target = meta.get("target").ok_or("巩固提案缺 frontmatter target 字段")?;
        if target.contains("..") || target.starts_with('/') {
            return Err("巩固目标路径不合法".to_string());
        }
        let dst = root.join(target);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&dst, body).map_err(|e| format!("写入目标失败: {e}"))?;
        fs::remove_file(&src_canon).map_err(|e| e.to_string())?;
        Ok((target.to_string(), Some("巩固提案已替换目标文件".to_string())))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 建隔离测试 kb：返回临时根目录（测试结束自动清理）
    fn test_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("md-agent-ut-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn parse_frontmatter_ok() {
        let (meta, body) = parse_frontmatter("---\ntitle: T\ntrigger: X\n---\n\n正文");
        assert_eq!(meta.get("title").map(String::as_str), Some("T"));
        assert_eq!(meta.get("trigger").map(String::as_str), Some("X"));
        assert_eq!(body.trim(), "正文");
    }

    #[test]
    fn ensure_layout_creates_skills() {
        let root = test_root("layout");
        ensure_layout(&root).unwrap();
        assert!(root.join("notes").is_dir());
        assert!(root.join("skills").is_dir());
        assert!(root.join("MEMORY.md").is_file());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn kind_inference_four_types() {
        let root = test_root("kind");
        ensure_layout(&root).unwrap();
        write(&root, "pending/新笔记.md", "# N\n");
        write(&root, "pending/MEMORY.t.md", "## 2026-08-03\n- x\n");
        write(&root, "pending/SKILL.技能.md", "---\ntitle: S\ntrigger: T\n---\n# S\n");
        write(&root, "pending/CONSOLIDATE.c.md", "---\ntarget: MEMORY.md\n---\n正文\n");
        let kinds: Vec<String> = list_pending(&root).into_iter().map(|p| p.kind).collect();
        assert!(kinds.contains(&"note".to_string()));
        assert!(kinds.contains(&"memory".to_string()));
        assert!(kinds.contains(&"skill".to_string()));
        assert!(kinds.contains(&"consolidate".to_string()));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn approve_skill_installs_and_indexes() {
        let root = test_root("skill");
        ensure_layout(&root).unwrap();
        write(&root, "pending/SKILL.整理.md",
            "---\ntype: skill\ntitle: 整理\ntrigger: 整理\n---\n# 整理\n步骤\n");
        let (target, note) = approve_pending(&root, "pending/SKILL.整理.md", None).unwrap();
        assert_eq!(target, "skills/整理.md");
        assert!(root.join("skills/整理.md").is_file());
        let idx = fs::read_to_string(root.join("skills/INDEX.md")).unwrap();
        assert!(idx.contains("整理"));
        assert!(note.unwrap().contains("注册表"));
        // list_skills 应列出（trigger 命中）
        let sk = list_skills(&root);
        assert_eq!(sk.len(), 1);
        assert_eq!(sk[0].trigger, "整理");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn approve_consolidate_replaces_target() {
        let root = test_root("consol");
        ensure_layout(&root).unwrap();
        fs::write(root.join("MEMORY.md"), "# M\n- 旧内容\n").unwrap();
        write(&root, "pending/CONSOLIDATE.c.md",
            "---\ntype: consolidate\ntarget: MEMORY.md\n---\n# M\n- 新内容\n");
        let (target, _) = approve_pending(&root, "pending/CONSOLIDATE.c.md", None).unwrap();
        assert_eq!(target, "MEMORY.md");
        let mem = fs::read_to_string(root.join("MEMORY.md")).unwrap();
        assert!(mem.contains("新内容") && !mem.contains("旧内容"));
        assert!(!root.join("pending/CONSOLIDATE.c.md").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn approve_note_memory_regression() {
        let root = test_root("regr");
        ensure_layout(&root).unwrap();
        // note
        write(&root, "pending/notes/新.md", "# 新\n正文\n");
        let (t1, _) = approve_pending(&root, "pending/notes/新.md", None).unwrap();
        assert_eq!(t1, "notes/新.md");
        assert!(root.join("notes/新.md").is_file());
        // memory
        write(&root, "pending/MEMORY.t.md", "## 2026-08-03\n- 条目\n");
        let (t2, _) = approve_pending(&root, "pending/MEMORY.t.md", None).unwrap();
        assert_eq!(t2, "MEMORY.md");
        assert!(fs::read_to_string(root.join("MEMORY.md")).unwrap().contains("条目"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn consolidate_target_path_guard() {
        let root = test_root("guard");
        ensure_layout(&root).unwrap();
        write(&root, "pending/CONSOLIDATE.bad.md",
            "---\ntarget: ../../evil.md\n---\n正文\n");
        let r = approve_pending(&root, "pending/CONSOLIDATE.bad.md", None);
        assert!(r.is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn memory_summary_derived_no_review() {
        let root = test_root("msum");
        ensure_layout(&root).unwrap();
        fs::write(root.join("MEMORY.md"), "# 记忆\n\n## 2026-08-01\n- 旧决策甲\n\n## 2026-08-03\n- 新决策丙\n").unwrap();
        let n = sync_memory_summary(&root).unwrap();
        assert!(n >= 2);
        let s = fs::read_to_string(root.join("memory_summary.md")).unwrap();
        assert!(s.contains("新决策丙") && s.contains("2026-08-03"));
        assert!(s.contains("旧决策甲")); // ≤40 条全保留
        // 源文件未被修改（派生产物不动源，无需人审）
        let mem = fs::read_to_string(root.join("MEMORY.md")).unwrap();
        assert!(mem.contains("旧决策甲"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn memory_summary_caps_at_40() {
        let root = test_root("msum40");
        ensure_layout(&root).unwrap();
        let mut mem = String::from("# 记忆\n");
        for i in 0..60 {
            mem.push_str(&format!("## 2026-08-{:02}\n- 决策{:02}\n", (i % 28) + 1, i));
        }
        fs::write(root.join("MEMORY.md"), mem).unwrap();
        let n = sync_memory_summary(&root).unwrap();
        assert!(n <= 40); // 截断到 40 条内
        let s = fs::read_to_string(root.join("memory_summary.md")).unwrap();
        assert!(s.contains("决策59")); // 最新保留
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn list_apps_parses_manifest() {
        let root = test_root("apps");
        ensure_layout(&root).unwrap();
        write(&root, "apps/match/app.json",
            r#"{"id":"match","name":"相亲评估工作台","version":"0.2.0","entry":"index.html","permissions":["llm"],"description":"d"}"#);
        let apps = list_apps(&root);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].id, "match");
        assert_eq!(apps[0].permissions, vec!["llm".to_string()]);
        assert_eq!(apps[0].entry, "index.html");
        fs::remove_dir_all(&root).unwrap();
    }
}
