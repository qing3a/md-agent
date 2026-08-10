//! 本地配置：统一路径 config.json（用户配置目录，Windows: %APPDATA%\md-agent\config.json），
//! env MD_AGENT_CONFIG 可覆盖路径（测试隔离用）。旧 exe 旁 config.json 首次启动自动迁移。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    /// Ollama / OpenAI 兼容基址，如 http://127.0.0.1:11434 或 https://api.openai.com/v1
    pub endpoint: String,
    pub model: String,
    /// 云端 API Key（Ollama 本地可不填）
    pub api_key: String,
    /// 语义召回通道（可选）：OpenAI 兼容 /embeddings 端点；未配置则语义召回降级纯 grep
    pub embedding: EmbeddingConfig,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            model: String::new(),
            api_key: String::new(),
            embedding: EmbeddingConfig::default(),
        }
    }
}

/// Embedding 配置（语义召回的向量来源，Phase 4 M1）：
/// 留空 = 语义召回关闭，现有纯 grep 检索完全不变（零破坏）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    /// OpenAI 兼容 embeddings 基址（如 http://127.0.0.1:11434/v1 或 https://api.openai.com/v1）
    pub endpoint: String,
    /// embedding 模型名（如 bge-m3 / text-embedding-3-large）
    pub model: String,
    pub api_key: String,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            model: String::new(),
            api_key: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HeartbeatConfig {
    /// 心跳自动同步开关（默认关，保持手动行为）
    pub enabled: bool,
    /// 检查周期（秒）
    pub interval_secs: u64,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub kb_root: String,
    pub llm: LlmConfig,
    pub heartbeat: HeartbeatConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            kb_root: crate::kb::kb_root().to_string_lossy().into_owned(),
            llm: LlmConfig::default(),
            heartbeat: HeartbeatConfig::default(),
        }
    }
}

/// 统一配置路径：MD_AGENT_CONFIG 优先（测试隔离），否则固定用户配置目录——
/// debug/dist 各自 exe 旁 config.json 独立漂移（曾出配置页空字段清空事故），统一到用户目录后共享一份。
pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("MD_AGENT_CONFIG") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Some(dir) = user_config_dir() {
        dir.join("config.json")
    } else {
        legacy_config_path() // 非常规环境（无用户目录）→ 回退 exe 旁（保持旧行为）
    }
}

/// 用户配置目录：Windows %APPDATA%\md-agent；其他 $XDG_CONFIG_HOME 或 ~/.config 的 md-agent
fn user_config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(appdata).join("md-agent"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
            if !x.trim().is_empty() {
                return Some(PathBuf::from(x).join("md-agent"));
            }
        }
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".config").join("md-agent"))
    }
}

/// 旧位置：exe 旁 config.json（debug/dist 各自，统一前的位置）
fn legacy_config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("config.json")))
        .unwrap_or_else(|| PathBuf::from("config.json"))
}

/// 迁移旧配置到统一路径：统一路径无 config 且 exe 旁有 → 复制一次（先到先得；之后读写都走统一路径）。
/// 测试隔离（MD_AGENT_CONFIG）下不触发：统一路径=env 路径，仅当它不存在且 exe 旁确有 config 才复制。
fn migrate_legacy() {
    let p = config_path();
    if p.exists() {
        return;
    }
    let legacy = legacy_config_path();
    if legacy.exists() && legacy != p {
        if let Ok(s) = std::fs::read_to_string(&legacy) {
            if let Some(dir) = p.parent() {
                if std::fs::create_dir_all(dir).is_ok() {
                    if std::fs::write(&p, s).is_ok() {
                        eprintln!("config 已迁移到统一路径: {}", p.display());
                    }
                }
            }
        }
    }
}

pub fn load() -> Config {
    migrate_legacy();
    let p = config_path();
    if let Ok(s) = std::fs::read_to_string(&p) {
        if let Ok(c) = serde_json::from_str(&s) {
            return c;
        }
    }
    Config::default()
}

pub fn save(c: &Config) -> std::io::Result<()> {
    migrate_legacy();
    let p = config_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let s = serde_json::to_string_pretty(c).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(p, s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    // env 是全局的，测试并行会互相覆盖 MD_AGENT_CONFIG → 静态锁串行化 env 操作
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner()) // 容忍 panic 中毒
    }

    fn tmp_cfg(name: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("md-agent-cfg-test-{}-{}.json", name, n))
    }

    #[test]
    fn env_override_takes_precedence() {
        let _g = lock_env();
        let f = tmp_cfg("env");
        std::env::set_var("MD_AGENT_CONFIG", &f);
        assert_eq!(config_path(), f);
        let _ = std::fs::remove_file(&f);
        std::env::remove_var("MD_AGENT_CONFIG");
    }

    #[test]
    fn load_save_roundtrip_via_env() {
        let _g = lock_env();
        let f = tmp_cfg("rt");
        std::env::set_var("MD_AGENT_CONFIG", &f);
        let mut c = Config::default();
        c.llm.endpoint = "http://test".into();
        c.llm.api_key = "k-12345678901234567890123456789012".into();
        c.heartbeat.enabled = true;
        save(&c).expect("save");
        let loaded = load();
        assert_eq!(loaded.llm.endpoint, "http://test");
        assert_eq!(loaded.llm.api_key.len(), 34); // "k-" + 32 位数字
        assert!(loaded.heartbeat.enabled);
        let _ = std::fs::remove_file(&f);
        std::env::remove_var("MD_AGENT_CONFIG");
    }

    #[test]
    fn missing_returns_default() {
        let _g = lock_env();
        let f = tmp_cfg("missing");
        std::env::set_var("MD_AGENT_CONFIG", &f);
        let _ = std::fs::remove_file(&f);
        let c = load();
        assert!(c.llm.endpoint.is_empty());
        let _ = std::fs::remove_file(&f);
        std::env::remove_var("MD_AGENT_CONFIG");
    }

    #[test]
    fn create_dir_all_on_save_nested() {
        let _g = lock_env();
        let base = tmp_cfg("nested");
        let dir = base.join("sub");
        std::env::set_var("MD_AGENT_CONFIG", &dir.join("config.json"));
        save(&Config::default()).expect("save nested");
        assert!(dir.join("config.json").exists());
        let _ = std::fs::remove_dir_all(&base);
        std::env::remove_var("MD_AGENT_CONFIG");
    }

    #[test]
    fn env_isolation_no_migration_error() {
        let _g = lock_env();
        // env 覆盖下迁移逻辑不污染：env 路径不存在且 exe 旁（测试二进制 deps/ 下）无 config → load 返回 default 且不报错
        let f = tmp_cfg("migrate");
        let _ = std::fs::remove_file(&f);
        std::env::set_var("MD_AGENT_CONFIG", &f);
        let c = load();
        assert!(c.llm.endpoint.is_empty());
        let _ = std::fs::remove_file(&f);
        std::env::remove_var("MD_AGENT_CONFIG");
    }

    #[test]
    fn unified_path_is_user_dir_config_json() {
        let _g = lock_env();
        std::env::remove_var("MD_AGENT_CONFIG");
        let base = tmp_cfg("appdata");
        std::env::set_var("APPDATA", &base);
        // Windows 分支：%APPDATA%\md-agent\config.json（目录 + 文件名都要拼对）
        let p = config_path();
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("config.json"));
        assert!(p.to_string_lossy().contains("md-agent"));
        std::env::remove_var("APPDATA");
    }
}
