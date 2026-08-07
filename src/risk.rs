//! 风控预警（纯规则、零 LLM）：针对律师项目模板的确定性检查——
//! 时效到期（deadline ≤ N 天）、证据缺口（状态含「待补」）、案件信息缺失（frontmatter 关键字段空）。
//! 数据源：项目内 notes/ 下的 .md（type: case / evidence / timeline 由图谱同款推断），
//! 输出供心跳挂载（状态行/徽标/面板提示）+ LLM 工具（risk.check）问答。
//! 设计原则：规则先行、可解释、零 token；LLM 只在用户主动问「案件有什么风险」时经工具消费。

use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Default)]
pub struct RiskItem {
    pub kind: String,   // deadline | evidence_gap | info_missing
    pub case: String,   // 案件文档相对路径（或标题）
    pub label: String,  // 中文摘要（直接展示给用户）
    pub days: Option<i64>, // 距今天数（时效类；负=已过期）
    pub path: String,   // 具体来源文件路径（点击跳图谱）
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RiskReport {
    pub items: Vec<RiskItem>,
    pub deadlines: usize,
    pub evidence_gaps: usize,
    pub info_missing: usize,
}

/// 轻量摘要（心跳挂载进 hb_status，供状态行/徽标即时提示；详情走 /api/risk）
#[derive(Debug, Clone, Serialize, Default)]
pub struct RiskBrief {
    pub deadlines: usize,
    pub evidence_gaps: usize,
    pub info_missing: usize,
    pub urgent: usize, // 已过期 + 7 天内 = 紧急数
}

impl RiskReport {
    pub fn brief(&self) -> RiskBrief {
        RiskBrief {
            deadlines: self.deadlines,
            evidence_gaps: self.evidence_gaps,
            info_missing: self.info_missing,
            urgent: self
                .items
                .iter()
                .filter(|i| i.kind == "deadline" && i.days.unwrap_or(8) <= 7)
                .count(),
        }
    }
}

/// 关键信息缺失检查字段（案件总览 frontmatter / 基本信息）
const KEY_FIELDS: &[&str] = &["案号", "对方当事人", "委托方", "案由"];

/// 扫描 kb 根（含项目隔离目录）：找 type: case 的笔记 → 关联证据/时间线 → 规则判定
pub fn scan(root: &Path) -> RiskReport {
    let mut report = RiskReport::default();
    // 收集全部 .md（排除 pending/sessions；含 projects/ 项目隔离区）
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let walk = ignore::WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|e| {
            let n = e.file_name();
            n != "pending" && n != "sessions"
        })
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
        files.push(p.to_path_buf());
    }

    // 1) 时效：frontmatter deadline 字段（案件总览）或时间线「即将到来的期限」列表
    for f in &files {
        let rel = rel_of(root, f);
        let Ok(content) = std::fs::read_to_string(f) else { continue };
        let (meta, body) = crate::kb::parse_frontmatter(&content);
        let typ = crate::graph::infer_type(&rel, meta.get("type").map(|s| s.as_str()));
        // 仅案件类检查时效（case 或文件名含「案件」）
        if typ == "case" || rel.contains("案件") {
            if let Some(dl) = meta.get("deadline").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
                if let Some(days) = days_until(&dl) {
                    let (level, label) = if days < 0 {
                        ("已过期", format!("⏰ 时效已过期 {} 天：{}", -days, dl))
                    } else if days <= 7 {
                        ("紧急", format!("⏰ 时效紧急：{} 天后到期（{}）", days, dl))
                    } else if days <= 30 {
                        ("临近", format!("⏰ 时效临近：{} 天后到期（{}）", days, dl))
                    } else {
                        continue; // >30 天不预警
                    };
                    let _ = level;
                    report.items.push(RiskItem {
                        kind: "deadline".into(),
                        case: rel.clone(),
                        label,
                        days: Some(days),
                        path: rel.clone(),
                    });
                }
            }
            // 信息缺失：frontmatter/正文基本信息缺关键字段
            let mut missing: Vec<&str> = Vec::new();
            for k in KEY_FIELDS {
                let has = meta.get(*k).map(|v| !v.trim().is_empty()).unwrap_or(false)
                    || body.contains(k);
                if !has {
                    missing.push(k);
                }
            }
            if !missing.is_empty() {
                report.items.push(RiskItem {
                    kind: "info_missing".into(),
                    case: rel.clone(),
                    label: format!("📋 案件信息缺失：{}", missing.join("、")),
                    days: None,
                    path: rel.clone(),
                });
                report.info_missing += 1;
            }
        }
        // 2) 证据缺口：证据清单表格数据行状态列含「待补」；跳过表头/分隔行（表头含「待补」字样会误报）
        if typ == "evidence" || rel.contains("证据") {
            for line in body.lines() {
                let t = line.trim();
                if t.starts_with('|') && (t.starts_with("| 编号") || t.starts_with("|---") || t.starts_with("|--")) {
                    continue; // 表头/分隔行
                }
                if line.contains("待补") || line.contains("缺失") || line.contains("待取") {
                    report.items.push(RiskItem {
                        kind: "evidence_gap".into(),
                        case: rel.clone(),
                        label: format!("📎 证据待补：{}", line.trim().chars().take(40).collect::<String>()),
                        days: None,
                        path: rel.clone(),
                    });
                    report.evidence_gaps += 1;
                }
            }
        }
    }
    report.deadlines = report.items.iter().filter(|i| i.kind == "deadline").count();
    report
}

