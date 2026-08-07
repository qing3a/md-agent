//! 项目制（多项目硬隔离，完全隔离 MVP）。
//!
//! 数据模型：
//! - 全局层 = kb_root 本身（系统配置、应用/技能/市场 + 「个人空间」默认项目，零迁移）。
//! - 每个项目 = `kb_root/projects/<id>/` 下的独立迷你 kb：自己的 L1 规范、notes、sessions、
//!   pending、图谱/活动/任务三库（各自的 .db）。
//! - 默认项目「个人空间」= 全局 kb 根（project 参数为空/"default" 时即全局根）。
//!
//! 隔离由「root 指向」保证：底层全部以 `root: &Path` 参数化的函数（search/graph/activity/kb…）
//! 传入项目根后天然互不可见——项目 A 的检索/会话/记忆绝不会带出项目 B。
//!
//! 模板内容内嵌随 exe 分发（include_str!），保持单文件形态。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PROJECTS_DIR: &str = "projects";
pub const META_FILE: &str = "meta.json";

/// 内置模板白名单（创建项目时校验；模板文件在 TEMPLATE_FILES 中定义）
pub const TEMPLATES: &[&str] = &["blank", "lawyer", "headhunter"];

/// 模板内容表：(template, 相对项目根的路径, 内嵌内容)。随 exe 编译进单文件。
pub const TEMPLATE_FILES: &[(&str, &str, &str)] = &[
    ("blank", "MEMORY.md", include_str!("templates/projects/blank/MEMORY.md")),
    ("blank", "KB.md", include_str!("templates/projects/blank/KB.md")),
    ("blank", "FRAMEWORK.md", include_str!("templates/projects/blank/FRAMEWORK.md")),
    ("blank", "RULES.md", include_str!("templates/projects/blank/RULES.md")),
    ("blank", "notes/项目说明.md", include_str!("templates/projects/blank/notes/项目说明.md")),
    // 律师模板：案件建档/证据/时间线/法律研究
    ("lawyer", "MEMORY.md", include_str!("templates/projects/lawyer/MEMORY.md")),
    ("lawyer", "KB.md", include_str!("templates/projects/lawyer/KB.md")),
    ("lawyer", "FRAMEWORK.md", include_str!("templates/projects/lawyer/FRAMEWORK.md")),
    ("lawyer", "RULES.md", include_str!("templates/projects/lawyer/RULES.md")),
    ("lawyer", "notes/案件总览.md", include_str!("templates/projects/lawyer/notes/案件总览.md")),
    ("lawyer", "notes/证据清单.md", include_str!("templates/projects/lawyer/notes/证据清单.md")),
    ("lawyer", "notes/时间线.md", include_str!("templates/projects/lawyer/notes/时间线.md")),
    ("lawyer", "notes/法律研究.md", include_str!("templates/projects/lawyer/notes/法律研究.md")),
    ("lawyer", "notes/当事人与诉求.md", include_str!("templates/projects/lawyer/notes/当事人与诉求.md")),
    // 猎头模板：职位/候选人/客户/沟通
    ("headhunter", "MEMORY.md", include_str!("templates/projects/headhunter/MEMORY.md")),
    ("headhunter", "KB.md", include_str!("templates/projects/headhunter/KB.md")),
    ("headhunter", "FRAMEWORK.md", include_str!("templates/projects/headhunter/FRAMEWORK.md")),
    ("headhunter", "RULES.md", include_str!("templates/projects/headhunter/RULES.md")),
    ("headhunter", "notes/职位需求.md", include_str!("templates/projects/headhunter/notes/职位需求.md")),
    ("headhunter", "notes/候选人.md", include_str!("templates/projects/headhunter/notes/候选人.md")),
    ("headhunter", "notes/客户公司.md", include_str!("templates/projects/headhunter/notes/客户公司.md")),
    ("headhunter", "notes/沟通记录.md", include_str!("templates/projects/headhunter/notes/沟通记录.md")),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub id: String,
    pub name: String,
    pub template: String,
    pub created: i64,
}

pub fn projects_dir(root: &Path) -> PathBuf {
    root.join(PROJECTS_DIR)
}

/// 项目目录：kb_root/projects/<id>（id 须先过 validate_id）
pub fn project_dir(root: &Path, id: &str) -> PathBuf {
    projects_dir(root).join(id)
}

pub fn meta_path(root: &Path, id: &str) -> PathBuf {
    project_dir(root, id).join(META_FILE)
}

