//! SkillHub 接入：连接「md 文档集合」源（git 仓库 / GitHub zip / 本地目录 / 旧 skillhub.md 索引兼容）。
//! 客户端扫描并分析集合中的 md 文档（frontmatter：name/description/version/type）自动生成目录——
//! 真实 SkillHub（anthropics/skills、wshobson/agents 等）就是纯 md 文档集合，没有特制索引包。
//! 目录条目带 rel（集合内相对路径），安装时从本地缓存的集合直接取文件（零下载）；
//! 旧 `type: hub` 索引条目保留 source 字段，仍走 download_app 下载通道。

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const HUBS_DIR: &str = "hubs";
const MAX_INDEX_BYTES: usize = 512 * 1024;

/// 集合内非技能文档排除清单（大小写不敏感文件名匹配）
const EXCLUDED_DOCS: [&str; 9] = [
    "readme.md",
    "license",
    "license.txt",
    "third_party_notices.md",
    "index.md",
    "contributing.md",
    "changelog.md",
    "agents.md",
    "claude.md",
];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HubApp {
    pub id: String,
    pub name: String,
    pub version: String,
    pub entry: String,
    pub permissions: Vec<String>,
    pub source: String,
    pub description: String,
    /// 条目类型（skill | app；安装时以包内容识别为准）
    pub kind: String,
    /// 集合内相对路径（连接时分析得出；空 = 旧索引条目，走 source 下载）
    #[serde(default)]
    pub rel: String,
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

/// PowerShell 可执行（服务从 MSYS/其他 PATH 启动时裸 powershell 可能找不到，Windows 用全路径）
fn powershell_exe() -> &'static str {
    if cfg!(target_os = "windows") {
        "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
    } else {
        "powershell"
    }
}

fn valid_app_id(id: &str) -> bool {
    file_safe(id)
}

fn sanitize_name(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '-' })
        .collect()
}

/// source 协议白名单（旧索引条目用）：local: / git+https: / https zip / https md；
/// 裸 http:// 一律拒绝；http://localhost 仅放行本地 mock 测试。
pub fn valid_source(src: &str) -> bool {
    let s = src.trim();
    if s.starts_with("local:") || s.starts_with("git+https://") || s.starts_with("https://") {
        return true;
    }
    s.starts_with("http://localhost") || s.starts_with("http://127.0.0.1")
}

/// hub 连接来源：git+https 仓库 / https zip / https md / local: 目录（或本地 http mock）
fn valid_hub_url(url: &str) -> bool {
    let u = url.trim();
    u.starts_with("git+https://") || u.starts_with("https://") || u.starts_with("local:")
        || u.starts_with("http://localhost") || u.starts_with("http://127.0.0.1")
}

// ==================== md 文档分析（新模式核心） ====================

/// 解析 md 文档 frontmatter（--- 开头的键值对），返回有序字段列表
fn parse_frontmatter(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut in_fm = false;
    let mut closed = false;
    for line in text.lines().take(60) {
        let l = line.trim();
        if !in_fm && l == "---" {
            in_fm = true;
            continue;
        }
        if in_fm {
            if l == "---" {
                closed = true;
                break;
            }
            if let Some((k, v)) = l.split_once(':') {
                let (k, v) = (k.trim(), v.trim().trim_matches('"').trim_matches('\''));
                if !k.is_empty() && !v.is_empty() {
                    out.push((k.to_string(), v.to_string()));
                }
            }
        }
    }
    if closed { out } else { Vec::new() }
}

fn fm_get(fm: &[(String, String)], key: &str) -> Option<String> {
    fm.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
}

/// 目录内是否含 md 文件（zip 根定位用，浅层递归）
fn has_md(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if has_md(&p) {
                return true;
            }
        } else if p.extension().map(|x| x == "md").unwrap_or(false) {
            return true;
        }
    }
    false
}

/// 收集含 app.json 的应用目录（跳过 .git）
fn collect_app_dirs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        if p.file_name().and_then(|n| n.to_str()) == Some(".git") {
            continue;
        }
        if p.join("app.json").is_file() {
            out.push(p);
        } else {
            collect_app_dirs(&p, out);
        }
    }
}