fn rel_of(root: &Path, f: &Path) -> String {
    f.strip_prefix(root)
        .unwrap_or(f)
        .to_string_lossy()
        .replace('\\', "/")
}

/// 计算 YYYY-MM-DD 距今天数（解析失败返回 None）
fn days_until(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.trim().split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let y: i64 = parts[0].parse().ok()?;
    let m: i64 = parts[1].parse().ok()?;
    let d: i64 = parts[2].parse().ok()?;
    let today = chrono::Local::now().date_naive();
    let target = chrono::NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32)?;
    Some((target - today).num_days())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("md-agent-ut-risk-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    fn write(root: &std::path::Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    #[test]
    fn deadline_detection_within_30_days() {
        let root = test_root("dl");
        let today = chrono::Local::now().date_naive();
        let soon = (today + chrono::Duration::days(10)).format("%Y-%m-%d").to_string();
        let far = (today + chrono::Duration::days(90)).format("%Y-%m-%d").to_string();
        write(&root, "projects/law1/notes/案件总览.md", &format!(
            "---\ntype: case\ntitle: 张案\ndeadline: {soon}\n案号: (2026)民初123号\n对方当事人: 李四\n委托方: 张三\n案由: 合同纠纷\n---\n# 案件总览\n"
        ));
        write(&root, "projects/law1/notes/案件总览2.md", &format!(
            "---\ntype: case\ntitle: 远案\ndeadline: {far}\n案号: x\n对方当事人: y\n委托方: z\n案由: w\n---\n"
        ));
        let r = scan(&root);
        let dl: Vec<_> = r.items.iter().filter(|i| i.kind == "deadline").collect();
        assert_eq!(dl.len(), 1, "90 天后不预警");
        assert_eq!(dl[0].days, Some(10));
        assert!(dl[0].label.contains("时效临近"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn overdue_and_urgent_levels() {
        let root = test_root("od");
        let today = chrono::Local::now().date_naive();
        let past = (today - chrono::Duration::days(3)).format("%Y-%m-%d").to_string();
        let urgent = (today + chrono::Duration::days(3)).format("%Y-%m-%d").to_string();
        write(&root, "projects/law1/notes/案件总览.md", &format!(
            "---\ntype: case\ncase: 过期案\ndeadline: {past}\n案号: a\n对方当事人: b\n委托方: c\n案由: d\n---\n"
        ));
        write(&root, "projects/law1/notes/案件总览2.md", &format!(
            "---\ntype: case\ncase: 紧急案\ndeadline: {urgent}\n案号: a\n对方当事人: b\n委托方: c\n案由: d\n---\n"
        ));
        let r = scan(&root);
        let labels: Vec<String> = r.items.iter().filter(|i| i.kind == "deadline").map(|i| i.label.clone()).collect();
        assert!(labels.iter().any(|l| l.contains("已过期")));
        assert!(labels.iter().any(|l| l.contains("时效紧急")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn evidence_gap_and_info_missing() {
        let root = test_root("gap");
        write(&root, "projects/law1/notes/案件总览.md",
            "---\ntype: case\ntitle: 张案\n---\n# 案件总览\n## 基本信息\n- 案由：合同纠纷\n");
        // 案号/对方当事人/委托方 缺失 → info_missing
        write(&root, "projects/law1/notes/证据清单.md",
            "# 证据清单\n\n| 编号 | 证据名称 | 状态 |\n| 1 | 合同 | 待补 |\n");
        let r = scan(&root);
        assert!(r.items.iter().any(|i| i.kind == "evidence_gap" && i.label.contains("待补")));
        assert!(r.items.iter().any(|i| i.kind == "info_missing"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
