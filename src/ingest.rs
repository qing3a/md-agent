//! 文档摄入（Phase 4 前置于消费化 P0）：anydoc 把外部文档（PDF/DOCX/PPT/XLS/EPUB…）
//! 转成 GFM Markdown，落 kb/notes/ 进检索。dry_run 预览两态，扫描件/加密文档明确报错。

use std::path::Path;

/// 摄入结果
pub struct IngestResult {
    /// 转换出的 Markdown（预览或待落盘正文）
    pub markdown: String,
    /// 识别出的格式（任意 doc 格式的显示名）
    pub format: String,
    /// dry_run=false 时的落盘相对路径（notes/<name>.md）
    pub path: Option<String>,
}

/// 把字节内容转成 Markdown。文件名仅用于扩展名兜底识别。
pub fn convert_bytes(bytes: &[u8], name: &str) -> Result<String, String> {
    let ext_fmt = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(anydoc::Format::from_extension);
    // 优先内容签名识别，签名不可用时回落扩展名（CSV 等无签名格式必须显式命名）
    match anydoc::to_markdown_bytes(bytes, ext_fmt) {
        Ok(md) => Ok(md),
        Err(e) => Err(convert_err_msg(&e)),
    }
}

/// 把 ConvertError 翻译成对用户友好的提示
fn convert_err_msg(e: &anydoc::ConvertError) -> String {
    match e {
        anydoc::ConvertError::Unsupported(_) => {
            "不支持的文档格式（扫描件/图片型 PDF 或未知类型暂不支持）".to_string()
        }
        anydoc::ConvertError::Encrypted => "文档已加密或受密码保护".to_string(),
        anydoc::ConvertError::Malformed { detail, .. } => format!("文档结构异常：{detail}"),
        anydoc::ConvertError::ResourceLimit { limit, detail } => {
            format!("文档超出安全限制（{limit}）：{detail}")
        }
        anydoc::ConvertError::MissingPart { part } => format!("文档缺少必要部分：{part}"),
        anydoc::ConvertError::Io(err) => format!("读取文档失败：{err}"),
        _ => "文档转换失败".to_string(),
    }
}

/// 落盘：转换 → 写 kb/notes/<name>.md（带 frontmatter）→ 返回相对路径。
/// 文件名做安全化（去路径分隔/非法字符），避免注入目录。
pub fn ingest_to_notes(
    kb_root: &Path,
    bytes: &[u8],
    name: &str,
) -> Result<(String, String), String> {
    let md = convert_bytes(bytes, name)?;
    let safe = safe_name(name);
    let rel = format!("notes/{safe}.md");
    let path = kb_root.join(&rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = build_markdown(&md, name);
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok((rel, md))
}

/// 文件名安全化：取最后一段（含 `/` 或 `\` 分隔）的扩展名主体，替换非法字符
fn safe_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = match last.rfind('.') {
        Some(i) if i > 0 => &last[..i],
        _ => last,
    };
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == ':' || c == '*' || c == '?' || c == '"'
                || c == '<' || c == '>' || c == '|'
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.trim().is_empty() {
        "ingested".to_string()
    } else {
        cleaned
    }
}

/// 组装带 frontmatter 的笔记正文
fn build_markdown(body: &str, src_name: &str) -> String {
    let mut title = src_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(src_name)
        .to_string();
    if let Some(i) = title.rfind('.') {
        title.truncate(i);
    }
    format!(
        "---\ntype: note\ntitle: {title}\ntags: []\nsource: {src_name}\n---\n\n{body}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_name_strips_separators() {
        // 取最后一段（路径分隔符前内容丢弃，含 `..\` 越权前缀），非法字符替换
        assert_eq!(safe_name("a/b\\c:*.md"), "c__");
        assert_eq!(safe_name("..\\evil"), "evil");
        assert_eq!(safe_name(""), "ingested");
        assert_eq!(safe_name("合同.pdf"), "合同");
    }

    #[test]
    fn build_markdown_has_frontmatter() {
        let md = build_markdown("# 正文\n\nhello", "doc.pdf");
        assert!(md.starts_with("---\ntype: note"));
        assert!(md.contains("source: doc.pdf"));
        assert!(md.contains("# 正文"));
    }

    #[test]
    fn convert_unsupported_reports_friendly() {
        // 未知格式 + 无签名 → Unsupported（人类可读）
        let err = convert_bytes(&[0u8; 4], "noidea.zzz").unwrap_err();
        assert!(err.contains("不支持的文档格式"));
    }
}
