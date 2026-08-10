//! 跨会话记忆（Phase 4 M3，参考 ZCode project_memory_extract/dream/recall 映射到人审闭环）：
//! - recall：提问前双路检索（grep + 可选语义）注入上下文——纯只读，写回永远走 pending 人审；
//! - extract：会话收尾 LLM 提炼 → pending/MEMORY.* 提案 → 人审批准合并进 MEMORY.md；
//! - dream：后台巩固子 agent（复用 agent.rs 受限循环）→ pending/CONSOLIDATE.* 提案 → 人审。
//! 人审铁律不变：本模块只生成提案，绝不直写 L1/L2。

use serde::Serialize;
use std::path::Path;

/// 召回命中（注入上下文的片段）
#[derive(Debug, Clone, Serialize)]
pub struct RecallHit {
    pub file: String,
    pub line: u64,
    pub section: Option<String>,
    pub text: String,
    pub score: f64,
}

/// recall 结果
#[derive(Debug, Serialize)]
pub struct RecallResult {
    pub query: String,
    /// 是否叠加了语义通道（embedding 配置并成功时 true）
    pub semantic: bool,
    pub hits: Vec<RecallHit>,
}

/// 提问前记忆召回：grep（layer=all：MEMORY.md + 会话归档 + L2 全部）+ 语义（embedding 配置时）RRF 融合。
/// 纯只读（连向量库都不写）；embedding 未配置/失败自动降级纯 grep。
pub async fn recall(root: &Path, query: &str, k: usize) -> Result<RecallResult, String> {
    let mut r = crate::search::search(root, query, "all", true)?;
    let mut semantic = false;
    // 语义通道（可选）：配置了 embedding 且已有向量索引 → 融合；任何失败静默降级
    let cfg = crate::config::load();
    if crate::embed::configured(&cfg.llm.embedding.endpoint, &cfg.llm.embedding.model) {
        match crate::embed::embed_texts(
            &cfg.llm.embedding.endpoint,
            &cfg.llm.embedding.model,
            &cfg.llm.embedding.api_key,
            &[query.to_string()],
        )
        .await
        {
            Ok(mut qvs) => {
                if let Some(qv) = qvs.pop() {
                    if let Ok(sem) = crate::vector::semantic_search(
                        root,
                        &qv,
                        k * 3,
                        Some(&cfg.llm.embedding.model),
                    ) {
                        r = crate::search::merge_semantic(root, r, &sem, 60.0);
                        semantic = true;
                    }
                }
            }
            Err(_) => {}
        }
    }
    let hits: Vec<RecallHit> = r
        .hits
        .into_iter()
        .take(k)
        .map(|h| RecallHit {
            file: h.file,
            line: h.line,
            section: h.section,
            text: h.text,
            score: h.score,
        })
        .collect();
    Ok(RecallResult {
        query: query.to_string(),
        semantic,
        hits,
    })
}

// ---------- extract（会话收尾提炼 → pending/MEMORY.* 提案，人审后合并进 MEMORY.md） ----------

/// 会话提炼 prompt（三段式：决策/经验/事实，产出可直接人审的提案正文）
fn extract_system_prompt() -> &'static str {
    "你是知识库记忆整理器。从以下会话中提炼可跨会话复用的记忆，只输出 markdown，不要任何解释：\n\
     ## 决策\n- 已拍板的决策（一句话一条）\n\
     ## 经验\n- 可复用的教训/方法（一句话一条）\n\
     ## 事实\n- 新确认的事实/约定（一句话一条）\n\
     没有内容的段落整个省略（不要空段落）。"
}