fn walk_md_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().and_then(|n| n.to_str()) != Some(".git") {
                walk_md_files(&p, out);
            }
        } else if p.extension().map(|x| x == "md").unwrap_or(false) {
            out.push(p);
        }
    }
}

fn is_within(path: &Path, ancestor: &Path) -> bool {
    path.strip_prefix(ancestor).is_ok()
}

/// 分析文档集合：应用目录（app.json）+ 技能 md（SKILL.md / 裸 md frontmatter）→ 目录条目。
/// 排除：README/LICENSE/INDEX 等导航文档、无 frontmatter 的普通文档、应用目录内的 md。
pub fn analyze_collection(root: &Path) -> Vec<HubApp> {
    let mut out: Vec<HubApp> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // 1. 应用：目录含 app.json → manifest 条目（kind=app）
    let mut app_dirs = Vec::new();
    collect_app_dirs(root, &mut app_dirs);
    for dir in &app_dirs {
        let Ok(m) = crate::market::read_manifest(dir) else { continue };
        let rel = dir.strip_prefix(root).unwrap_or(dir).to_string_lossy().replace('\\', "/");
        if !seen.insert(m.id.clone()) {
            continue;
        }
        out.push(HubApp {
            id: m.id,
            name: m.name,
            version: m.version,
            entry: m.entry,
            permissions: m.permissions,
            source: String::new(),
            description: m.description,
            kind: "app".to_string(),
            rel,
        });
    }

    // 2. 技能：md 文档（SKILL.md / 带 frontmatter 的裸 md）
    let mut mds = Vec::new();
    walk_md_files(root, &mut mds);
    for md in mds {
        if app_dirs.iter().any(|d| is_within(&md, d)) {
            continue; // 应用目录内的文档不算独立技能
        }
        let fname = md.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
        if EXCLUDED_DOCS.contains(&fname.as_str()) {
            continue;
        }
        let rel = md.strip_prefix(root).unwrap_or(&md).to_string_lossy().replace('\\', "/");
        let Ok(content) = std::fs::read_to_string(&md) else { continue };
        let fm = parse_frontmatter(&content);
        let is_skill_md = fname == "skill.md";
        if fm.is_empty() && !is_skill_md {
            continue; // 无 frontmatter 的普通文档（README 等）跳过
        }
        // 名称：frontmatter name/title → 目录名 → 文件名去扩展名
        let dir_name = md.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str());
        let stem = md.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let raw_name = fm_get(&fm, "name")
            .or_else(|| fm_get(&fm, "title"))
            .or_else(|| dir_name.map(String::from))
            .unwrap_or_else(|| stem.to_string());
        // id：frontmatter 名 sanitize 后若退化（纯连字符 = 全非 ascii），回退文件名 stem（通常 ascii）
        let mut id = sanitize_name(&raw_name.trim());
        if id.is_empty() || id.chars().all(|c| c == '-') {
            id = sanitize_name(stem);
        }
        if id.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        let version = fm_get(&fm, "version").unwrap_or_else(|| "0.0.0".to_string());
        let kind = match fm_get(&fm, "type").as_deref() {
            Some("app") => "app".to_string(),
            _ => "skill".to_string(),
        };
        out.push(HubApp {
            id: id.clone(),
            name: raw_name,
            version,
            entry: String::new(),
            permissions: Vec::new(),
            source: String::new(),
            description: fm_get(&fm, "description").unwrap_or_default(),
            kind,
            rel,
        });
    }

    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

// ==================== 旧索引解析（兼容通道） ====================

