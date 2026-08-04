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