/// 会话收尾提炼：LLM 三段式提炼 → 写 pending/MEMORY.EXTRACT-<ts>.md 提案。
/// 返回提案相对路径；提炼内容为空 → Ok(None)（无可沉淀）。
/// 绝不直写 L1/L2——批准走 approve_pending（MEMORY.* 前缀 → 合并进 MEMORY.md）。
pub async fn extract_proposal(root: &Path, qa: &str, source: &str) -> Result<Option<String>, String> {
    if qa.trim().is_empty() {
        return Ok(None);
    }
    let cfg = crate::config::load();
    if cfg.llm.endpoint.trim().is_empty() || cfg.llm.model.trim().is_empty() {
        return Err("未配置 LLM（llm.endpoint/model），无法提炼会话记忆".to_string());
    }
    let body = serde_json::json!({ "messages": [
        { "role": "system", "content": extract_system_prompt() },
        { "role": "user", "content": format!("会话来源：{source}\n\n{qa}") },
    ]});
    let resp = crate::llm::chat(&cfg.llm.endpoint, &cfg.llm.model, &cfg.llm.api_key, body)
        .await
        .map_err(|e| format!("会话提炼 LLM 调用失败: {e}"))?;
    let content = resp["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    if content.is_empty() {
        return Ok(None);
    }
    write_extract_proposal(root, &content, source)
        .map(Some)
}

/// 落盘 extract 提案（pending/MEMORY.EXTRACT-<ts>.md）——纯文件操作，可单测。
pub fn write_extract_proposal(root: &Path, content: &str, source: &str) -> Result<String, String> {
    let pending = root.join("pending");
    std::fs::create_dir_all(&pending).map_err(|e| e.to_string())?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let file = format!("pending/MEMORY.EXTRACT-{ts}.md");
    let prop = format!(
        "---\ntype: memory\ntitle: 会话记忆提炼\nupdated: {today}\nsource: {source}\ntarget: MEMORY.md\n---\n\n\
         > 会话收尾提炼（LLM 生成，批准前请人工核对）。批准后按当日小节合并进 MEMORY.md。\n\n{content}\n"
    );
    std::fs::write(root.join(&file), prop).map_err(|e| format!("写入提案失败: {e}"))?;
    // 人审收敛（2026-08-11）：会话提炼派生产物自动落地（git 自动提交即回滚）；MEMORY. 修改类不补链
    match crate::kb::auto_land(root, &file) {
        Ok((_, note)) => crate::activity::record(
            root,
            "pending",
            &format!("自动沉淀: {}", note.as_deref().unwrap_or("")),
            serde_json::json!({}),
        ),
        Err(e) => crate::activity::record(root, "sys", &format!("自动沉淀失败 {file}: {e}"), serde_json::json!({})),
    }
    Ok(file)
}

// ---------- dream（后台巩固子 agent → 自动落地替换 MEMORY.md，git 兜底） ----------

/// 巩固 dream prompt：分析 MEMORY.md 全文，输出改进版（去重/合并/补链建议内联注释）。
/// 要求保留原有有效内容，只做整理；无改进时输出「无需改进」。
fn dream_system_prompt() -> &'static str {
    "你是知识库记忆巩固器。以下 MEMORY.md 是跨会话持久记忆。请整理输出改进版全文（只输出 markdown）：\n\
     1. 删除完全重复的行/条目；\n\
     2. 同主题的零散条目合并成一条（保留信息不丢失）；\n\
     3. 明显关联的知识补上 [[双链]] 提示（在条目后加（可关联 [[文档名]]）注释）；\n\
     4. 保留原有结构（# 记忆 / ## 日期 小节）与所有有效信息。\n\
     若记忆已经很干净无需任何改动，只输出：无需改进"
}

/// 后台巩固 dream：LLM 分析 MEMORY.md → 改进版提案写 pending/CONSOLIDATE.DREAM-<ts>.md。
/// 返回提案相对路径列表（可空）；无 LLM 配置 → Err（调用方决定是否忽略）。
/// 绝不直写——批准走 approve_pending（CONSOLIDATE.* → target 替换 MEMORY.md）。
pub async fn dream_proposals(root: &Path) -> Result<Vec<String>, String> {
    let mem_path = root.join("MEMORY.md");
    let Ok(mem) = std::fs::read_to_string(&mem_path) else {
        return Ok(Vec::new()); // 无 MEMORY.md → 无事可巩固（环境无关，先于 LLM 配置检查）
    };
    if mem.trim().is_empty() {
        return Ok(Vec::new());
    }
    let cfg = crate::config::load();
    if cfg.llm.endpoint.trim().is_empty() || cfg.llm.model.trim().is_empty() {
        return Err("未配置 LLM（llm.endpoint/model），无法运行记忆巩固".to_string());
    }
    let body = serde_json::json!({ "messages": [
        { "role": "system", "content": dream_system_prompt() },
        { "role": "user", "content": mem.clone() },
    ]});
    let resp = crate::llm::chat(&cfg.llm.endpoint, &cfg.llm.model, &cfg.llm.api_key, body)
        .await
        .map_err(|e| format!("巩固 dream LLM 调用失败: {e}"))?;
    let improved = resp["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    if improved.is_empty() || improved.contains("无需改进") {
        return Ok(Vec::new());
    }
    let file = write_dream_proposal(root, &improved)?;
    Ok(vec![file])
}

