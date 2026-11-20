use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub name: String,
    pub direct_url: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Ok(Self {
            bind: var("CONDENSATE_BIND")
                .unwrap_or_else(|| "127.0.0.1:8470".into())
                .parse()
                .context("CONDENSATE_BIND must look like 127.0.0.1:8470")?,
            data_dir: var("CONDENSATE_DATA_DIR").unwrap_or_else(|| "data".into()).into(),
            name: var("CONDENSATE_NAME").unwrap_or_else(|| "Condensate".into()),
            direct_url: var("CONDENSATE_DIRECT_URL").map(|u| u.trim_end_matches('/').to_string()),
        })
    }
}
