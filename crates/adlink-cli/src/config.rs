//! Declarative TOML configuration loader with `${ENV_VAR}` expansion.

use std::collections::BTreeMap;
use std::path::Path;

use adlink_api::ProductionServerConfig;
use adlink_connectors::{ConnectorMode, ConnectorSuiteConfig};
use adlink_domain::AdCreationConfig;
use adlink_mcp::GuardrailConfig;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSection {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_web_dir")]
    pub web_dir: String,
    #[serde(flatten, default)]
    pub production: ProductionServerConfig,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            web_dir: default_web_dir(),
            production: ProductionServerConfig::default(),
        }
    }
}

fn default_host() -> String {
    "0.0.0.0".into()
}
fn default_port() -> u16 {
    8787
}
fn default_web_dir() -> String {
    "web-starter".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSection {
    #[serde(default = "default_db_path")]
    pub db_path: String,
    #[serde(default = "default_reader_pool_size")]
    pub reader_pool_size: usize,
}

impl Default for StorageSection {
    fn default() -> Self {
        Self {
            db_path: default_db_path(),
            reader_pool_size: default_reader_pool_size(),
        }
    }
}

fn default_db_path() -> String {
    "data/adlink.db".into()
}
fn default_reader_pool_size() -> usize {
    8
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthSection {
    #[serde(default)]
    pub api_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AdLinkConfig {
    #[serde(default)]
    pub server: ServerSection,
    #[serde(default)]
    pub storage: StorageSection,
    #[serde(default)]
    pub auth: AuthSection,
    #[serde(default)]
    pub guardrail: GuardrailConfig,
    #[serde(default)]
    pub ad_creation: AdCreationConfig,
    #[serde(default)]
    pub fx_rates: BTreeMap<String, f64>,
    #[serde(default)]
    pub connectors: BTreeMap<String, ConnectorMode>,
}

impl AdLinkConfig {
    pub fn load_or_default(path: Option<&Path>) -> anyhow::Result<Self> {
        let Some(p) = path else {
            return Ok(Self::default());
        };
        let raw = std::fs::read_to_string(p)
            .map_err(|e| anyhow::anyhow!("failed to read config '{}': {e}", p.display()))?;
        let expanded = expand_env_vars(&raw);
        let cfg: Self = toml::from_str(&expanded)
            .map_err(|e| anyhow::anyhow!("failed to parse TOML config '{}': {e}", p.display()))?;
        Ok(cfg)
    }

    pub fn connector_suite(&self) -> ConnectorSuiteConfig {
        ConnectorSuiteConfig {
            connectors: self.connectors.clone(),
        }
    }
}

/// Expands `${VAR_NAME}` placeholders using process environment variables (leaving unset ones empty).
pub fn expand_env_vars(input: &str) -> String {
    let re = Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("valid regex");
    re.replace_all(input, |caps: &regex::Captures<'_>| {
        let key = &caps[1];
        std::env::var(key).unwrap_or_default()
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn repo_toml_configs_parse_all_fifteen_connectors_and_prod_settings() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir.parent().unwrap().parent().unwrap();

        let mock_cfg =
            AdLinkConfig::load_or_default(Some(&repo_root.join("config/adlink.mock.toml")))
                .unwrap();
        assert_eq!(mock_cfg.connectors.len(), 15);
        assert_eq!(mock_cfg.storage.reader_pool_size, 8);
        assert!(!mock_cfg.ad_creation.enabled);
        assert_eq!(mock_cfg.fx_rates.get("USD"), Some(&7.25));
        assert_eq!(mock_cfg.server.production.max_in_flight_requests, 512);

        let ex_cfg =
            AdLinkConfig::load_or_default(Some(&repo_root.join("config/adlink.example.toml")))
                .unwrap();
        assert_eq!(ex_cfg.connectors.len(), 15);
        assert_eq!(ex_cfg.storage.reader_pool_size, 8);
        assert!(!ex_cfg.ad_creation.enabled);
        assert_eq!(ex_cfg.server.production.log_format, "json");
        assert!(ex_cfg.connectors.contains_key("tiktok_ads"));
        assert!(ex_cfg.connectors.contains_key("meta_ads"));
        assert!(ex_cfg.connectors.contains_key("google_ads"));
        assert!(ex_cfg.connectors.contains_key("apple_search_ads"));
        assert!(ex_cfg.connectors.contains_key("amazon_ads"));
    }
}