/// 落盘 dream 提案（pending/CONSOLIDATE.DREAM-<ts>.md）——纯文件操作，可单测。
/// frontmatter target: MEMORY.md——批准后正文替换 MEMORY.md（与 consolidate 提案同契约）。
pub fn write_dream_proposal(root: &Path, improved: &str) -> Result<String, String> {
    let pending = root.join("pending");
    std::fs::create_dir_all(&pending).map_err(|e| e.to_string())?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let file = format!("pending/CONSOLIDATE.DREAM-{ts}.md");
    let prop = format!(
        "---\ntype: consolidate\ntitle: 记忆巩固（dream）\nupdated: {today}\ntarget: MEMORY.md\n---\n\n\
         > 后台巩固（LLM 生成，批准前请人工核对）。批准后替换 MEMORY.md。\n\n{improved}\n"
    );
    std::fs::write(root.join(&file), prop).map_err(|e| format!("写入提案失败: {e}"))?;
    // 人审收敛（2026-08-11）：dream 巩固派生产物自动落地（替换 MEMORY.md，git 兜底）；CONSOLIDATE. 修改类不补链
    match crate::kb::auto_land(root, &file) {
        Ok((_, note)) => crate::activity::record(
            root,
            "pending",
            &format!("自动沉淀: {}", note.as_deref().unwrap_or("")),
            serde_json::json!({}),
        ),
        Err(e) => crate::activity::record(root, "sys", &format!("自动沉淀失败 {file}: {e}"), serde_json::json!({})),
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn tmp_kb(name: &str) -> std::path::PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("md-agent-mem-test-{name}-{n}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("notes")).unwrap();
        std::fs::write(d.join("MEMORY.md"), "# 记忆\n\n## 2026-08-01\n- 初始记忆\n").unwrap(); // 落地目标（auto_land 消费）
        d
    }

    fn write(d: &std::path::Path, rel: &str, content: &str) {
        let p = d.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }

    #[test]
    fn recall_grep_hits_memory_and_archives() {
        let d = tmp_kb("grep");
        write(&d, "MEMORY.md", "# 记忆\n\n## 2026-08-11\n- 双层 MD 记忆是人审闭环的核心\n");
        write(&d, "notes/会话归档/2026-08-10-x.md", "# 会话归档\n决定了托盘架构方案\n");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(recall(&d, "记忆 人审", 5)).unwrap();
        assert!(!r.hits.is_empty(), "应命中 MEMORY.md");
        let files: Vec<&str> = r.hits.iter().map(|h| h.file.as_str()).collect();
        assert!(files.contains(&"MEMORY.md"), "{files:?}");
        assert!(!r.semantic, "未配置 embedding 时无语义通道");
    }

    #[test]
    fn recall_no_match_returns_empty() {
        let d = tmp_kb("nomatch");
        write(&d, "MEMORY.md", "# 记忆\n- 与查询无关\n");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(recall(&d, "完全不存在的词xyz", 5)).unwrap();
        assert!(r.hits.is_empty());
    }

    #[test]
    fn recall_respects_k_limit() {
        let d = tmp_kb("limit");
        for i in 0..6 {
            write(&d, &format!("notes/文档{i}.md"), &format!("# 文档{i}\n共同关键词 内容{i}\n"));
        }
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(recall(&d, "共同关键词", 3)).unwrap();
        assert!(r.hits.len() <= 3, "top k 截断: {}", r.hits.len());
    }

    #[test]
    fn recall_empty_query_errors() {
        let d = tmp_kb("emptyq");
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(rt.block_on(recall(&d, "", 5)).is_err());
    }

    // ---------- extract（会话提炼 → pending 提案） ----------

    #[test]
    fn write_extract_proposal_lands_pending_memory() {
        let d = tmp_kb("extract");
        let f = write_extract_proposal(&d, "## 决策\n- 用双层 MD 记忆\n", "sessions/x.md").unwrap();
        assert!(f.starts_with("pending/MEMORY.EXTRACT-"), "{f}");
        // 人审收敛（2026-08-11）：提案自动落地——pending 已消费，提炼内容并入 MEMORY.md
        assert!(!d.join(&f).is_file(), "提案已自动落地（pending 清空）");
        let mem = std::fs::read_to_string(d.join("MEMORY.md")).unwrap();
        assert!(mem.contains("双层 MD 记忆"), "提炼内容应并入 MEMORY.md");
        let pending = crate::kb::list_pending(&d);
        assert!(!pending.iter().any(|p| p.path == f), "落地后不在待审队列");
    }

    #[test]
    fn write_extract_proposal_creates_pending_dir() {
        let d = tmp_kb("extractdir");
        let f = write_extract_proposal(&d, "## 事实\n- x\n", "qa").unwrap();
        assert!(d.join("pending").is_dir());
        assert!(!d.join(&f).is_file(), "自动落地后 pending 文件已消费");
        let mem = std::fs::read_to_string(d.join("MEMORY.md")).unwrap();
        assert!(mem.contains("- x"), "事实内容并入 MEMORY.md");
    }

    #[test]
    fn extract_proposal_empty_qa_returns_none() {
        let d = tmp_kb("emptyqa");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(extract_proposal(&d, "  ", "sessions/a.md")).unwrap();
        assert!(r.is_none(), "空会话无可沉淀");
    }

    #[test]
    fn extract_system_prompt_has_three_sections() {
        let p = extract_system_prompt();
        assert!(p.contains("## 决策") && p.contains("## 经验") && p.contains("## 事实"), "{p}");
    }

    // ---------- dream（后台巩固 → pending 提案） ----------

    #[test]
    fn write_dream_proposal_lands_pending_consolidate() {
        let d = tmp_kb("dream");
        let f = write_dream_proposal(&d, "# 记忆\n\n## 2026-08-11\n- 合并后的记忆\n").unwrap();
        assert!(f.starts_with("pending/CONSOLIDATE.DREAM-"), "{f}");
        // 人审收敛（2026-08-11）：dream 自动落地——MEMORY.md 被替换为改进版
        assert!(!d.join(&f).is_file(), "提案已自动落地（pending 清空）");
        let mem = std::fs::read_to_string(d.join("MEMORY.md")).unwrap();
        assert!(mem.contains("合并后的记忆"), "MEMORY.md 应被改进版替换");
        let pending = crate::kb::list_pending(&d);
        assert!(!pending.iter().any(|p| p.path == f), "落地后不在待审队列");
    }

    #[test]
    fn write_dream_proposal_creates_pending_dir() {
        let d = tmp_kb("dreamdir");
        let f = write_dream_proposal(&d, "改进内容").unwrap();
        assert!(d.join("pending").is_dir());
        assert!(!d.join(&f).is_file(), "自动落地后 pending 文件已消费");
        let mem = std::fs::read_to_string(d.join("MEMORY.md")).unwrap();
        assert!(mem.contains("改进内容"), "MEMORY.md 应被替换");
    }

    #[test]
    fn dream_system_prompt_asks_for_improvements() {
        let p = dream_system_prompt();
        assert!(p.contains("合并") && p.contains("重复") && p.contains("无需改进"), "{p}");
    }

    #[test]
    fn dream_proposals_missing_memory_returns_empty() {
        let d = tmp_kb("nomem");
        std::fs::remove_file(d.join("MEMORY.md")).unwrap(); // 显式移除：测「无 MEMORY.md」前提
        let rt = tokio::runtime::Runtime::new().unwrap();
        // 无 MEMORY.md → 无事可巩固（即使未配置 LLM 也不报错）
        let r = rt.block_on(dream_proposals(&d)).unwrap();
        assert!(r.is_empty());
    }
}
