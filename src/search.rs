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

// ---------- 记忆关联建议（记忆断链修复 B） ----------

#[derive(Debug, Serialize)]
pub struct SuggestLink {
    /// 相对 KB 根路径（notes/...）
    pub path: String,
    /// 命中的检索词（去重）
    pub matched: Vec<String>,
    /// 总命中次数
    pub hits: usize,
}

/// 中文通用词（关联评分时剔除，防噪声命中）
const LINK_STOPWORDS: &[&str] = &[
    "这个", "那个", "这些", "那些", "因为", "所以", "但是", "然后", "可以", "就是", "不是",
    "一个", "我们", "你们", "他们", "什么", "怎么", "如何", "现在", "还是", "没有", "进行",
    "已经", "需要", "应该", "相关", "之后", "之前", "比如", "例如", "如果", "问题", "内容",
];

/// 记忆关联建议：给定记忆条目文本，从 L2 notes/ 找相关文档（词重叠评分）。
/// ASCII 词(≥3 含字母) + CJK 二元组（去停用词）做检索词；每文档统计「命中词数」与
/// 总命中数，按命中词数降序、再按总命中降序；优先命中词数 ≥2 的文档（防通用词噪声），
/// 不足再用 ≥1 的补到 limit。返回 top N——供写回 MEMORY 时生成「相关：[[双链]]」建议。
pub fn suggest_links(root: &Path, content: &str, limit: usize) -> Result<Vec<SuggestLink>, String> {
    let terms = link_terms(content);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let notes = root.join("notes");
    // (路径, 命中词集合, 总命中数)
    let mut scored: Vec<(String, HashSet<String>, usize)> = Vec::new();
    if notes.exists() {
        let mut walker = ignore::WalkBuilder::new(&notes)
            .hidden(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .build();
        for entry in walker.flatten() {
            let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
            if !is_file {
                continue;
            }
            let p = entry.path();
            if p.extension().map(|e| e != "md").unwrap_or(true) {
                continue;
            }
            let rel = p
                .strip_prefix(&root)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_default();
            let Ok(text) = std::fs::read_to_string(p) else { continue };
            let lower = text.to_lowercase();
            let mut matched: HashSet<String> = HashSet::new();
            let mut hits = 0usize;
            for t in &terms {
                let c = lower.matches(t).count();
                if c > 0 {
                    hits += c;
                    matched.insert(t.clone());
                }
            }
            if !matched.is_empty() {
                scored.push((rel, matched, hits));
            }
        }
    }
    scored.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));

    let to_link = |(path, matched, hits): &(String, HashSet<String>, usize)| -> SuggestLink {
        let mut m: Vec<String> = matched.iter().cloned().collect();
        m.sort();
        SuggestLink { path: path.clone(), matched: m, hits: *hits }
    };

    let mut out: Vec<SuggestLink> = Vec::new();
    for s in scored.iter().filter(|s| s.1.len() >= 2) {
        out.push(to_link(s));
        if out.len() >= limit {
            return Ok(out);
        }
    }
    let weak: Vec<&(String, HashSet<String>, usize)> = scored
        .iter()
        .filter(|s| s.1.len() == 1 && !out.iter().any(|o| o.path == s.0))
        .collect();
    for s in weak {
        out.push(to_link(s));
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

/// 提取关联检索词：ASCII 词（≥3 且含字母，小写）+ CJK（U+4E00..9FFF）连续片段滑窗二元组
fn link_terms(content: &str) -> Vec<String> {
    let mut set: HashSet<String> = HashSet::new();
    let mut buf = String::new();
    let mut ascii_mode = false;
    let flush = |buf: &mut String, ascii_mode: bool, set: &mut HashSet<String>| {
        if buf.is_empty() {
            return;
        }
        if ascii_mode {
            if buf.len() >= 3 && buf.chars().any(|c| c.is_ascii_alphabetic()) {
                set.insert(buf.to_lowercase());
            }
        } else {
            let s: Vec<char> = buf.chars().collect();
            for i in 0..s.len().saturating_sub(1) {
                let bg: String = s[i..i + 2].iter().collect();
                if !LINK_STOPWORDS.contains(&bg.as_str()) {
                    set.insert(bg);
                }
            }
        }
        buf.clear();
    };
    for ch in content.chars() {
        let is_ascii = ch.is_ascii_alphanumeric() || ch == '_';
        let is_cjk = ('\u{4E00}'..='\u{9FFF}').contains(&ch);
        if is_ascii {
            if !ascii_mode {
                flush(&mut buf, ascii_mode, &mut set);
                ascii_mode = true;
            }
            buf.push(ch);
        } else if is_cjk {
            if ascii_mode {
                flush(&mut buf, ascii_mode, &mut set);
                ascii_mode = false;
            }
            buf.push(ch);
        } else {
            flush(&mut buf, ascii_mode, &mut set);
            ascii_mode = false;
        }
    }
    flush(&mut buf, ascii_mode, &mut set);
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("md-agent-st-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("notes/架构")).unwrap();
        d
    }

    #[test]
    fn suggest_links_finds_related_doc() {
        let root = tmp("links");
        std::fs::write(root.join("notes/架构/托盘应用.md"), "# 托盘应用\n托盘心跳 双链 CheckMenuItem 参数\n").unwrap();
        std::fs::write(root.join("notes/其它文档.md"), "# 其它\n无相关内容\n").unwrap();
        let out = suggest_links(&root, "修了托盘心跳 CheckMenuItem 参数 bug", 3).unwrap();
        assert_eq!(out.len(), 1, "只应命中相关文档");
        assert!(out[0].path.ends_with("托盘应用.md"));
        assert!(out[0].matched.len() >= 2, "强关联应命中多个词");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn suggest_links_empty_on_no_match() {
        let root = tmp("links2");
        std::fs::write(root.join("notes/文档.md"), "# 文档\n只有不相关内容\n").unwrap();
        let out = suggest_links(&root, "跟库里完全无关的话题讨论", 3).unwrap();
        assert!(out.is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn link_terms_no_punct_garbage() {
        let t = link_terms("为什么会有孤立文档？现在 修了bug");
        // 全角问号不应产生带标点二元组
        assert!(!t.iter().any(|s| s.contains('？')), "不得含全角标点: {t:?}");
        assert!(t.iter().any(|s| s == "checkmenuitem" || s == "bug"), "ASCII 词应保留");
        assert!(t.iter().any(|s| s == "文档"), "中文二元组应保留");
    }
}
