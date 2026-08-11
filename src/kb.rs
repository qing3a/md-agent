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
/// L1 可读白名单（read_l1 端点）：规范/记忆/索引层 = L1_FILES + INDEX + 记忆摘要。
/// memory_summary.md 是派生产物（摘要提示，允许读取），不替代 MEMORY.md 原文。
pub const L1_READABLE_FILES: [&str; 6] = [
    "KB.md",
    "FRAMEWORK.md",
    "RULES.md",
    "MEMORY.md",
    INDEX_FILE,
    "memory_summary.md",
];

/// 已安装应用（应用市场阶段 1）：来自 kb/apps/<id>/app.json
#[derive(Debug, Serialize)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub entry: String,
    pub permissions: Vec<String>,
    pub description: String,
    /// 来源 hub 名（SkillHub 目录安装时记录；本地导入为 None）
    pub source_hub: Option<String>,
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
        let source_hub = v.get("source_hub").and_then(|x| x.as_str()).map(String::from);
        out.push(AppInfo { id, name, version, entry, permissions, description, source_hub });
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

/// read_l1 结果（上下文组装 v2：LLM 显式工具取用 L1 规范/记忆/索引层）
#[derive(Debug, Serialize)]
pub struct L1ReadResult {
    pub ok: bool,
    pub file: String,
    /// head=文件头（前 max 字）+ 章节清单 | section=命中 ## 小节原文 | section_list=未命中，给章节清单
    pub mode: String,
    /// 文件全文字符数（截断前）
    pub total_chars: usize,
    /// head=前 max 字；section=小节原文（含 ## 标题，截到 max）；section_list=空串
    pub content: String,
    /// `## ` 小节标题清单（三种 mode 均附）
    pub sections: Vec<String>,
}

