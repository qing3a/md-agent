//! SkillHub 接入（阶段 4）：hub 注册表 + skillhub.md 索引解析 + source 协议白名单。
//! hub = 轻索引（md 文本，frontmatter type: hub + ## apps 列表），只管"谁有什么、去哪下载"；
//! app 包本体在任意位置（git 仓库 / GitHub archive zip / 本地路径），下载后走 market::install_local 校验落盘。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const HUBS_DIR: &str = "hubs";
const MAX_INDEX_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HubApp {
    pub id: String,
    pub name: String,
    pub version: String,
    pub entry: String,
    pub permissions: Vec<String>,
    pub source: String,
    pub description: String,
    /// 条目类型（索引可声明 type: app|skill，缺省 app；安装时以包内容识别为准）
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubInfo {
    pub name: String,
    pub url: String,
    pub version: String,
    pub apps: Vec<HubApp>,
}

fn file_safe(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

fn valid_app_id(id: &str) -> bool {
    file_safe(id)
}

/// source 协议白名单：local:（本地路径）/ git+https://（git clone）/ https://…zip（GitHub archive 等）/ https 直连；
/// 裸 http:// 一律拒绝（第三方 hub 索引不可信，协议白名单是第一道闸）；http://localhost 仅放行本地 mock 测试。
pub fn valid_source(src: &str) -> bool {
    let s = src.trim();
    if s.starts_with("local:") || s.starts_with("git+https://") || s.starts_with("https://") {
        return true;
    }
    s.starts_with("http://localhost") || s.starts_with("http://127.0.0.1")
}

/// hub 连接 URL 协议：https，或 http://localhost/127.0.0.1（本地 mock skillhub.md 验证链路用）
fn valid_hub_url(url: &str) -> bool {
    let u = url.trim();
    u.starts_with("https://") || u.starts_with("http://localhost") || u.starts_with("http://127.0.0.1")
}

/// 解析 skillhub.md 索引文本：frontmatter（type: hub / name / version）+ ## apps 列表。
/// 防御式：type 非 hub 或缺 name → 整体失败；条目缺 id / id 非法 / source 非法 → 跳过该条（不拖垮整个 hub）。
pub fn parse_hub_index(text: &str, url: &str) -> Result<HubInfo, String> {
    let mut fm_type: Option<String> = None;
    let mut fm_name: Option<String> = None;
    let mut fm_version: Option<String> = None;
    let mut in_fm = false;
    let mut fm_seen = false;
    let mut apps_section = false;
    let mut cur: Option<HubApp> = None;
    let mut apps: Vec<HubApp> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line == "---" {
            if !in_fm {
                in_fm = true;
                continue;
            }
            in_fm = false;
            fm_seen = true;
            continue;
        }
        if in_fm {
            if let Some((k, v)) = line.split_once(':') {
                let (k, v) = (k.trim(), v.trim());
                match k {
                    "type" => fm_type = Some(v.to_string()),
                    "name" => fm_name = Some(v.to_string()),
                    "version" => fm_version = Some(v.to_string()),
                    _ => {}
                }
            }
            continue;
        }
        if !fm_seen {
            // 前置内容（frontmatter 前）忽略
            continue;
        }
        if line == "## apps" {
            apps_section = true;
            continue;
        }
        if !apps_section {
            continue;
        }
        if let Some(rest) = line.strip_prefix("- ") {
            // 新条目
            if let Some(a) = cur.take() {
                push_valid_app(&mut apps, a);
            }
            cur = Some(HubApp::default());
            apply_field(cur.as_mut().unwrap(), rest);
        } else if let Some(a) = cur.as_mut() {
            apply_field(a, line);
        }
    }
    if let Some(a) = cur.take() {
        push_valid_app(&mut apps, a);
    }

    if fm_type.as_deref() != Some("hub") {
        return Err("不是合法的 SkillHub 索引（缺 type: hub）".to_string());
    }
    let name = fm_name.ok_or("hub 索引缺 name 字段".to_string())?;
    if !file_safe(&name) {
        return Err(format!("非法 hub 名: {name}"));
    }
    Ok(HubInfo {
        name,
        url: url.to_string(),
        version: fm_version.unwrap_or_else(|| "0".to_string()),
        apps,
    })
}

fn push_valid_app(apps: &mut Vec<HubApp>, mut a: HubApp) {
    if !valid_app_id(&a.id) {
        return; // 缺 id / 非法 id → 跳过
    }
    if a.source.is_empty() || !valid_source(&a.source) {
        return; // source 缺失或不合法（裸 http 等）→ 跳过
    }
    if a.name.is_empty() {
        a.name = a.id.clone();
    }
    if a.entry.is_empty() {
        a.entry = "index.html".to_string();
    }
    if a.kind.is_empty() {
        a.kind = "app".to_string();
    }
    apps.push(a);
}

fn apply_field(a: &mut HubApp, line: &str) {
    let Some((k, v)) = line.split_once(':') else { return };
    let (k, v) = (k.trim(), v.trim());
    match k {
        "id" => a.id = v.to_string(),
        "name" => a.name = v.to_string(),
        "version" => a.version = v.to_string(),
        "entry" => a.entry = v.to_string(),
        "description" => a.description = v.to_string(),
        "source" => a.source = v.to_string(),
        "type" => a.kind = v.to_string(),
        "permissions" => {
            a.permissions = v
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|x| x.trim().trim_matches('"').trim_matches('\'').to_string())
                .filter(|x| !x.is_empty())
                .collect();
        }
        _ => {}
    }
}

