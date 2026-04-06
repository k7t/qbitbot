use anyhow::Context;
use serde::Deserialize;
use std::path::Path;

fn default_qb_url() -> String {
    "http://localhost:8080".to_string()
}
fn default_username() -> String {
    "admin".to_string()
}
fn default_password() -> String {
    "adminadmin".to_string()
}
fn default_limit() -> usize {
    10
}
fn default_format() -> TorrentFormat {
    TorrentFormat::Detailed
}
fn default_categories() -> Vec<Category> {
    vec![Category {
        name: "Default".to_string(),
        save_path: String::new(),
    }]
}
fn default_event_port() -> u16 {
    9091
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TorrentFormat {
    Detailed,
    Brief,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Category {
    pub name: String,
    pub save_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_qb_url")]
    pub qb_url: String,

    #[serde(default = "default_username")]
    pub qb_username: String,

    #[serde(default = "default_password")]
    pub qb_password: String,

    pub bot_token: String,

    pub bot_allowed_users: Vec<i64>,

    #[serde(default = "default_limit")]
    pub torrent_list_limit: usize,

    #[serde(default = "default_format")]
    pub torrent_format: TorrentFormat,

    #[serde(default = "default_categories")]
    pub categories: Vec<Category>,

    #[serde(default = "default_event_port")]
    pub event_server_port: u16,
}

pub fn load(path: &Path) -> anyhow::Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read config file: {}", path.display()))?;
    let cfg: Config =
        serde_json::from_str(&content).context("Invalid JSON in config file")?;
    if cfg.bot_token.is_empty() {
        anyhow::bail!("bot_token is required in config.json");
    }
    if cfg.bot_allowed_users.is_empty() {
        anyhow::bail!("bot_allowed_users must contain at least one user ID");
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_config(json: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(json.as_bytes()).unwrap();
        f
    }

    #[test]
    fn load_minimal_config() {
        let f = write_config(
            r#"{"bot_token":"tok","bot_allowed_users":[1]}"#,
        );
        let cfg = load(f.path()).unwrap();
        assert_eq!(cfg.qb_url, "http://localhost:8080");
        assert_eq!(cfg.torrent_list_limit, 10);
        assert_eq!(cfg.torrent_format, TorrentFormat::Detailed);
        assert_eq!(cfg.event_server_port, 9091);
    }

    #[test]
    fn load_full_config() {
        let f = write_config(
            r#"{
                "qb_url":"http://host:8080",
                "qb_username":"u",
                "qb_password":"p",
                "bot_token":"t",
                "bot_allowed_users":[42],
                "torrent_list_limit":5,
                "torrent_format":"brief",
                "categories":[{"name":"Movies","save_path":"/movies"}],
                "event_server_port":9999
            }"#,
        );
        let cfg = load(f.path()).unwrap();
        assert_eq!(cfg.torrent_format, TorrentFormat::Brief);
        assert_eq!(cfg.event_server_port, 9999);
        assert_eq!(cfg.categories[0].name, "Movies");
    }

    #[test]
    fn error_on_missing_token() {
        let f = write_config(r#"{"bot_allowed_users":[1],"bot_token":""}"#);
        assert!(load(f.path()).is_err());
    }

    #[test]
    fn error_on_empty_users() {
        let f = write_config(r#"{"bot_token":"tok","bot_allowed_users":[]}"#);
        assert!(load(f.path()).is_err());
    }
}
