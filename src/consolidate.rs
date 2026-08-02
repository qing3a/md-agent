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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn test_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("md-agent-consol-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("notes")).unwrap();
        fs::create_dir_all(dir.join("pending")).unwrap();
        dir
    }

    #[test]
    fn dedup_memory_lines() {
        let root = test_root("dedup");
        fs::write(root.join("MEMORY.md"), "# M\n\n## 2026-08-03\n- A\n- A\n- B\n").unwrap();
        let audit = AuditReport { docs: 1, links: 0, dangling: vec![], orphans: vec![], no_out: vec![], duplicates: vec![], mentions: vec![] };
        let created = generate_proposals(&root, &audit).unwrap();
        assert!(created.iter().any(|c| c.contains("CONSOLIDATE.MEMORY")));
        let prop = fs::read_to_string(root.join(&created[0])).unwrap();
        assert_eq!(prop.matches("- A").count(), 1); // 重复行只剩一条
        assert!(prop.contains("- B"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn dup_title_proposal() {
        let root = test_root("dup");
        fs::write(root.join("notes/a.md"), "# 重复主题\n\n内容A\n").unwrap();
        let audit = AuditReport { docs: 2, links: 0, dangling: vec![], orphans: vec![], no_out: vec![], duplicates: vec![("重复主题".to_string(), 2, "notes/a.md | notes/b.md".to_string())], mentions: vec![] };
        let created = generate_proposals(&root, &audit).unwrap();
        assert!(created.iter().any(|c| c.contains("CONSOLIDATE.DUP")));
        let prop = fs::read_to_string(root.join(&created[0])).unwrap();
        assert!(prop.contains("重复 2 次") && prop.contains("内容A"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn no_proposal_when_clean() {
        let root = test_root("clean");
        fs::write(root.join("MEMORY.md"), "# M\n- A\n- B\n").unwrap();
        let audit = AuditReport { docs: 1, links: 0, dangling: vec![], orphans: vec![], no_out: vec![], duplicates: vec![], mentions: vec![] };
        let created = generate_proposals(&root, &audit).unwrap();
        assert!(created.is_empty());
        fs::remove_dir_all(&root).unwrap();
    }
}

/// v2：LLM 生成重复标题文档的整合版（防幻觉：正文标注「LLM 生成，人工核对」，冲突保留）。
/// 返回新生成的提案列表；未配置 LLM 返回 Err。
pub async fn generate_llm_proposals(
    root: &Path,
    audit: &AuditReport,
) -> Result<Vec<String>, String> {
    let cfg = crate::config::load();
    if cfg.llm.endpoint.trim().is_empty() {
        return Err("未配置 LLM（llm.endpoint/model/api_key），无法使用巩固器 v2".to_string());
    }
    let mut created: Vec<String> = Vec::new();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    for (title, count, paths) in &audit.duplicates {
        if *count < 2 {
            continue;
        }
        let parts: Vec<&str> = paths.split('|').map(|x| x.trim()).collect();
        if parts.len() < 2 {
            continue;
        }
        let mut docs = Vec::new();
        for p in &parts {
            if let Ok(c) = std::fs::read_to_string(root.join(p)) {
                docs.push(format!("### 文档 {p}\n{c}"));
            }
        }
        if docs.len() < 2 {
            continue;
        }
        let system = "你是知识库记忆整理器。以下多篇文档标题重复，请生成一篇整合版：\
            合并共同内容、保留各自独有要点、末尾用「## 冲突与存疑」列出冲突或存疑处。\
            直接输出合并后的 markdown 全文（以 # 标题开头），不要任何解释。";
        let user = format!("重复标题：{title}\n\n{}", docs.join("\n\n"));
        let body = serde_json::json!({ "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ]});
        let resp = crate::llm::chat(&cfg.llm.endpoint, &cfg.llm.model, &cfg.llm.api_key, body)
            .await
            .map_err(|e| format!("巩固器 v2 LLM 调用失败: {e}"))?;
        let merged = resp["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        if merged.is_empty() {
            continue;
        }
        let safe: String = title
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .take(12)
            .collect();
        let file = format!("pending/CONSOLIDATE.DUP-LLM-{safe}-{}.md", timestamp());
        let prop = format!(
            "---\ntype: consolidate\ntitle: 重复标题LLM合并\nupdated: {today}\ntarget: {first}\n---\n\n\
> 巩固器 v2（LLM）：标题「{title}」重复 {count} 次（{paths}），以下为 LLM 整合版。\n\
> ⚠ LLM 生成内容，批准前请人工核对；冲突/存疑已在正文标注。\n\n{merged}\n",
            first = parts[0]
        );
        std::fs::write(root.join(&file), prop).map_err(|e| format!("写入提案失败: {e}"))?;
        created.push(file);
    }
    Ok(created)
}