/// read_l1：读 L1 规范/记忆/索引层原文（返回源文件原文，非派生产物；memory_summary 例外允许）。
/// 切分单位 = 文件 / `##` 小节（记忆统一模型铁律 1，不发明新元数据）。
/// - file 白名单外 → Err（调用方返回 400）
/// - file + 无 q → 文件头（前 max 字）+ `##` 章节清单，mode=head
/// - file + q → 定位第一个「标题或正文」含 q 的 `##` 小节（到下一个 `##` 前，截到 max），mode=section
/// - 未命中 → 章节清单，mode=section_list
pub fn read_l1(root: &Path, file: &str, q: Option<&str>, max_chars: usize) -> Result<L1ReadResult, String> {
    if !L1_READABLE_FILES.contains(&file) {
        return Err(format!("文件不在 L1 可读白名单: {file}"));
    }
    let content = fs::read_to_string(root.join(file)).map_err(|e| format!("读取 {file} 失败: {e}"))?;
    let total_chars = content.chars().count();
    let lines: Vec<&str> = content.lines().collect();

    // `## ` 小节标题（去掉前缀，标题本身作为索引）
    let sections: Vec<String> = lines
        .iter()
        .filter(|l| l.trim_start().starts_with("## "))
        .map(|l| l.trim_start().trim_start_matches("## ").trim().to_string())
        .collect();

    let trunc = |s: &str| s.chars().take(max_chars).collect::<String>();

    // file + q：定位第一个「标题或正文」含 q 的 ## 小节
    if let Some(qraw) = q {
        let q = qraw.trim();
        if !q.is_empty() {
            let ql = q.to_lowercase();
            let heading_idx: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| l.trim_start().starts_with("## "))
                .map(|(i, _)| i)
                .collect();
            for (k, &start) in heading_idx.iter().enumerate() {
                let end = heading_idx.get(k + 1).copied().unwrap_or(lines.len());
                let sec_raw = lines[start..end].join("\n");
                if sec_raw.to_lowercase().contains(&ql) {
                    return Ok(L1ReadResult {
                        ok: true,
                        file: file.to_string(),
                        mode: "section".to_string(),
                        total_chars,
                        content: trunc(&sec_raw),
                        sections,
                    });
                }
            }
            // 未命中 → 章节清单
            return Ok(L1ReadResult {
                ok: true,
                file: file.to_string(),
                mode: "section_list".to_string(),
                total_chars,
                content: String::new(),
                sections,
            });
        }
    }

    // file + 无 q → 文件头 + 章节清单
    Ok(L1ReadResult {
        ok: true,
        file: file.to_string(),
        mode: "head".to_string(),
        total_chars,
        content: trunc(&content),
        sections,
    })
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
/// 待审自动落地（2026-08-11 人审收敛：派生产物无人审，git 自动提交即回滚通道）。
/// 提案写入 pending 后立即落地——与 approve_pending 同一路由（MEMORY 合并/SKILL 移入/
/// CONSOLIDATE 替换/EXPERIENCE 落盘/note rename），失败返回 Err 由调用方埋点追溯。
pub fn auto_land(root: &Path, rel: &str) -> Result<(String, Option<String>), String> {
    approve_pending(root, rel, None)
}

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
        // 剥离前导提示行（2026-08-11：dream 提案正文带「> 后台巩固」说明行，替换目标时不得混入）
        fs::write(&dst, strip_leading_hints(body)).map_err(|e| format!("写入目标失败: {e}"))?;
        fs::remove_file(&src_canon).map_err(|e| e.to_string())?;
        Ok((target.to_string(), Some("巩固提案已替换目标文件".to_string())))
    } else if stripped.starts_with("DECISION.") {
        // 未决决策提案（B3）→ 明细落 notes/决策/未决.md + L1 MEMORY 决策待定指针（幂等）
        let content = match edited {
            Some(c) => c.to_string(),
            None => fs::read_to_string(&src_canon).map_err(|e| e.to_string())?,
        };
        let (meta, body) = parse_frontmatter(&content);
        let date = meta
            .get("date")
            .cloned()
            .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());
        // 标题行 = 第一个非空行（frontmatter 后可能带空行——前端写提案格式 `---\n\n## 议题`）
        let mut body_lines = body.lines().skip_while(|l| l.trim().is_empty());
        let first_line = body_lines
            .next()
            .unwrap_or("未决议题")
            .trim()
            .trim_start_matches("## ")
            .to_string();
        // 1) notes/决策/未决.md 追加明细（每议题一节；body 首行「## 议题」已并入日期头，跳过避免重复）
        let dec_dir = root.join("notes").join("决策");
        fs::create_dir_all(&dec_dir).map_err(|e| e.to_string())?;
        let dec_file = dec_dir.join("未决.md");
        let old_dec = fs::read_to_string(&dec_file).unwrap_or_default();
        let body_rest = body_lines.collect::<Vec<_>>().join("\n");
        let entry = format!("\n\n## [{date}] {first_line}\n{}\n", body_rest.trim());
        fs::write(&dec_file, format!("{}{}", old_dec.trim_end(), entry)).map_err(|e| e.to_string())?;
        // 2) L1 MEMORY 决策待定指针（已有小节则追加 bullet，否则新建小节）
        let mem_path = root.join("MEMORY.md");
        let old_mem = fs::read_to_string(&mem_path).unwrap_or_default();
        let bullet = format!("- [{date}] {first_line}（未决）：见 [[notes/决策/未决.md]]");
        if let Some(hdr) = old_mem.find("## 决策待定") {
            let after_hdr = &old_mem[hdr..];
            let nl = after_hdr.find('\n').map(|i| hdr + i + 1).unwrap_or(old_mem.len());
            let new_mem = format!("{}{}\n{}", &old_mem[..nl], bullet, &old_mem[nl..]);
            fs::write(&mem_path, new_mem).map_err(|e| e.to_string())?;
        } else {
            let new_mem = format!("{}{}\n\n## 决策待定\n{}\n", old_mem.trim_end(), "", bullet);
            fs::write(&mem_path, new_mem).map_err(|e| e.to_string())?;
        }
        fs::remove_file(&src_canon).map_err(|e| e.to_string())?;
        Ok((
            "notes/决策/未决.md".to_string(),
            Some("未决决策已登记（MEMORY 决策待定 + notes/决策/未决.md）".to_string()),
        ))
    } else if stripped.starts_with("EXPERIENCE.") {
        // 经验提案（C2 三分通道）：EXPERIENCE.MEMORY.* → 记忆；EXPERIENCE.BEHAVIOR.* → 行为建议；EXPERIENCE.CODE.* → 代码 backlog
        let content = match edited {
            Some(c) => c.to_string(),
            None => fs::read_to_string(&src_canon).map_err(|e| e.to_string())?,
        };
        let (_, body) = parse_frontmatter(&content);
        let sub = stripped
            .strip_prefix("EXPERIENCE.")
            .unwrap_or("BEHAVIOR")
            .split('.')
            .next()
            .unwrap_or("BEHAVIOR")
            .to_string();
        match sub.as_str() {
            "MEMORY" => {
                // 经验教训 → 合并进 MEMORY.md（可检索；body 为 # 开头，append_memory_entry 转 bullet 进当日小节）
                append_memory_entry(&root, &body).map_err(|e| e.to_string())?;
                fs::remove_file(&src_canon).map_err(|e| e.to_string())?;
                Ok(("MEMORY.md".to_string(), Some("经验已并入 MEMORY.md（可检索）".to_string())))
            }
            "CODE" => {
                // 代码类经验 → 落 backlog（占位，待 C3 验证管线处理）
                let name = stripped.strip_prefix("EXPERIENCE.CODE.").unwrap_or("proposal");
                let dst = root.join("notes").join("自组织").join("代码提案").join(name);
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                fs::write(&dst, &body).map_err(|e| format!("写入失败: {e}"))?;
                fs::remove_file(&src_canon).map_err(|e| e.to_string())?;
                Ok(("notes/自组织/代码提案/".to_string(), Some("代码类经验已入 backlog（待 C3 验证管线）".to_string())))
            }
            _ => {
                // 行为类（默认）→ 落行为建议记录
                let name = stripped
                    .strip_prefix("EXPERIENCE.")
                    .unwrap_or("proposal")
                    .strip_prefix("BEHAVIOR.")
                    .unwrap_or("proposal");
                let dst = root.join("notes").join("自组织").join("行为建议").join(name);
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                fs::write(&dst, &body).map_err(|e| format!("写入失败: {e}"))?;
                fs::remove_file(&src_canon).map_err(|e| e.to_string())?;
                Ok(("notes/自组织/行为建议/".to_string(), Some("行为类经验已落行为建议（可人工应用）".to_string())))
            }
        }
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

/// 前导提示行判断：空行或 "> " 引用块（LLM 提案的说明行，不进目标文件）
fn hint_line(l: &str) -> bool {
    let t = l.trim();
    t.is_empty() || t.starts_with('>')
}

/// 剥离正文前导提示行（"> " 引用块与空行）——auto_land 替换目标文件前使用，
/// 防「> 人工核对」类说明混入 MEMORY.md 等落盘内容
fn strip_leading_hints(content: &str) -> String {
    content
        .lines()
        .skip_while(|l| hint_line(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 计算记忆条目将追加的文本（行级 diff 预览与落盘共用）
fn memory_added_text(old: &str, content: &str) -> String {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    // 剥离前导提示行与提案 frontmatter（2026-08-11：auto_land 直读提案文件，
    // LLM 提案的 "> 说明" 引用块、frontmatter 块（--- 到 ---）与空行一律不进记忆，只沉淀正文）
    let lines: Vec<&str> = content.lines().collect();
    let skip_hints = |mut i: usize| {
        while i < lines.len() && hint_line(lines[i]) {
            i += 1;
        }
        i
    };
    let mut start = skip_hints(0);
    if start < lines.len() && lines[start].trim() == "---" {
        start += 1; // 越过开头 ---
        while start < lines.len() && lines[start].trim() != "---" {
            start += 1;
        }
        // start 指向闭合 ---（无闭合则停在末尾，正文为空）
        start = start.min(lines.len());
        start = skip_hints(start + 1).min(lines.len()); // 越过闭合 --- 并再跳提示行/空行
    }
    let entry = lines[start..].join("\n").trim().to_string();
    if entry.is_empty() {
        return String::new(); // 只有提示行/空 frontmatter 无可沉淀
    }
    let strip = |s: &str| -> String { s.trim_start_matches(['-', '#', ' ']).trim().to_string() };
    if entry.starts_with("## ") {
        format!("\n\n{entry}\n")
    } else if old.contains(&format!("## {today}")) {
        format!("\n- {}\n", strip(&entry))
    } else if old.trim().is_empty() {
        format!("# 记忆\n\n## {today}\n- {}\n", strip(&entry))
    } else {
        format!("\n\n## {today}\n- {}\n", strip(&entry))
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
    } else if stripped.starts_with("DECISION.") {
        // 未决决策提案（B3）：预览 = 将写入 notes/决策/未决.md 的明细
        let (_, body) = parse_frontmatter(&content);
        Ok(PendingPreview {
            path: rel.to_string(),
            target: "notes/决策/未决.md".to_string(),
            kind: "decision".to_string(),
            added: body.trim().to_string(),
        })
    } else if stripped.starts_with("code/") {
        // 代码提案（C3）：预览 = 提案摘要 + 修改文件清单（应用走 /dev apply）
        let (meta, body) = parse_frontmatter(&content);
        let reason = meta.get("reason").cloned().unwrap_or_default();
        let files = meta.get("files").cloned().unwrap_or_default();
        Ok(PendingPreview {
            path: rel.to_string(),
            target: "项目源码（/dev apply 应用）".to_string(),
            kind: "code-patch".to_string(),
            added: format!("代码提案：{reason}\n修改文件：{files}\n\n（/dev apply {rel} 应用 + cargo build 验证，失败自动回滚）\n\n{body}"),
        })
    } else if stripped.starts_with("EXPERIENCE.") {
        // 经验提案（C2）：预览 = 正文 + 目标按类型
        let (_, body) = parse_frontmatter(&content);
        let sub = stripped
            .strip_prefix("EXPERIENCE.")
            .unwrap_or("BEHAVIOR")
            .split('.')
            .next()
            .unwrap_or("BEHAVIOR");
        let target = match sub {
            "MEMORY" => "MEMORY.md".to_string(),
            "CODE" => "notes/自组织/代码提案/".to_string(),
            _ => "notes/自组织/行为建议/".to_string(),
        };
        Ok(PendingPreview {
            path: rel.to_string(),
            target,
            kind: "experience".to_string(),
            added: body.trim().to_string(),
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

/// 未决决策拍板（B3 闭环）：/decide <主题> <结论>
/// 从未决清单 notes/决策/未决.md 移除匹配议题小节（标题行模糊匹配），
/// 结论追加 notes/决策/已决.md；L1 MEMORY「决策待定」区移除对应行、决策区追加记录。
/// 幂等：未决清单无匹配但已决.md 已含同主题 → 提示已拍板不重复追加。
pub fn decide_undecided(root: &Path, topic: &str, conclusion: &str) -> Result<String, String> {
    let topic = topic.trim();
    let conclusion = conclusion.trim();
    if topic.is_empty() || conclusion.is_empty() {
        return Err("主题与结论不能为空".to_string());
    }
    let dec_dir = root.join("notes").join("决策");
    let dec_file = dec_dir.join("未决.md");
    // 幂等前置：清单缺失（可能已被拍板删档）→ 已决.md 含同主题则视为已拍板
    let idempotent_done = || -> Option<String> {
        let done_file = dec_dir.join("已决.md");
        let done = fs::read_to_string(&done_file).ok()?;
        if done.contains(topic) {
            Some("该议题已拍板（notes/决策/已决.md 已有记录）".to_string())
        } else {
            None
        }
    };
    let content = match fs::read_to_string(&dec_file) {
        Ok(c) => c,
        Err(_) => {
            if let Some(msg) = idempotent_done() {
                return Ok(msg);
            }
            return Err("未决清单不存在（notes/决策/未决.md）".to_string());
        }
    };

    // 行扫描切分：以 "## " 开头的行为新节首；第一个节之前的文字为文件头
    let mut head = String::new();
    let mut sections: Vec<String> = Vec::new();
    for line in content.lines() {
        if line.starts_with("## ") {
            sections.push(line.to_string());
        } else if sections.is_empty() {
            if !line.trim().is_empty() {
                head.push_str(line);
                head.push('\n');
            }
        } else {
            let last = sections.last_mut().unwrap();
            last.push('\n');
            last.push_str(line);
        }
    }
    let mut hit: Option<(String, String)> = None; // (标题行, 完整小节)
    let mut rest: Vec<String> = Vec::new();
    for s in &sections {
        let title = s.lines().next().unwrap_or("").trim();
        if hit.is_none() && title.contains(topic) {
            hit = Some((title.trim_start_matches("## ").trim().to_string(), s.clone()));
        } else {
            rest.push(s.clone());
        }
    }
    let Some((title, _)) = hit else {
        // 未命中 → 幂等检查：已决.md 已含同主题 → 视为已拍板
        if let Some(msg) = idempotent_done() {
            return Ok(msg);
        }
        return Err(format!("未决清单中无含「{topic}」的议题（查看 notes/决策/未决.md）"));
    };

    // 1) 未决.md：移除命中小节；无剩余小节 → 删文件（DECISION 批准路径重建兼容缺失）
    if rest.is_empty() {
        fs::remove_file(&dec_file).map_err(|e| format!("清理未决清单失败: {e}"))?;
    } else {
        let mut new_content = head;
        new_content.push_str(&rest.join("\n\n"));
        fs::write(&dec_file, new_content).map_err(|e| format!("更新未决清单失败: {e}"))?;
    }
    // 2) 已决.md 追加（标题行保留原议题 + 结论成节；title 已含 [date] 前缀）
    let done_file = dec_dir.join("已决.md");
    let old_done = fs::read_to_string(&done_file).unwrap_or_default();
    let entry = format!("## {title}\n结论：{conclusion}\n", title = title);
    let new_done = if old_done.trim().is_empty() {
        entry
    } else {
        format!("{}\n\n{}", old_done.trim_end(), entry)
    };
    fs::create_dir_all(&dec_dir).map_err(|e| e.to_string())?;
    fs::write(&done_file, new_done).map_err(|e| format!("写已决.md 失败: {e}"))?;
    // 3) L1 MEMORY：决策待定区移除该行 + 决策区追加记录
    let mem_path = root.join("MEMORY.md");
    let old_mem = fs::read_to_string(&mem_path).unwrap_or_default();
    let mut new_mem = String::new();
    let mut in_undecided = false;
    let mut removed_line = false;
    for line in old_mem.lines() {
        if line.starts_with("## 决策待定") {
            in_undecided = true;
        } else if line.starts_with("## ") {
            in_undecided = false;
        }
        let is_undecided_line = in_undecided && line.starts_with("- ") && line.contains("（未决）") && line.contains(topic);
        if is_undecided_line {
            removed_line = true;
            continue; // 移除该指针行
        }
        new_mem.push_str(line);
        new_mem.push('\n');
    }
    let bullet = format!("- {title}：{conclusion}");
    // 决策区标题匹配：排除「## 决策待定」前缀误命中（find 精确到换行）
    let dec_sec = new_mem.find("## 决策\n");
    if let Some(hdr) = dec_sec {
        // 决策区存在 → 首行后插入
        let nl = new_mem[hdr..].find('\n').map(|i| hdr + i + 1).unwrap_or(new_mem.len());
        new_mem = format!("{}{}\n{}", &new_mem[..nl], bullet, &new_mem[nl..]);
    } else {
        new_mem = format!("{}{}\n\n## 决策\n{}\n", new_mem.trim_end(), "", bullet);
    }
    fs::write(&mem_path, new_mem).map_err(|e| format!("写 MEMORY.md 失败: {e}"))?;
    let _ = removed_line;
    Ok(format!("已拍板：{title}（未决清单已移除，结论入 MEMORY 决策区）"))
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
    fn approve_consolidate_strips_hint_lines() {
        // 2026-08-11：dream 提案正文带「> 后台巩固」说明行 → 替换目标时剥离，不混入 MEMORY.md
        let root = test_root("consolhint");
        ensure_layout(&root).unwrap();
        fs::write(root.join("MEMORY.md"), "# M\n- 旧内容\n").unwrap();
        write(&root, "pending/CONSOLIDATE.DREAM-h.md",
            "---\ntype: consolidate\ntarget: MEMORY.md\n---\n\n> 后台巩固（LLM 生成，批准前请人工核对）。批准后替换 MEMORY.md。\n\n# 记忆\n- 新内容\n");
        approve_pending(&root, "pending/CONSOLIDATE.DREAM-h.md", None).unwrap();
        let mem = fs::read_to_string(root.join("MEMORY.md")).unwrap();
        assert!(!mem.contains("后台巩固"), "提示行不得混入目标: {mem}");
        assert!(mem.contains("新内容") && !mem.contains("旧内容"));
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
    fn approve_experience_routes_by_type() {
        let root = test_root("exp");
        ensure_layout(&root).unwrap();
        // MEMORY 型 → 并入 MEMORY.md
        write(&root, "pending/EXPERIENCE.MEMORY.abc.md",
            "---\ntype: experience\nsignal: correction\ndate: 2026-08-05\n---\n\n# 经验提案\n\n- 类型：memory\n- 问题：答错命令\n- 改进：记住 /rescan\n");
        let (t1, _) = approve_pending(&root, "pending/EXPERIENCE.MEMORY.abc.md", None).unwrap();
        assert_eq!(t1, "MEMORY.md");
        let mem = fs::read_to_string(root.join("MEMORY.md")).unwrap();
        assert!(mem.contains("/rescan") && mem.contains("答错命令"));
        // BEHAVIOR 型（默认）→ 落行为建议
        write(&root, "pending/EXPERIENCE.BEHAVIOR.def.md",
            "---\ntype: experience\nsignal: tool_failure\ndate: 2026-08-05\n---\n\n# 经验提案\n\n- 类型：behavior\n- 问题：x\n- 改进：y\n");
        let (t2, _) = approve_pending(&root, "pending/EXPERIENCE.BEHAVIOR.def.md", None).unwrap();
        assert!(t2.starts_with("notes/自组织/行为建议"));
        assert!(root.join("notes/自组织/行为建议").join("def.md").is_file());
        // CODE 型 → 落代码 backlog
        write(&root, "pending/EXPERIENCE.CODE.ghi.md",
            "---\ntype: experience\nsignal: correction\ndate: 2026-08-05\n---\n\n# 经验提案\n\n- 类型：code\n- 问题：z\n- 改进：w\n");
        let (t3, _) = approve_pending(&root, "pending/EXPERIENCE.CODE.ghi.md", None).unwrap();
        assert!(t3.starts_with("notes/自组织/代码提案"));
        assert!(root.join("notes/自组织/代码提案").join("ghi.md").is_file());
        // preview kind=experience
        write(&root, "pending/EXPERIENCE.MEMORY.jkl.md", "---\ntype: experience\n---\n# 经验提案\n- 改进：m\n");
        let pv = preview_pending(&root, "pending/EXPERIENCE.MEMORY.jkl.md").unwrap();
        assert_eq!(pv.kind, "experience");
        assert_eq!(pv.target, "MEMORY.md");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn approve_decision_writes_undecided_and_memory_pointer() {
        let root = test_root("decis");
        ensure_layout(&root).unwrap();
        write(&root, "pending/DECISION.s1-1.md",
            "---\ntype: decision\nsource: sessions/s1.md\ndate: 2026-08-05\n---\n## 议题：是否做 MCP\n上次方案：先做工具链，MCP 后置\n");
        // 幂等性：先批准一次（新建小节），再批准第二次（追加 bullet 不重复建节）
        let (t1, _) = approve_pending(&root, "pending/DECISION.s1-1.md", None).unwrap();
        assert_eq!(t1, "notes/决策/未决.md");
        // 前端真实格式（frontmatter 后带空行）：标题行必须正确提取（非空行）
        write(&root, "pending/DECISION.s1-2.md",
            "---\ntype: decision\nsource: sessions/s1.md\ndate: 2026-08-05\n---\n\n## 议题：第二个议题\n上次方案：方案 B\n");
        let (_, _) = approve_pending(&root, "pending/DECISION.s1-2.md", None).unwrap();
        // 明细落 notes/决策/未决.md（两节，标题行不带空值）
        let dec = fs::read_to_string(root.join("notes/决策/未决.md")).unwrap();
        assert!(dec.contains("是否做 MCP") && dec.contains("第二个议题"));
        assert!(!dec.contains("## [2026-08-05] \n"), "标题行为空: {dec}");
        // MEMORY 决策待定小节只有一个 ##，含两条 bullet
        let mem = fs::read_to_string(root.join("MEMORY.md")).unwrap();
        assert_eq!(mem.matches("## 决策待定").count(), 1);
        assert_eq!(mem.matches("（未决）：见 [[notes/决策/未决.md]]").count(), 2);
        assert!(mem.contains("议题：第二个议题（未决）"), "指针缺议题名: {mem}");
        // preview kind=decision
        write(&root, "pending/DECISION.s2-1.md", "---\ntype: decision\ndate: 2026-08-05\n---\n## 议题：X\n上次方案：Y\n");
        let pv = preview_pending(&root, "pending/DECISION.s2-1.md").unwrap();
        assert_eq!(pv.kind, "decision");
        assert_eq!(pv.target, "notes/决策/未决.md");
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

    // ---------- read_l1（上下文组装 v2） ----------

    #[test]
    fn read_l1_head_mode() {
        let root = test_root("l1head");
        ensure_layout(&root).unwrap();
        fs::write(root.join("RULES.md"), "# 规范\n\n## Frontmatter\n- 所有文件以 --- 开头\n\n## 命名\n- kebab-case\n").unwrap();
        let r = read_l1(&root, "RULES.md", None, 1200).unwrap();
        assert_eq!(r.mode, "head");
        assert!(r.content.starts_with("# 规范"));
        assert_eq!(r.content.chars().count(), r.total_chars, "max 足够大时不截断");
        assert_eq!(r.sections, vec!["Frontmatter".to_string(), "命名".to_string()]);
        // 小 max 截断生效（按字符截，不劈 CJK）
        let r2 = read_l1(&root, "RULES.md", None, 10).unwrap();
        assert_eq!(r2.content.chars().count(), 10);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn read_l1_section_hit_returns_section_original() {
        let root = test_root("l1sec");
        ensure_layout(&root).unwrap();
        fs::write(root.join("FRAMEWORK.md"), "# 框架\n\n## 双链\n- 使用 `[[文件名]]` 在 MD 之间建立链接\n\n## 分层原则\n- L1 只放要点\n").unwrap();
        let r = read_l1(&root, "FRAMEWORK.md", Some("双链"), 1200).unwrap();
        assert_eq!(r.mode, "section");
        assert!(r.content.contains("## 双链"), "小节应含 ## 标题");
        assert!(r.content.contains("[[文件名]]"), "返回源文件原文");
        assert!(!r.content.contains("分层原则"), "只返回命中小节，不含后续小节");
        // 未命中 q 也应能读（memory_summary 是允许读的派生产物）
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn read_l1_section_miss_returns_section_list() {
        let root = test_root("l1miss");
        ensure_layout(&root).unwrap();
        fs::write(root.join("FRAMEWORK.md"), "# 框架\n\n## 双链\n- x\n\n## 分层原则\n- y\n").unwrap();
        let r = read_l1(&root, "FRAMEWORK.md", Some("不存在的词"), 1200).unwrap();
        assert_eq!(r.mode, "section_list");
        assert!(r.content.is_empty());
        assert_eq!(r.sections, vec!["双链".to_string(), "分层原则".to_string()]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn read_l1_rejects_non_whitelist() {
        let root = test_root("l1rej");
        ensure_layout(&root).unwrap();
        write(&root, "notes/不存在.md", "# x\n");
        assert!(read_l1(&root, "不存在.md", None, 1200).is_err());
        // 白名单外（含路径穿越尝试）一律拒绝
        assert!(read_l1(&root, "notes/不存在.md", None, 1200).is_err());
        assert!(read_l1(&root, "../MEMORY.md", None, 1200).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn read_l1_ok_reads_memory_summary_derived() {
        let root = test_root("l1msum");
        ensure_layout(&root).unwrap();
        fs::write(root.join("memory_summary.md"), "# 记忆摘要（自动生成）\n\n## 2026-08-03\n- 决策丙\n").unwrap();
        let r = read_l1(&root, "memory_summary.md", None, 1200).unwrap();
        assert_eq!(r.mode, "head");
        assert!(r.content.contains("决策丙"));
        fs::remove_dir_all(&root).unwrap();
    }

    // ---------- reject_pending ----------

    #[test]
    fn reject_single_and_all() {
        let root = test_root("rej");
        ensure_layout(&root).unwrap();
        write(&root, "pending/notes/a.md", "# A\n");
        write(&root, "pending/MEMORY.b.md", "## 2026-08-03\n- x\n");
        assert_eq!(reject_pending(&root, "pending/notes/a.md").unwrap(), 1);
        assert!(!root.join("pending/notes/a.md").exists());
        assert_eq!(reject_pending(&root, "all").unwrap(), 1);
        assert_eq!(list_pending(&root).len(), 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reject_rejects_outside_pending() {
        let root = test_root("rejguard");
        ensure_layout(&root).unwrap();
        write(&root, "notes/合法.md", "# x\n");
        let r = reject_pending(&root, "notes/合法.md");
        assert!(r.is_err());
        assert!(root.join("notes/合法.md").is_file(), "越权拒绝不得删除目标");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reject_missing_file_errors() {
        let root = test_root("rejmiss");
        ensure_layout(&root).unwrap();
        assert!(reject_pending(&root, "pending/不存在.md").is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    // ---------- memory 合并三分支（memory_added_text） ----------

    #[test]
    fn memory_added_three_branches() {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        // ① 自带 ## 标题 → 原样追加
        let a = memory_added_text("", "## 2026-08-01\n- 自定义节\n");
        assert!(a.contains("## 2026-08-01"), "自带标题原样追加");
        assert!(a.contains("自定义节"), "正文保留");
        // ② 当日小节已存在 → 追加 bullet
        let b = memory_added_text(&format!("# 记忆\n\n## {today}\n- 旧\n"), "- 新条目\n");
        assert!(b.starts_with("\n- 新条目"), "当日节追加 bullet");
        assert!(!b.contains("## "), "不重复建节");
        // ③ 空库 → 建头 + 当日小节
        let c = memory_added_text("", "新条目");
        assert!(c.starts_with(&format!("# 记忆\n\n## {today}")), "空库建头+当日节");
        // ④ 非空但无当日节 → 追加新当日小节
        let d = memory_added_text("# 记忆\n\n## 2026-01-01\n- 旧\n", "新条目");
        assert!(d.contains(&format!("## {today}")), "无当日节则新建");
        assert!(!d.contains("2026-01-01"), "不触碰旧节");
        fs::remove_dir_all(&std::env::temp_dir().join("md-agent-ut-rej")).ok();
    }

    #[test]
    fn memory_added_strips_proposal_hint_lines() {
        // 2026-08-11：LLM 提案的 "> 说明" 引用块与空行不进记忆
        let hint = "> 会话收尾提炼（LLM 生成，批准前请人工核对）。\n\n## 决策\n- 双骨架方案\n\n## 经验\n- 及时沉淀\n";
        let a = memory_added_text("", hint);
        assert!(!a.contains("人工核对"), "提示行剥离");
        assert!(!a.contains("> 会话"), "引用块不进记忆");
        assert!(a.contains("## 决策"), "决策小节保留");
        assert!(a.contains("双骨架方案"), "正文内容保留");
        // 只有提示行 → 无可沉淀
        assert_eq!(memory_added_text("# 记忆\n", "> 仅提示行\n"), "");
        fs::remove_dir_all(&std::env::temp_dir().join("md-agent-ut-rej")).ok();
    }

    #[test]
    fn memory_added_strips_proposal_frontmatter() {
        // 2026-08-11：auto_land 直读提案文件（write_extract_proposal 落盘带 frontmatter），
        // frontmatter 块（--- 到 ---）与提示行不得混入 MEMORY.md
        let prop = "---\ntype: memory\ntitle: 会话记忆提炼\nupdated: 2026-08-11\nsource: sessions/test.md\ntarget: MEMORY.md\n---\n\n\
                    > 会话收尾提炼（LLM 生成，批准前请人工核对）。\n\n\
                    ## 决策\n- 双骨架方案\n\n## 经验\n- 及时沉淀\n";
        let a = memory_added_text("", prop);
        assert!(!a.contains("type: memory"), "frontmatter 不进记忆: {a}");
        assert!(!a.contains("target: MEMORY.md"), "frontmatter 不进记忆: {a}");
        assert!(!a.contains("人工核对"), "提示行剥离");
        assert!(!a.contains("> 会话"), "引用块不进记忆");
        assert!(a.contains("## 决策") && a.contains("双骨架方案"), "正文小节保留: {a}");
        assert!(a.contains("## 经验") && a.contains("及时沉淀"), "正文小节保留: {a}");
        // 只有 frontmatter + 提示行 → 无可沉淀
        assert_eq!(memory_added_text("# 记忆\n", "---\ntype: memory\ntarget: MEMORY.md\n---\n\n> 仅提示行\n"), "");
        fs::remove_dir_all(&std::env::temp_dir().join("md-agent-ut-rej")).ok();
    }

    #[test]
    fn preview_memory_does_not_write() {
        let root = test_root("prev");
        ensure_layout(&root).unwrap();
        fs::write(root.join("MEMORY.md"), "# 记忆\n").unwrap();
        write(&root, "pending/MEMORY.p.md", "## 2026-08-03\n- 预览内容\n");
        let pv = preview_pending(&root, "pending/MEMORY.p.md").unwrap();
        assert_eq!(pv.kind, "memory");
        assert_eq!(pv.target, "MEMORY.md");
        assert!(pv.added.contains("预览内容"));
        // 只读：MEMORY.md 未变、待审文件还在
        let mem = fs::read_to_string(root.join("MEMORY.md")).unwrap();
        assert!(!mem.contains("预览内容"), "preview 不得落盘");
        assert!(root.join("pending/MEMORY.p.md").exists(), "preview 不得消费待审");
        fs::remove_dir_all(&root).unwrap();
    }

    // ---------- resolve_in_kb 路径安全 ----------

    #[test]
    fn resolve_in_kb_blocks_escape() {
        let root = test_root("resv");
        ensure_layout(&root).unwrap();
        write(&root, "notes/合法.md", "# x\n");
        assert!(resolve_in_kb(&root, "notes/合法.md").is_some());
        // 路径穿越 / 绝对路径一律拒绝
        assert!(resolve_in_kb(&root, "../evil.md").is_none());
        assert!(resolve_in_kb(&root, "notes/../../evil.md").is_none());
        if let Some(abs) = std::env::current_dir().ok() {
            assert!(resolve_in_kb(&root, &abs.to_string_lossy()).is_none());
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn resolve_in_kb_allows_existing_ancestors() {
        let root = test_root("resv2");
        ensure_layout(&root).unwrap();
        write(&root, "notes/子目录/目标.md", "# x\n");
        // 目标存在 → 直接命中
        assert!(resolve_in_kb(&root, "notes/子目录/目标.md").is_some());
        // 目标不存在但父目录存在 → 仍返回可写路径（用于新建）
        assert!(resolve_in_kb(&root, "notes/子目录/新文件.md").is_some());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn decide_undecided_roundtrip() {
        let root = test_root("decide");
        ensure_layout(&root).unwrap();
        // 未决清单两节（DECISION 批准路径的真实格式：## [date] 议题：xxx）+ MEMORY 决策待定指针
        fs::create_dir_all(root.join("notes/决策")).unwrap();
        fs::write(
            root.join("notes/决策/未决.md"),
            "## [2026-08-06] 议题：工具选择\n上次方案：A/B/C\n\n## [2026-08-07] 议题：模板命名\n上次方案：lawyer\n",
        )
        .unwrap();
        fs::write(
            root.join("MEMORY.md"),
            "# 记忆\n\n## 决策待定\n- [2026-08-07] 议题：模板命名（未决）：见 [[notes/决策/未决.md]]\n",
        )
        .unwrap();
        // 拍板：命中「模板命名」
        let msg = decide_undecided(&root, "模板命名", "用 lawyer").unwrap();
        assert!(msg.contains("模板命名"));
        // 未决清单只剩工具选择
        let dec = fs::read_to_string(root.join("notes/决策/未决.md")).unwrap();
        assert!(dec.contains("工具选择") && !dec.contains("模板命名"));
        // 已决.md 有记录
        let done = fs::read_to_string(root.join("notes/决策/已决.md")).unwrap();
        assert!(done.contains("模板命名") && done.contains("用 lawyer"));
        // MEMORY：决策待定指针移除 + 决策区追加
        let mem = fs::read_to_string(root.join("MEMORY.md")).unwrap();
        assert!(!mem.contains("（未决）：见"), "指针未移除: {mem}");
        assert!(mem.contains("## 决策") && mem.contains("议题：模板命名：用 lawyer"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn decide_undecided_idempotent_and_missing() {
        let root = test_root("decide2");
        ensure_layout(&root).unwrap();
        // 空参拒绝
        assert!(decide_undecided(&root, "", "x").is_err());
        assert!(decide_undecided(&root, "x", "").is_err());
        // 清单不存在
        assert!(decide_undecided(&root, "议题", "结论").is_err());
        // 命中一次 → 第二次幂等提示
        fs::create_dir_all(root.join("notes/决策")).unwrap();
        fs::write(root.join("notes/决策/未决.md"), "## [2026-08-07] 议题：买哪款\n上次方案：A\n").unwrap();
        decide_undecided(&root, "买哪款", "选 B").unwrap();
        let again = decide_undecided(&root, "买哪款", "选 B").unwrap();
        assert!(again.contains("已拍板"));
        // 未决清单已删（唯一小节移除后）
        assert!(!root.join("notes/决策/未决.md").exists());
        // 无关主题 → Err
        assert!(decide_undecided(&root, "不存在的议题", "x").is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}
