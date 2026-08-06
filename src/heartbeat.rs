//! 心跳自动同步：周期指纹检测知识库变化，变化则重建 INDEX + 图谱；
//! 重建后顺带跑本地审计（零 LLM），把发现摘要进状态供状态栏提示——
//! 自组织「自动发现、人审执行」闭环的自动发现侧。

use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Default)]
pub struct AuditBrief {
    pub orphans: usize,
    pub dangling: usize,
    pub duplicates: usize,
    pub mentions: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct HeartbeatStatus {
    pub enabled: bool,
    pub interval_secs: u64,
    pub last_sync: Option<String>,
    pub files: usize,
    pub changed: bool,
    pub audit: Option<AuditBrief>,
}

/// 知识库指纹：活跃语料（全部 .md，排除 pending/，与检索/图谱口径一致）的
/// (相对路径, mtime 秒, 大小) 排序列表。
pub fn fingerprint(root: &Path) -> Vec<(String, i64, u64)> {
    let mut entries = Vec::new();
    let walk = ignore::WalkBuilder::new(root)
        .hidden(false)
        // pending 待审 + sessions 会话快照（流水非知识）不参与指纹，避免其变更触发重建
        .filter_entry(|e| e.file_name() != "pending" && e.file_name() != "sessions")
        .build();
    for ent in walk.flatten() {
        let is_file = ent.file_type().map(|t| t.is_file()).unwrap_or(false);
        if !is_file {
            continue;
        }
        let p = ent.path();
        if p.extension().map(|e| e != "md").unwrap_or(true) {
            continue;
        }
        let rel = p
            .strip_prefix(root)
            .map(|r| r.to_string_lossy().into_owned())
            .unwrap_or_default();
        let meta = ent.metadata().ok();
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let size = meta.map(|m| m.len()).unwrap_or(0);
        entries.push((rel, mtime, size));
    }
    entries.sort();
    entries
}

/// 指纹比较键（mtime 秒精度足够：本机编辑间隔远大于 1s）
pub fn fingerprint_key(fp: &[(String, i64, u64)]) -> String {
    let mut s = String::with_capacity(fp.len() * 32);
    for (p, m, l) in fp {
        s.push_str(p);
        s.push(':');
        s.push_str(&m.to_string());
        s.push(':');
        s.push_str(&l.to_string());
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_root(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("md-agent-ut-hb-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &std::path::Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn fingerprint_excludes_pending_and_sessions() {
        let root = test_root("excl");
        write(&root, "KB.md", "# KB\n");
        write(&root, "notes/知识.md", "# 知识\n");
        write(&root, "pending/草稿.md", "# 草稿\n");
        write(&root, "sessions/流水.md", "# 流水\n");
        let fp = fingerprint(&root);
        let names: Vec<&str> = fp.iter().map(|(p, _, _)| p.as_str()).collect();
        assert!(names.contains(&"KB.md"));
        assert!(names.iter().any(|n| n.replace('\\', "/") == "notes/知识.md"), "notes 应参与指纹: {names:?}");
        assert!(!names.iter().any(|n| n.contains("pending")), "待审不参与指纹");
        assert!(!names.iter().any(|n| n.contains("sessions")), "会话流水不参与指纹");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn fingerprint_changes_when_content_changes() {
        let root = test_root("change");
        write(&root, "notes/文档.md", "# 文档\n第一版\n");
        let fp1 = fingerprint(&root);
        let key1 = fingerprint_key(&fp1);
        std::thread::sleep(std::time::Duration::from_millis(1100)); // mtime 秒精度
        write(&root, "notes/文档.md", "# 文档\n第二版（变长了）\n");
        let fp2 = fingerprint(&root);
        let key2 = fingerprint_key(&fp2);
        assert_ne!(key1, key2, "内容变化指纹必须不同");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn fingerprint_stable_when_unchanged() {
        let root = test_root("stable");
        write(&root, "notes/文档.md", "# 文档\n内容\n");
        let k1 = fingerprint_key(&fingerprint(&root));
        let k2 = fingerprint_key(&fingerprint(&root));
        assert_eq!(k1, k2, "未变化时指纹稳定");
        fs::remove_dir_all(&root).unwrap();
    }
}
