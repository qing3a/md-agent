//! 本地配置：config.json（默认在可执行文件旁），env MD_AGENT_CONFIG 可覆盖路径。

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
}

impl Default for LlmConfig {
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
pub struct Config {
    pub kb_root: String,
    pub llm: LlmConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            kb_root: crate::kb::kb_root().to_string_lossy().into_owned(),
            llm: LlmConfig::default(),
        }
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("MD_AGENT_CONFIG") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("config.json")))
        .unwrap_or_else(|| PathBuf::from("config.json"))
}

pub fn load() -> Config {
    let p = config_path();
    if let Ok(s) = std::fs::read_to_string(&p) {
        if let Ok(c) = serde_json::from_str(&s) {
            return c;
        }
    }
    Config::default()
}

pub fn save(c: &Config) -> std::io::Result<()> {
    let s = serde_json::to_string_pretty(c).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(config_path(), s)
}