/// 拉取 hub 索引文本（HTTP GET，限 512KB，30s 超时）
async fn fetch_index(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("md-agent/0.1 (skillhub client)")
        .build()
        .map_err(|e| format!("构建客户端失败: {e}"))?;
    let resp = client.get(url).send().await.map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("读取失败: {e}"))?;
    let bytes = &bytes[..bytes.len().min(MAX_INDEX_BYTES)];
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

/// 连接 hub：拉取索引 → 解析校验 → 存入 kb/hubs/<name>.json
pub async fn connect_hub(root: &Path, url: &str) -> Result<HubInfo, String> {
    let u = url.trim();
    if !valid_hub_url(u) {
        return Err("hub URL 仅支持 https（或本地 http://localhost mock）".to_string());
    }
    let text = fetch_index(u).await?;
    let hub = parse_hub_index(&text, u)?;
    let dir = root.join(HUBS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&hub).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}.json", hub.name)), json).map_err(|e| e.to_string())?;
    Ok(hub)
}

/// 已连接 hub 列表（读 kb/hubs/*.json）
pub fn list_hubs(root: &Path) -> Vec<HubInfo> {
    let dir = root.join(HUBS_DIR);
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else { return out };
    for e in rd.flatten() {
        let Ok(content) = std::fs::read_to_string(e.path()) else { continue };
        if let Ok(h) = serde_json::from_str::<HubInfo>(&content) {
            out.push(h);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 刷新 hub 索引：先拉新（失败返回 Err，旧索引文件不动——降级不丢目录），成功再覆盖写
pub async fn refresh_hub(root: &Path, name: &str) -> Result<HubInfo, String> {
    if !file_safe(name) {
        return Err(format!("非法 hub 名: {name}"));
    }
    let dir = root.join(HUBS_DIR);
    let file = dir.join(format!("{name}.json"));
    let old = std::fs::read_to_string(&file).map_err(|e| format!("hub 未连接: {e}"))?;
    let old_hub: HubInfo = serde_json::from_str(&old).map_err(|e| e.to_string())?;
    let text = fetch_index(&old_hub.url).await?;
    let hub = parse_hub_index(&text, &old_hub.url)?;
    let json = serde_json::to_string_pretty(&hub).map_err(|e| e.to_string())?;
    std::fs::write(&file, json).map_err(|e| e.to_string())?;
    Ok(hub)
}

/// 断开 hub：删除注册表文件（不删已安装的 app——数据一体，已装 app 是本地权威副本）
pub fn disconnect_hub(root: &Path, name: &str) -> Result<(), String> {
    if !file_safe(name) {
        return Err(format!("非法 hub 名: {name}"));
    }
    let file = root.join(HUBS_DIR).join(format!("{name}.json"));
    if !file.is_file() {
        return Err(format!("hub 未连接: {name}"));
    }
    std::fs::remove_file(&file).map_err(|e| e.to_string())
}

/// 可抓取的 HTTP 源：https 或本地 mock（http://localhost / http://127.0.0.1）
fn fetchable(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://localhost") || s.starts_with("http://127.0.0.1")
}

/// 按 source 下载 app 包到临时目录（tmp_root 由调用方创建并负责清理），返回 app 包根目录。
/// 支持：local:<路径>（本地目录，直接引用）/ git+https://<url>[#<子目录>]（git clone --depth 1）/
/// https://…zip（下载 + PowerShell Expand-Archive，解压后自动定位含 app.json 的目录）/
/// https://…md 裸技能文件（直接下载文本）
pub async fn download_app(source: &str, tmp_root: &Path) -> Result<PathBuf, String> {
    let s = source.trim();
    if let Some(p) = s.strip_prefix("local:") {
        let pb = PathBuf::from(p);
        if !pb.is_dir() {
            return Err(format!("本地目录不存在: {p}"));
        }
        return Ok(pb);
    }
    if let Some(rest) = s.strip_prefix("git+") {
        let (url, sub) = match rest.split_once('#') {
            Some((u, sub)) => (u, Some(sub)),
            None => (rest, None),
        };
        let dir = tmp_root.join("src");
        let status = std::process::Command::new("git")
            .args(["clone", "--depth", "1", "--quiet", url])
            .arg(&dir)
            .status()
            .map_err(|e| format!("git 命令执行失败: {e}"))?;
        if !status.success() {
            return Err(format!("git clone 失败: {url}"));
        }
        let target = match sub {
            Some(s) => dir.join(s),
            None => dir,
        };
        if !target.is_dir() {
            return Err(format!("仓库内找不到子目录: {}", sub.unwrap_or("")));
        }
        return Ok(target);
    }
    if fetchable(s) && s.ends_with(".zip") {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .user_agent("md-agent/0.1 (skillhub client)")
            .build()
            .map_err(|e| format!("构建客户端失败: {e}"))?;
        let resp = client.get(s).send().await.map_err(|e| format!("下载失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        let bytes = resp.bytes().await.map_err(|e| format!("读取失败: {e}"))?;
        let zip_path = tmp_root.join("pkg.zip");
        std::fs::write(&zip_path, &bytes).map_err(|e| e.to_string())?;
        let out = tmp_root.join("pkg");
        let ps = format!(
            "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
            zip_path.display(),
            out.display()
        );
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps])
            .status()
            .map_err(|e| format!("powershell 解压失败: {e}"))?;
        if !status.success() {
            return Err("zip 解压失败".to_string());
        }
        return Ok(find_app_dir(&out));
    }
    // 裸技能文件（https://…md 或本地 mock）：直接下载文本，返回文件路径（install_bundle 识别为技能）
    if fetchable(s) && s.ends_with(".md") {
        std::fs::create_dir_all(tmp_root).map_err(|e| e.to_string())?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("md-agent/0.1 (skillhub client)")
            .build()
            .map_err(|e| format!("构建客户端失败: {e}"))?;
        let resp = client.get(s).send().await.map_err(|e| format!("下载失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        let bytes = resp.bytes().await.map_err(|e| format!("读取失败: {e}"))?;
        let bytes = &bytes[..bytes.len().min(MAX_INDEX_BYTES)];
        let md_path = tmp_root.join("SKILL.md");
        std::fs::write(&md_path, String::from_utf8_lossy(bytes).as_bytes()).map_err(|e| e.to_string())?;
        return Ok(md_path);
    }
    Err("不支持的 source 协议（仅 local: / git+https: / https zip / https md）".to_string())
}

/// 解压产物定位 app 包根：根含 app.json 则用根，否则扫一层子目录（GitHub archive 有顶层 repo-commit/ 目录）
fn find_app_dir(out: &Path) -> PathBuf {
    if out.join("app.json").is_file() {
        return out.to_path_buf();
    }
    if let Ok(rd) = std::fs::read_dir(out) {
        for e in rd.flatten() {
            if e.path().is_dir() && e.path().join("app.json").is_file() {
                return e.path();
            }
        }
    }
    out.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("md-agent-hub-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    const FULL_INDEX: &str = r#"---
type: hub
name: skillhub.cn
version: 2
---

# SkillHub 商店

## apps
- id: match
  name: 相亲工作台
  version: 0.3.1
  entry: index.html
  permissions: [llm, file]
  source: git+https://github.com/user/match-app
- id: calendar
  name: 日历
  source: https://github.com/user/cal-app/archive/refs/tags/v1.0.zip
"#;

    #[test]
    fn parse_full_index() {
        let h = parse_hub_index(FULL_INDEX, "https://skillhub.cn/install/skillhub.md").unwrap();
        assert_eq!(h.name, "skillhub.cn");
        assert_eq!(h.version, "2");
        assert_eq!(h.apps.len(), 2);
        assert_eq!(h.apps[0].id, "match");
        assert_eq!(h.apps[0].permissions, vec!["llm", "file"]);
        assert_eq!(h.apps[0].source, "git+https://github.com/user/match-app");
        assert_eq!(h.apps[1].entry, "index.html"); // 缺 entry 补默认
        assert_eq!(h.apps[1].name, "日历"); // 显式 name 原样保留
    }

    #[test]
    fn parse_rejects_non_hub_type() {
        let txt = "---\ntype: skill\ntitle: x\n---\n# hi\n";
        assert!(parse_hub_index(txt, "https://x/install.md").is_err());
    }

    #[test]
    fn parse_rejects_missing_name() {
        let txt = "---\ntype: hub\n---\n## apps\n";
        assert!(parse_hub_index(txt, "https://x/install.md").is_err());
    }

    #[test]
    fn parse_skips_bad_entries() {
        // 裸 http source 与非法 id 条目被跳过，合法条目保留
        let txt = r#"---
type: hub
name: h
---
## apps
- id: ok
  source: local:/tmp/x
- id: httpapp
  source: http://evil.example/app.zip
- id: ../evil
  source: git+https://github.com/a/b
"#;
        let h = parse_hub_index(txt, "https://h/install.md").unwrap();
        assert_eq!(h.apps.len(), 1);
        assert_eq!(h.apps[0].id, "ok");
    }

    #[test]
    fn source_whitelist() {
        assert!(valid_source("local:C:/tmp/app"));
        assert!(valid_source("git+https://github.com/a/b"));
        assert!(valid_source("https://github.com/a/b/archive/v1.zip"));
        assert!(!valid_source("http://evil.example/a.zip"));
        assert!(!valid_source("ftp://x/y"));
        assert!(valid_source("http://localhost:8756/apps/match"));
    }

    #[test]
    fn connect_persists_and_disconnect_guards() {
        let root = tmp("root");
        let f = root.join(HUBS_DIR).join("skillhub.cn.json");
        fs::create_dir_all(root.join(HUBS_DIR)).unwrap();
        fs::write(&f, serde_json::to_string(&parse_hub_index(FULL_INDEX, "https://skillhub.cn/install/skillhub.md").unwrap()).unwrap()).unwrap();
        assert_eq!(list_hubs(&root).len(), 1);
        assert_eq!(list_hubs(&root)[0].apps.len(), 2);
        assert!(disconnect_hub(&root, "../evil").is_err());
        disconnect_hub(&root, "skillhub.cn").unwrap();
        assert_eq!(list_hubs(&root).len(), 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn download_local_path() {
        let root = tmp("local");
        let app = root.join("appdir");
        fs::create_dir_all(&app).unwrap();
        let tmpd = tmp("tmpd");
        let got = tokio_test_block_on(download_app(&format!("local:{}", app.display()), &tmpd)).unwrap();
        assert_eq!(got, app);
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&tmpd).unwrap();
    }

    fn tokio_test_block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }
}