/// 解析 skillhub.md 索引文本：frontmatter（type: hub / name / version）+ ## apps 列表。
/// 防御式：type 非 hub 或缺 name → 整体失败；条目缺 id / id 非法 / source 非法 → 跳过该条。
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
        return;
    }
    if a.source.is_empty() || !valid_source(&a.source) {
        return;
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

// ==================== 拉取 / 缓存 / 连接 ====================

/// 拉取文本（HTTP GET，限 512KB，30s 超时）
async fn fetch_text(url: &str) -> Result<String, String> {
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

/// 从来源 URL 推断 hub 名（仓库/目录末段，去 .git/.zip/.md 后缀；GitHub archive 取 repo 名）
fn infer_name(u: &str) -> String {
    let t = u.trim().trim_end_matches('/');
    // GitHub archive 链接：.../archive/refs/heads/<branch>.zip → 取 archive 前段（repo 名）
    let t = t
        .split_once("/archive/")
        .map(|(before, _)| before)
        .unwrap_or(t);
    let t = t.rsplit(['/', ':', '\\']).next().unwrap_or("hub");
    let t = t.trim_end_matches(".git").trim_end_matches(".zip").trim_end_matches(".md");
    let name = sanitize_name(t);
    if name.is_empty() { "hub".to_string() } else { name }
}

fn hubs_dir(root: &Path) -> PathBuf {
    root.join(HUBS_DIR)
}

/// 集合缓存根目录（git 仓库 / zip 解压 / 单 md 落盘共用 <name>/ 目录）
fn collection_dir(root: &Path, name: &str) -> PathBuf {
    hubs_dir(root).join(name)
}

/// 定位已缓存的集合根：git → src[#sub]；zip → pkg 内包根；local → 原目录；单 md → 目录本身
pub fn collection_root(root: &Path, h: &HubInfo) -> Option<PathBuf> {
    let u = h.url.trim();
    if let Some(p) = u.strip_prefix("local:") {
        let pb = PathBuf::from(p);
        return pb.is_dir().then_some(pb);
    }
    let base = collection_dir(root, &h.name);
    if let Some(rest) = u.strip_prefix("git+") {
        let (_, sub) = rest.split_once('#').unwrap_or((rest, ""));
        let src = base.join("src");
        let root_dir = if sub.is_empty() { src } else { src.join(sub) };
        return root_dir.is_dir().then_some(root_dir);
    }
    if u.ends_with(".zip") {
        let pkg = base.join("pkg");
        return pkg.is_dir().then(|| find_pkg_root(&pkg));
    }
    if u.ends_with(".md") && base.is_dir() {
        return Some(base); // 单 md 集合
    }
    None
}

/// 解压产物定位包根：根含 app.json 或 md 则用根，否则扫一层子目录（GitHub archive 顶层 repo-commit/）
pub fn find_pkg_root(out: &Path) -> PathBuf {
    if out.join("app.json").is_file() || has_md(out) {
        return out.to_path_buf();
    }
    if let Ok(rd) = std::fs::read_dir(out) {
        for e in rd.flatten() {
            if e.path().is_dir() && (e.path().join("app.json").is_file() || has_md(&e.path())) {
                return e.path();
            }
        }
    }
    out.to_path_buf()
}

/// 分析集合并持久化 hub 注册表（kb/hubs/<name>.json + 集合缓存）
fn persist_hub(root: &Path, h: &HubInfo) -> Result<(), String> {
    let dir = hubs_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(h).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}.json", h.name)), json).map_err(|e| e.to_string())
}

/// 连接 hub：按来源类型分发（git 仓库 clone / GitHub zip 解压 / 本地目录引用 / md 索引或单技能 /
/// skillhub API 型），扫描分析集合生成目录，持久化注册表。集合缓存在 kb/hubs/<name>/，安装时直接取文件。
pub async fn connect_hub(root: &Path, url: &str) -> Result<HubInfo, String> {
    let u = url.trim();
    if !valid_hub_url(u) {
        return Err("hub 来源仅支持：git+https 仓库 / https zip / https md / local: 目录".to_string());
    }

    // skillhub API 型：URL 含 skillhub.cn（install/skillhub.md 引导文档或 api.skillhub.cn 直连）
    // —— 无文档集合可分析，目录 = showcase 榜单 API 合并，安装走 download?slug= zip 下载
    if u.contains("skillhub.cn") {
        return connect_skillhub_api(root, u).await;
    }

    // 本地路径：目录 → 集合分析引用；md 文件 → 读内容走索引/单技能通道
    if let Some(p) = u.strip_prefix("local:") {
        let path = PathBuf::from(p);
        if path.is_dir() {
            let apps = analyze_collection(&path);
            let h = HubInfo {
                name: infer_name(p),
                url: u.to_string(),
                version: "0".to_string(),
                apps,
            };
            persist_hub(root, &h)?;
            return Ok(h);
        }
        if path.is_file() && path.extension().map(|x| x == "md").unwrap_or(false) {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("读取失败: {e}"))?;
            return connect_md(root, u, p, &text).await;
        }
        return Err(format!("本地路径不存在或不是 md 文档: {p}"));
    }

    // md 文档：先试旧索引（type: hub），失败按单技能集合处理
    if u.ends_with(".md") {
        let text = fetch_text(u).await?;
        return connect_md(root, u, u, &text).await;
    }

    // git 仓库：clone 到 kb/hubs/<name>/src（可带 #子目录）
    if let Some(rest) = u.strip_prefix("git+") {
        let (repo, sub) = rest.split_once('#').unwrap_or((rest, ""));
        let name = infer_name(repo);
        let base = collection_dir(root, &name);
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
        let src = base.join("src");
        let status = std::process::Command::new("git")
            .args(["clone", "--depth", "1", "--quiet", repo])
            .arg(&src)
            .status()
            .map_err(|e| format!("git 命令执行失败: {e}"))?;
        if !status.success() {
            return Err(format!("git clone 失败: {repo}"));
        }
        let collection = if sub.is_empty() { src } else { src.join(sub) };
        if !collection.is_dir() {
            return Err(format!("仓库内找不到子目录: {}", if sub.is_empty() { "(空)" } else { sub }));
        }
        let apps = analyze_collection(&collection);
        let h = HubInfo { name, url: u.to_string(), version: "0".to_string(), apps };
        persist_hub(root, &h)?;
        return Ok(h);
    }

    // GitHub archive zip：下载解压到 kb/hubs/<name>/pkg
    if u.ends_with(".zip") {
        let name = infer_name(u);
        let base = collection_dir(root, &name);
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .user_agent("md-agent/0.1 (skillhub client)")
            .build()
            .map_err(|e| format!("构建客户端失败: {e}"))?;
        let resp = client.get(u).send().await.map_err(|e| format!("下载失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        let bytes = resp.bytes().await.map_err(|e| format!("读取失败: {e}"))?;
        let zip_path = base.join("pkg.zip");
        std::fs::write(&zip_path, &bytes).map_err(|e| e.to_string())?;
        let out = base.join("pkg");
        let ps = format!(
            "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
            zip_path.display(),
            out.display()
        );
        let status = std::process::Command::new(powershell_exe())
            .args(["-NoProfile", "-Command", &ps])
            .status()
            .map_err(|e| format!("powershell 解压失败: {e}"))?;
        if !status.success() {
            return Err("zip 解压失败".to_string());
        }
        let apps = analyze_collection(&find_pkg_root(&out));
        let h = HubInfo { name, url: u.to_string(), version: "0".to_string(), apps };
        persist_hub(root, &h)?;
        return Ok(h);
    }

    Err("无法识别的 hub 来源（支持 git+https 仓库 / https zip / https md / local: 目录）".to_string())
}

/// md 文档连接：先试旧索引（type: hub，条目带 source 走下载通道），
/// 失败按单技能集合处理（落盘到集合目录后分析，保持 rel 安装语义）
async fn connect_md(root: &Path, url: &str, name_src: &str, text: &str) -> Result<HubInfo, String> {
    if let Ok(h) = parse_hub_index(text, url) {
        persist_hub(root, &h)?;
        return Ok(h);
    }
    let name = infer_name(name_src);
    let base = collection_dir(root, &name);
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let fname = Path::new(name_src)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("SKILL.md");
    let target = base.join(fname);
    if target.is_file() && !matches!(std::fs::read_to_string(&target), Ok(s) if s == text) {
        return Err("hub 重名且内容不同——先 /market disconnect 再连接".to_string());
    }
    std::fs::write(&target, text).map_err(|e| e.to_string())?;
    let apps = analyze_collection(&base);
    let h = HubInfo { name, url: url.to_string(), version: "0".to_string(), apps };
    persist_hub(root, &h)?;
    Ok(h)
}

// ==================== skillhub API 型 hub（检索/榜单/下载 API） ====================

const SKILLHUB_API: &str = "https://api.skillhub.cn/api/v1";
const SKILLHUB_DOWNLOAD: &str = "https://api.skillhub.cn/api/v1/download?slug=";
const SKILLHUB_SHOWCASE: [&str; 5] = ["hot", "featured", "newest", "recommended", "trending"];
const SKILLHUB_MAX_ITEMS: usize = 120;

/// 是否为 skillhub download URL（download_app 当 zip 处理）
pub fn is_skillhub_download(s: &str) -> bool {
    s.starts_with(SKILLHUB_DOWNLOAD)
}

/// skillhub 检索：/api/v1/search?q= → 归一化 HubApp 列表（id=slug，source=下载 URL）
pub async fn search_skillhub(q: &str) -> Result<Vec<HubApp>, String> {
    let url = format!("{SKILLHUB_API}/search?q={}", urlencode(q.trim()));
    let text = fetch_text(&url).await?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("检索响应解析失败: {e}"))?;
    let results = v.get("results").and_then(|r| r.as_array()).cloned().unwrap_or_default();
    Ok(results.iter().filter_map(skillhub_entry).map(|(a, _)| a).collect())
}

/// 榜单 API → 合并去重（slug 去重、downloads 降序、取前 SKILLHUB_MAX_ITEMS）
async fn skillhub_showcase() -> Result<Vec<HubApp>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<(HubApp, u64)> = Vec::new();
    for section in SKILLHUB_SHOWCASE {
        let url = format!("{SKILLHUB_API}/showcase/{section}");
        let text = match fetch_text(&url).await {
            Ok(t) => t,
            Err(_) => continue, // 单榜失败不影响其他榜
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let skills = v
            .get("skills")
            .or_else(|| v.get("results"))
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        for s in skills {
            if let Some((app, dl)) = skillhub_entry(&s) {
                if seen.insert(app.id.clone()) {
                    out.push((app, dl));
                }
            }
        }
    }
    // downloads 降序（热门在前），限量
    out.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(out.into_iter().take(SKILLHUB_MAX_ITEMS).map(|(a, _)| a).collect())
}

/// skillhub 条目 → (HubApp, downloads)（兼容 search/showcase 两套字段命名：camelCase / snake_case）
fn skillhub_entry(s: &serde_json::Value) -> Option<(HubApp, u64)> {
    let g = |k1: &str, k2: &str| -> Option<String> {
        s.get(k1)
            .or_else(|| s.get(k2))
            .and_then(|v| v.as_str())
            .map(String::from)
    };
    let slug = g("slug", "publicSlug")?;
    if slug.is_empty() {
        return None;
    }
    let name = g("displayName", "name").unwrap_or_else(|| slug.clone());
    let version = g("version", "").unwrap_or_else(|| "0.0.0".to_string());
    let desc = g("summary", "")
        .or_else(|| g("description_zh", ""))
        .or_else(|| g("description", ""));
    let downloads = s
        .get("downloads")
        .or_else(|| s.get("installs"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let mut description = desc.unwrap_or_default();
    if downloads > 0 {
        description = format!("⬇ {downloads} · {description}");
    }
    Some((
        HubApp {
            id: slug.clone(),
            name,
            version,
            entry: String::new(),
            permissions: Vec::new(),
            source: format!("{SKILLHUB_DOWNLOAD}{}", urlencode(&slug)),
            description,
            kind: "skill".to_string(),
            rel: String::new(),
        },
        downloads,
    ))
}

/// 连接 skillhub API 型 hub：拉五榜合并 → 注册表（rel 空、source=下载 URL）
async fn connect_skillhub_api(root: &Path, url: &str) -> Result<HubInfo, String> {
    let apps = skillhub_showcase().await?;
    let h = HubInfo {
        name: "skillhub.cn".to_string(),
        url: url.to_string(),
        version: "0".to_string(),
        apps,
    };
    persist_hub(root, &h)?;
    Ok(h)
}

/// 百分号编码（URL 查询参数安全；只编码非 ascii 与保留字符）
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 已连接 hub 列表（读 kb/hubs/*.json）
pub fn list_hubs(root: &Path) -> Vec<HubInfo> {
    let dir = hubs_dir(root);
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().map(|x| x == "json").unwrap_or(false) {
            if let Ok(content) = std::fs::read_to_string(&p) {
                if let Ok(h) = serde_json::from_str::<HubInfo>(&content) {
                    out.push(h);
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 刷新 hub：按注册的 url 重新连接（覆盖集合缓存与注册表；失败不丢旧文件）
pub async fn refresh_hub(root: &Path, name: &str) -> Result<HubInfo, String> {
    if !file_safe(name) {
        return Err(format!("非法 hub 名: {name}"));
    }
    let file = hubs_dir(root).join(format!("{name}.json"));
    let old = std::fs::read_to_string(&file).map_err(|e| format!("hub 未连接: {e}"))?;
    let old_hub: HubInfo = serde_json::from_str(&old).map_err(|e| e.to_string())?;
    connect_hub(root, &old_hub.url).await
}

/// 断开 hub：删除注册表 json 与集合缓存（不删已安装的 app——已装 app 是本地权威副本）
pub fn disconnect_hub(root: &Path, name: &str) -> Result<(), String> {
    if !file_safe(name) {
        return Err(format!("非法 hub 名: {name}"));
    }
    let json = hubs_dir(root).join(format!("{name}.json"));
    if !json.is_file() {
        return Err(format!("hub 未连接: {name}"));
    }
    std::fs::remove_file(&json).map_err(|e| e.to_string())?;
    let col = collection_dir(root, name);
    if col.is_dir() {
        let _ = std::fs::remove_dir_all(&col);
    }
    Ok(())
}

/// 旧索引条目按 source 下载 app 包到临时目录（tmp_root 由调用方创建并负责清理），返回包根。
/// 支持：local:<路径> / git+https://<url>[#<子目录>] / https zip / https md 裸技能文件。
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
    // zip 包（https zip / skillhub download?slug= API——Content-Type 为 zip 但 URL 不带 .zip）
    if valid_source(s) && (s.ends_with(".zip") || is_skillhub_download(s)) {
        std::fs::create_dir_all(tmp_root).map_err(|e| e.to_string())?;
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
        let status = std::process::Command::new(powershell_exe())
            .args(["-NoProfile", "-Command", &ps])
            .status()
            .map_err(|e| format!("powershell 解压失败: {e}"))?;
        if !status.success() {
            return Err("zip 解压失败".to_string());
        }
        return Ok(find_pkg_root(&out));
    }
    // 裸技能文件（https://…md 或本地 mock）：直接下载文本，返回文件路径（install_bundle 识别为技能）
    if valid_source(s) && s.ends_with(".md") {
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

    // ---------- md 文档分析器 ----------

    /// 构造模拟真实 SkillHub 的文档集合：app 目录 + SKILL.md 技能 + 裸 md 技能 + 排除文档
    fn make_collection(root: &Path) {
        // 应用：目录含 app.json
        fs::create_dir_all(root.join("apps/hello")).unwrap();
        fs::write(root.join("apps/hello/app.json"), r#"{"id":"hello","name":"Hello 应用","version":"1.2.0","entry":"index.html","permissions":["llm"]}"#).unwrap();
        fs::write(root.join("apps/hello/index.html"), "<h1>hi</h1>").unwrap();
        // SKILL.md 技能（anthropics 风格：name/description/license）
        fs::create_dir_all(root.join("skills/docx")).unwrap();
        fs::write(root.join("skills/docx/SKILL.md"), "---\nname: docx\ndescription: 创建与编辑 Word 文档\nlicense: Proprietary\n---\n\n# DOCX\n步骤\n").unwrap();
        // 裸 md 技能（带 frontmatter）
        fs::create_dir_all(root.join("skills")).unwrap();
        fs::write(root.join("skills/meeting-notes.md"), "---\ntitle: 会议纪要整理\ndescription: 把录音转成结构化纪要\n---\n\n# 会议纪要\n").unwrap();
        // 排除：README / 无 frontmatter 文档 / 应用目录内 md
        fs::write(root.join("README.md"), "# 集合说明\n").unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/guide.md"), "没有 frontmatter 的普通文档\n").unwrap();
        fs::write(root.join("apps/hello/README.md"), "应用说明\n").unwrap();
        // 嵌套技能（wshobson 风格：plugins/<name>/）
        fs::create_dir_all(root.join("plugins/code-review")).unwrap();
        fs::write(root.join("plugins/code-review/SKILL.md"), "---\nname: code-review\ndescription: 代码评审\n---\n\n# 评审\n").unwrap();
    }

    #[test]
    fn analyze_collection_extracts_apps_and_skills() {
        let root = tmp("ac");
        make_collection(&root);
        let apps = analyze_collection(&root);
        let ids: Vec<&str> = apps.iter().map(|a| a.id.as_str()).collect();
        // 3 技能 + 1 应用 = 4 条；README/guide/应用内 README 排除
        assert_eq!(ids.len(), 4, "目录应为 4 条: {ids:?}");
        let app = apps.iter().find(|a| a.kind == "app").unwrap();
        assert_eq!(app.id, "hello");
        assert_eq!(app.version, "1.2.0");
        assert_eq!(app.rel, "apps/hello");
        assert!(apps.iter().any(|a| a.id == "docx" && a.kind == "skill"));
        assert!(apps.iter().any(|a| a.id == "meeting-notes" && a.description == "把录音转成结构化纪要"));
        assert!(apps.iter().any(|a| a.id == "code-review" && a.rel == "plugins/code-review/SKILL.md"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn analyze_skips_duplicate_ids_and_junk() {
        let root = tmp("dup");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join("a/SKILL.md"), "---\nname: same\ndescription: 一\n---\n").unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(root.join("b/SKILL.md"), "---\nname: same\ndescription: 二\n---\n").unwrap();
        fs::write(root.join("LICENSE.txt"), "MIT\n").unwrap();
        let apps = analyze_collection(&root);
        assert_eq!(apps.len(), 1, "重名只留一条: {:?}", apps.iter().map(|a| a.id.clone()).collect::<Vec<_>>());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn frontmatter_parse_basic() {
        let fm = parse_frontmatter("---\nname: x\ndescription: \"引号描述\"\n---\n# 正文\n");
        assert_eq!(fm_get(&fm, "name").as_deref(), Some("x"));
        assert_eq!(fm_get(&fm, "description").as_deref(), Some("引号描述"));
        assert!(parse_frontmatter("# 无 frontmatter\n正文").is_empty());
    }

    // ---------- 旧索引兼容 ----------

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
    fn parse_skips_bad_entries() {
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

    // ---------- 连接 / 缓存 / 安装定位 ----------

    #[test]
    fn connect_local_analyzes_and_persists() {
        let root = tmp("root");
        let src = tmp("src-col");
        make_collection(&src);
        let h = tokio_test_block_on(connect_hub(&root, &format!("local:{}", src.display()))).unwrap();
        let hname = src.file_name().unwrap().to_str().unwrap().to_string();
        assert_eq!(h.name, hname);
        assert_eq!(h.apps.len(), 4);
        // 注册表落盘 + 目录可重读
        assert!(root.join(format!("hubs/{hname}.json")).is_file());
        let listed = list_hubs(&root);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].apps.len(), 4);
        // 集合定位：local 直接指向原目录
        assert_eq!(collection_root(&root, &listed[0]).unwrap(), src);
        // 断开：删 json + 集合
        disconnect_hub(&root, &hname).unwrap();
        assert_eq!(list_hubs(&root).len(), 0);
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&src).unwrap();
    }

    #[test]
    fn connect_local_rejects_missing_dir() {
        let root = tmp("root2");
        let r = tokio_test_block_on(connect_hub(&root, "local:C:/no/such/dir")).unwrap_err();
        assert!(r.contains("不存在"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn md_index_connect_prefers_index_then_single_skill() {
        let root = tmp("root3");
        // 旧索引 md 文件 → 索引通道（hub 名取 frontmatter，条目带 source 不带 rel）
        let idx = root.join("idx.md");
        fs::write(&idx, FULL_INDEX).unwrap();
        let h = tokio_test_block_on(connect_hub(&root, &format!("local:{}", idx.display()))).unwrap();
        assert_eq!(h.name, "skillhub.cn");
        assert_eq!(h.apps.len(), 2);
        assert!(h.apps[0].source.contains("github.com"));
        assert!(h.apps[0].rel.is_empty(), "索引条目不带 rel，安装走 source 下载");
        // 单技能 md 文件 → 单技能集合通道（rel 指向文件）
        let sk = root.join("my-skill.md");
        fs::write(&sk, "---\nname: my-skill\ndescription: 我的技能\n---\n# 步骤\n").unwrap();
        let h2 = tokio_test_block_on(connect_hub(&root, &format!("local:{}", sk.display()))).unwrap();
        assert_eq!(h2.apps.len(), 1);
        assert_eq!(h2.apps[0].id, "my-skill");
        assert_eq!(h2.apps[0].rel, "my-skill.md");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn infer_name_from_urls() {
        assert_eq!(infer_name("https://github.com/anthropics/skills/archive/refs/heads/main.zip"), "skills");
        assert_eq!(infer_name("git+https://github.com/wshobson/agents.git"), "agents");
        assert_eq!(infer_name("C:/users/x/skills"), "skills");
    }

    #[test]
    fn skillhub_entry_normalizes_both_field_styles() {
        // showcase 风格（snake_case）
        let showcase = serde_json::json!({
            "slug": "docx", "name": "DOCX", "displayName": "DOCX 创建、编辑与分析",
            "version": "1.0.0", "downloads": 2729,
            "summary": "当用户想要创建 Word 文档时使用"
        });
        let (a, dl) = skillhub_entry(&showcase).unwrap();
        assert_eq!(a.id, "docx");
        assert_eq!(a.name, "DOCX 创建、编辑与分析");
        assert_eq!(a.version, "1.0.0");
        assert_eq!(dl, 2729);
        assert!(a.source.contains("download?slug=docx"));
        assert!(a.description.contains("2729"), "下载量应进描述: {}", a.description);
        assert_eq!(a.kind, "skill");
        assert!(a.rel.is_empty(), "API 条目无 rel（走 source 下载）");
        // search 风格（slug/publicSlug 兼容）＋ 缺 downloads
        let search = serde_json::json!({"slug": "excel-xlsx", "name": "Excel / XLSX", "version": "0.0.0", "summary": "s"});
        let (a2, dl2) = skillhub_entry(&search).unwrap();
        assert_eq!(a2.id, "excel-xlsx");
        assert_eq!(dl2, 0);
        // 缺 slug → 跳过
        assert!(skillhub_entry(&serde_json::json!({"name": "no-slug"})).is_none());
    }

    #[test]
    fn skillhub_download_url_detection() {
        assert!(is_skillhub_download("https://api.skillhub.cn/api/v1/download?slug=docx"));
        assert!(!is_skillhub_download("https://example.com/pkg.zip"));
    }

    #[test]
    fn connect_persists_and_disconnect_guards() {
        let root = tmp("root4");
        let src = tmp("src4");
        make_collection(&src);
        tokio_test_block_on(connect_hub(&root, &format!("local:{}", src.display()))).unwrap();
        assert!(disconnect_hub(&root, "../evil").is_err());
        assert!(disconnect_hub(&root, "nope").is_err());
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&src).unwrap();
    }

    #[test]
    fn download_local_path() {
        let root = tmp("dl");
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
