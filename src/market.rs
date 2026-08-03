//! 应用市场安装/卸载（阶段 2 v1）：本地路径导入（手动导入兜底通道，永远可用）。
//! GitHub 下载通道留 v2（网络不稳，见 .zcode/plans/plan-app-market.md 通道降级链）。

use crate::kb::AppInfo;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// 读取应用目录的 manifest（app.json 校验 + 路径安全）
pub fn read_manifest(dir: &Path) -> Result<AppInfo, String> {
    let mf = dir.join("app.json");
    let content = fs::read_to_string(&mf).map_err(|e| format!("缺少 app.json: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&content).map_err(|e| format!("app.json 解析失败: {e}"))?;
    let id = v.get("id").and_then(|x| x.as_str()).map(String::from).ok_or("app.json 缺 id 字段")?;
    if !valid_id(&id) {
        return Err(format!("非法应用 id: {id}（仅字母数字 ._-）"));
    }
    let entry = v.get("entry").and_then(|x| x.as_str()).unwrap_or("index.html").to_string();
    let entry_path = Path::new(&entry);
    if entry_path.is_absolute() || entry_path.components().any(|c| c == Component::ParentDir) {
        return Err("entry 不能是绝对路径或含 ..".to_string());
    }
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or(&id).to_string();
    let version = v.get("version").and_then(|x| x.as_str()).unwrap_or("0.0.0").to_string();
    let description = v.get("description").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let permissions = v
        .get("permissions")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|p| p.as_str().map(String::from)).collect())
        .unwrap_or_default();
    Ok(AppInfo { id, name, version, entry, permissions, description, source_hub: None })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// 从本地路径安装应用（手动导入通道）：校验 manifest → 复制目录到 kb/apps/<id>/
pub fn install_local(root: &Path, src: &str) -> Result<AppInfo, String> {
    let src_path = PathBuf::from(src);
    if !src_path.is_dir() {
        return Err(format!("来源目录不存在: {src}"));
    }
    let manifest = read_manifest(&src_path)?;
    let dst = root.join("apps").join(&manifest.id);
    if dst.exists() {
        return Err(format!("应用已存在: {}（先 /market uninstall 或 update）", manifest.id));
    }
    fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
    copy_dir(&src_path, &dst)?;
    Ok(manifest)
}

fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    for e in fs::read_dir(src).map_err(|e| e.to_string())? {
        let e = e.map_err(|e| e.to_string())?;
        let from = e.path();
        let to = dst.join(e.file_name());
        if from.is_dir() {
            fs::create_dir_all(&to).map_err(|e| e.to_string())?;
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| format!("复制失败 {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

/// 卸载应用：删除 kb/apps/<id>/（id 校验防目录穿越）
pub fn uninstall(root: &Path, id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err(format!("非法应用 id: {id}"));
    }
    let dir = root.join("apps").join(id);
    if !dir.is_dir() {
        return Err(format!("应用未安装: {id}"));
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("卸载失败: {e}"))?;
    Ok(())
}

/// 更新：v1 = 卸载后重装（简单可靠，走同一校验/复制管道）
pub fn update_local(root: &Path, id: &str, src: &str) -> Result<AppInfo, String> {
    uninstall(root, id)?;
    install_local(root, src)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("md-agent-mkt-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_app(dir: &Path, id: &str) {
        fs::create_dir_all(dir.join(id)).unwrap();
        fs::write(
            dir.join(id).join("app.json"),
            format!(r#"{{"id":"{id}","name":"测试","version":"1.0.0","entry":"index.html","permissions":["llm"]}}"#),
        )
        .unwrap();
        fs::write(dir.join(id).join("index.html"), "<h1>app</h1>").unwrap();
    }

    #[test]
    fn install_local_copies_and_validates() {
        let root = tmp("root");
        let src = tmp("src");
        make_app(&src, "demo");
        let m = install_local(&root, src.join("demo").to_str().unwrap()).unwrap();
        assert_eq!(m.id, "demo");
        assert!(root.join("apps/demo/index.html").is_file());
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&src).unwrap();
    }

    #[test]
    fn install_rejects_bad_manifest() {
        let root = tmp("root2");
        let src = tmp("src2");
        fs::create_dir_all(src.join("bad")).unwrap();
        fs::write(src.join("bad").join("app.json"), r#"{"id":"../evil"}"#).unwrap();
        assert!(install_local(&root, src.join("bad").to_str().unwrap()).is_err());
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&src).unwrap();
    }

    #[test]
    fn uninstall_removes_dir_and_guards() {
        let root = tmp("root3");
        let src = tmp("src3");
        make_app(&src, "gone");
        install_local(&root, src.join("gone").to_str().unwrap()).unwrap();
        assert!(root.join("apps/gone").is_dir());
        uninstall(&root, "gone").unwrap();
        assert!(!root.join("apps/gone").exists());
        assert!(uninstall(&root, "../evil").is_err());
        assert!(uninstall(&root, "nope").is_err());
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&src).unwrap();
    }
}
