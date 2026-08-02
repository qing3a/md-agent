//! 巩固器（Phase 3-C Step 2）：先规则后 LLM——v1 只用确定性规则生成巩固提案（防幻觉删真记忆）。
//! 输出：pending/CONSOLIDATE.*.md（frontmatter `target` 指向目标文件，正文 = 替换后全文），
//! 经待审人审（/preview → /approve）后由 kb::approve_pending 落地（替换目标文件）。

use crate::graph::AuditReport;
use std::collections::HashSet;
use std::path::Path;

/// 生成巩固提案，返回新生成的提案文件列表（相对 kb 根路径）。
/// v1 规则：
/// 1. MEMORY.md 行级去重（确定性、安全——只删完全相同的重复行，内容无损）
/// 2. 重复标题文档提示型提案（不自动合并内容——防误删，正文保留第一篇原文，人工决定合并/去重）
pub fn generate_proposals(root: &Path, audit: &AuditReport) -> std::io::Result<Vec<String>> {
    let pending = root.join("pending");
    std::fs::create_dir_all(&pending)?;
    let mut created: Vec<String> = Vec::new();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    // 规则 1：MEMORY.md 行级去重
    let mem_path = root.join("MEMORY.md");
    if let Ok(content) = std::fs::read_to_string(&mem_path) {
        let lines: Vec<&str> = content.lines().collect();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut dup = 0usize;
        let mut dedup: Vec<&str> = Vec::new();
        for l in &lines {
            if l.trim().is_empty() {
                dedup.push(l); // 空行保留
                continue;
            }
            if seen.insert(l) {
                dedup.push(l);
            } else {
                dup += 1;
            }
        }
        if dup > 0 {
            let new_content = dedup.join("\n") + "\n";
            let file = format!("pending/CONSOLIDATE.MEMORY-{}.md", timestamp());
            let prop = format!(
                "---\ntype: consolidate\ntitle: 记忆去重\nupdated: {today}\ntarget: MEMORY.md\n---\n\n\
> 巩固器 v1 规则：MEMORY.md 检测到 {dup} 行完全重复，已去重（只删重复行，内容无损）。人工可编辑后批准。\n\n{new_content}"
            );
            std::fs::write(root.join(&file), prop)?;
            created.push(file);
        }
    }

    // 规则 2：重复标题文档提示型提案（正文 = 第一篇原文，人审决定是否合并/去重）
    for (title, count, paths) in &audit.duplicates {
        let first = paths.split('|').next().unwrap_or("");
        if first.is_empty() {
            continue;
        }
        if let Ok(orig) = std::fs::read_to_string(root.join(first)) {
            let safe: String = title
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .take(12)
                .collect();
            let file = format!("pending/CONSOLIDATE.DUP-{safe}-{}.md", timestamp());
            let prop = format!(
                "---\ntype: consolidate\ntitle: 重复标题处理\nupdated: {today}\ntarget: {first}\n---\n\n\
> 巩固器 v1：检测到标题「{title}」重复 {count} 次（{paths}）。正文为第一篇原文，人工决定合并/去重后批准替换。\n\n{orig}"
            );
            std::fs::write(root.join(&file), prop)?;
            created.push(file);
        }
    }

    Ok(created)
}

fn timestamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}