/// id 合法性：仅 [A-Za-z0-9_-]，防路径注入
pub fn validate_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn load_meta(root: &Path, id: &str) -> Option<ProjectMeta> {
    let s = fs::read_to_string(meta_path(root, id)).ok()?;
    serde_json::from_str(&s).ok()
}

/// 列出全部项目（按创建时间倒序；「个人空间」默认项目由前端合成，不在此列）
pub fn list_projects(root: &Path) -> Vec<ProjectMeta> {
    let mut out: Vec<ProjectMeta> = Vec::new();
    let Ok(entries) = fs::read_dir(projects_dir(root)) else {
        return out;
    };
    for e in entries.flatten() {
        if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let id = e.file_name().to_string_lossy().to_string();
        if let Some(m) = load_meta(root, &id) {
            out.push(m);
        }
    }
    out.sort_by(|a, b| b.created.cmp(&a.created));
    out
}

/// 创建项目：目录 = projects/<时间戳>（同秒碰撞时加 -n 后缀）；模板内容内嵌复制；meta.json 落盘。
pub fn create_project(root: &Path, name: &str, template: &str) -> Result<ProjectMeta, String> {
    if !TEMPLATES.contains(&template) {
        return Err(format!("未知模板: {template}（可选: {}）", TEMPLATES.join(", ")));
    }
    let name = name.trim();
    if name.is_empty() {
        return Err("项目名称不能为空".to_string());
    }
    let now = now_secs();
    let mut id = now.to_string();
    let mut n = 0;
    while project_dir(root, &id).exists() {
        n += 1;
        id = format!("{now}-{n}");
    }
    let dir = project_dir(root, &id);
    fs::create_dir_all(&dir)
        .map_err(|e| format!("创建项目目录失败: {e}"))?;
    // 内嵌模板复制（中文 L1 + notes 初始结构）
    for (tpl, rel, content) in TEMPLATE_FILES {
        if *tpl != template {
            continue;
        }
        let dst = dir.join(rel);
        if let Some(parent) = dst.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&dst, content).map_err(|e| format!("写入 {rel} 失败: {e}"))?;
    }
    // 补最小结构（notes/skills 目录 + INDEX 占位；已由模板写的 L1 文件 ensure_layout 会跳过）
    let _ = crate::kb::ensure_layout(&dir);
    let meta = ProjectMeta {
        id: id.clone(),
        name: name.to_string(),
        template: template.to_string(),
        created: now,
    };
    fs::write(
        meta_path(root, &id),
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写 meta.json 失败: {e}"))?;
    Ok(meta)
}

/// 删除项目（默认项目「个人空间」不可删；仅限 projects/ 内目录，防逃逸）
pub fn delete_project(root: &Path, id: &str) -> Result<(), String> {
    if id.is_empty() || id == "default" {
        return Err("默认项目「个人空间」不可删除".to_string());
    }
    if !validate_id(id) {
        return Err("非法项目 id".to_string());
    }
    let dir = project_dir(root, id);
    if !dir.is_dir() {
        return Err(format!("项目不存在: {id}"));
    }
    if !dir.starts_with(projects_dir(root)) {
        return Err("路径逃逸".to_string());
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("删除项目失败: {e}"))
}

/// 重命名项目（改 meta.json 的 name）
pub fn rename_project(root: &Path, id: &str, name: &str) -> Result<ProjectMeta, String> {
    let mut meta = load_meta(root, id).ok_or_else(|| format!("项目不存在: {id}"))?;
    let name = name.trim();
    if name.is_empty() {
        return Err("项目名称不能为空".to_string());
    }
    meta.name = name.to_string();
    fs::write(
        meta_path(root, id),
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("更新 meta.json 失败: {e}"))?;
    Ok(meta)
}

