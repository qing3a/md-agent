//! 检索引擎：ignore 遍历 + grep crate 匹配（内嵌 ripgrep 内核）。
//! 纯文本、无向量库；多关键词任一命中 + 智能大小写（全小写查询视为不区分大小写）。

use grep::regex::RegexMatcher;
use grep::searcher::sinks::UTF8;
use grep::searcher::Searcher;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    /// 相对 KB 根的路径（`/` 分隔）
    pub file: String,
    pub line: u64,
    pub text: String,
    /// 命中所在小节标题（向上最近的一个 `#` 标题；行级切分，无解析器依赖）
    pub section: Option<String>,
    /// 命中行前后上下文片段（仅 ctx=true 时填充）
    pub context: String,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub query: String,
    pub layer: String,
    pub hit_count: usize,
    pub file_count: usize,
    pub hits: Vec<Hit>,
}

/// 空白分隔的多关键词，任一命中；含大写字母则区分大小写
fn build_pattern(query: &str) -> Option<String> {
    let keywords: Vec<&str> = query
        .split_whitespace()
        .filter(|k| !k.is_empty())
        .collect();
    if keywords.is_empty() {
        return None;
    }
    let escaped: Vec<String> = keywords.iter().map(|k| regex::escape(k)).collect();
    let has_upper = keywords.iter().any(|k| k.chars().any(|c| c.is_uppercase()));
    Some(if has_upper {
        escaped.join("|")
    } else {
        format!("(?i){}", escaped.join("|"))
    })
}

pub fn search(root: &Path, query: &str, layer: &str, ctx: bool) -> Result<SearchResult, String> {
    let pattern = build_pattern(query).ok_or_else(|| "查询为空".to_string())?;
    let matcher = RegexMatcher::new_line_matcher(&pattern)
        .map_err(|e| format!("正则构建失败: {e}"))?;

    let root = root.canonicalize().map_err(|e| e.to_string())?;

    // 检索范围：notes=仅 L2；l1=仅 KB 顶层；all=整个库
    let (dir, max_depth): (std::path::PathBuf, Option<usize>) = match layer {
        "notes" => (root.join("notes"), None),
        "l1" => (root.clone(), Some(1)),
        _ => (root.clone(), None),
    };

    let mut hits: Vec<Hit> = Vec::new();
    let mut files_seen: HashSet<String> = HashSet::new();

    if dir.exists() {
        let mut walker = ignore::WalkBuilder::new(&dir)
            .hidden(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .build();
        if let Some(d) = max_depth {
            walker = ignore::WalkBuilder::new(&dir)
                .hidden(false)
                .git_ignore(false)
                .git_global(false)
                .git_exclude(false)
                .max_depth(Some(d))
                .build();
        }
        for entry in walker {
            let Ok(entry) = entry else { continue };
            let Some(ft) = entry.file_type() else { continue };
            if !ft.is_file() {
                continue;
            }
            let path = entry.path();
            // 待审目录不参与检索（pending 待确认后才落地）
            if path.components().any(|c| c.as_os_str() == "pending") {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            // 跳过自动生成的索引，减少噪音
            if path.file_name().and_then(|n| n.to_str()) == Some("INDEX.md") {
                continue;
            }
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");

            let mut searcher = Searcher::new();
            let mut file_hits: Vec<Hit> = Vec::new();
            let res = searcher.search_path(
                matcher.clone(),
                path,
                UTF8(|line_number, line| -> std::io::Result<bool> {
                    file_hits.push(Hit {
                        file: rel.clone(),
                        line: line_number,
                        text: line.trim_end().to_string(),
                        section: None,
                        context: String::new(),
                    });
                    Ok(true)
                }),
            );
            if res.is_ok() && !file_hits.is_empty() {
                files_seen.insert(rel.clone());
                // 行级小节上下文（替代解析器；整篇只读一次，按行切分）
                if let Ok(content) = std::fs::read_to_string(path) {
                    let lines: Vec<&str> = content.lines().collect();
                    for h in &mut file_hits {
                        h.section = section_of(&lines, h.line);
                        if ctx {
                            h.context = context_of(&lines, h.line, 3, 600);
                        }
                    }
                }
                hits.extend(file_hits);
            }
        }
    }

    hits.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    let hit_count = hits.len();
    let file_count = files_seen.len();
    Ok(SearchResult {
        query: query.to_string(),
        layer: layer.to_string(),
        hit_count,
        file_count,
        hits,
    })
}

/// 命中行（1-based）所属小节：向上找最近的一个 `#` 标题行
fn section_of(lines: &[&str], hit_line: u64) -> Option<String> {
    if lines.is_empty() {
        return None;
    }
    let idx = ((hit_line as usize).saturating_sub(1)).min(lines.len() - 1);
    for i in (0..=idx).rev() {
        let t = lines[i].trim_start();
        if t.starts_with('#') {
            return Some(t.trim_start_matches('#').trim().to_string());
        }
    }
    None
}

/// 命中行上下文片段：前后 radius 行，上限 max_chars
fn context_of(lines: &[&str], hit_line: u64, radius: usize, max_chars: usize) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let idx = ((hit_line as usize).saturating_sub(1)).min(lines.len() - 1);
    let lo = idx.saturating_sub(radius);
    let hi = (idx + radius + 1).min(lines.len());
    let mut out = String::new();
    for l in &lines[lo..hi] {
        out.push_str(l.trim_end());
        out.push('\n');
        if out.len() > max_chars {
            break;
        }
    }
    out.trim_end().to_string()
}