/// 解析项目根：None/空/"default" → 全局 kb 根（个人空间）；否则 projects/<id>（校验存在 + 防逃逸）。
/// 这是项目级 API 的唯一切换入口——传入的 root 决定了检索/会话/记忆的隔离边界。
pub fn resolve_project_root(root: &Path, project: Option<&str>) -> Result<PathBuf, String> {
    let pid = project.map(|s| s.trim()).filter(|s| !s.is_empty());
    match pid {
        None | Some("default") => Ok(root.to_path_buf()),
        Some(id) => {
            if !validate_id(id) {
                return Err(format!("非法项目 id: {id}"));
            }
            let dir = project_dir(root, id);
            if !dir.is_dir() {
                return Err(format!("项目不存在: {id}"));
            }
            Ok(dir)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("md-agent-ut-proj-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn create_list_resolve_roundtrip() {
        let root = test_root("roundtrip");
        let m = create_project(&root, "张先生劳动仲裁", "blank").unwrap();
        assert_eq!(m.name, "张先生劳动仲裁");
        assert_eq!(m.template, "blank");
        assert!(validate_id(&m.id));
        // 模板已落盘
        assert!(project_dir(&root, &m.id).join("MEMORY.md").is_file());
        assert!(project_dir(&root, &m.id).join("notes/项目说明.md").is_file());
        // 列表与解析
        let list = list_projects(&root);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, m.id);
        let r = resolve_project_root(&root, Some(&m.id)).unwrap();
        assert_eq!(r, project_dir(&root, &m.id));
        // default/None → 全局根
        assert_eq!(resolve_project_root(&root, None).unwrap(), root);
        assert_eq!(resolve_project_root(&root, Some("default")).unwrap(), root);
        // 不存在 → Err
        assert!(resolve_project_root(&root, Some("nope")).is_err());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn validate_and_escape() {
        assert!(validate_id("abc-123_XYZ"));
        assert!(!validate_id("../evil"));
        assert!(!validate_id("a/b"));
        assert!(!validate_id(""));
    }

    #[test]
    fn delete_protects_default_and_missing() {
        let root = test_root("delete");
        assert!(delete_project(&root, "").is_err());
        assert!(delete_project(&root, "default").is_err());
        assert!(delete_project(&root, "ghost").is_err());
        let m = create_project(&root, "案件一", "blank").unwrap();
        assert!(delete_project(&root, &m.id).is_ok());
        assert!(!project_dir(&root, &m.id).exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rename_and_unknown_template() {
        let root = test_root("rename");
        assert!(create_project(&root, "x", "nonexistent").is_err());
        let m = create_project(&root, "旧名", "blank").unwrap();
        let r = rename_project(&root, &m.id, "新名").unwrap();
        assert_eq!(r.name, "新名");
        assert_eq!(load_meta(&root, &m.id).unwrap().name, "新名");
        assert!(rename_project(&root, &m.id, "   ").is_err());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn isolation_between_projects() {
        // 硬隔离的根：不同项目各自独立目录，同名文件互不可见
        let root = test_root("isolation");
        let a = create_project(&root, "项目A", "blank").unwrap();
        let b = create_project(&root, "项目B", "blank").unwrap();
        let da = project_dir(&root, &a.id);
        let db = project_dir(&root, &b.id);
        fs::write(da.join("notes/机密.md"), "# 机密\nA 的绝密资料").unwrap();
        assert!(!db.join("notes/机密.md").exists()); // B 里没有该文件
        assert!(da.join("notes/机密.md").exists());
        fs::remove_dir_all(&root).ok();
    }

    /// 硬隔离核心回归：全局检索/图谱绝不命中 projects/ 内容；项目检索只命中本项目
    #[test]
    fn global_scope_excludes_project_content() {
        let root = test_root("globalscope");
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::write(root.join("notes/全局.md"), "# 全局\n公共资料").unwrap();
        let a = create_project(&root, "案件A", "blank").unwrap();
        let da = project_dir(&root, &a.id);
        fs::write(da.join("notes/机密.md"), "# 机密\n绝密内容不得外泄").unwrap();

        // 全局检索（无项目指向）不得命中项目内容
        let g = crate::search::search(&root, "绝密", "all", false).unwrap();
        assert_eq!(g.hit_count, 0, "全局检索泄漏了项目内容: {:?}", g.hits.iter().map(|h| h.file.clone()).collect::<Vec<_>>());
        let g2 = crate::search::search(&root, "公共", "all", false).unwrap();
        assert!(g2.hit_count > 0);

        // 项目内检索命中本项目内容
        let p = crate::search::search(&da, "绝密", "all", false).unwrap();
        assert!(p.hit_count > 0, "项目内检索未命中本项目内容");

        // 全局图谱不含项目文档
        let rep = crate::graph::sync_graph(&root).unwrap();
        let stats = crate::graph::stats(&root).unwrap();
        let docs_json = serde_json::to_value(&stats).unwrap();
        let docs = docs_json["docs"].as_u64().unwrap_or(0);
        let projects_claimed = serde_json::to_value(crate::graph::projects(&root).unwrap()).unwrap();
        // 全局图谱只含全局文档；project 统计不应出现业务项目 id（projects/ 被排除）
        let s = projects_claimed.to_string();
        assert!(!s.contains(&a.id), "全局图谱混入项目内容: {s}");
        assert!(docs >= 1 && docs <= 2, "全局图谱文档数异常: {docs}（{rep:?}）");
        fs::remove_dir_all(&root).ok();
    }
}
