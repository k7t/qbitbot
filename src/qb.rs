use anyhow::Context;
use reqwest::{StatusCode, multipart};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::config::Config;

#[derive(Debug, Clone, Deserialize)]
pub struct TorrentInfo {
    #[allow(dead_code)]
    pub hash: String,
    pub name: String,
    pub state: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub dlspeed: u64,
    #[serde(default)]
    pub upspeed: u64,
    #[serde(default)]
    pub num_leechs: u32,
    #[serde(default)]
    pub num_seeds: u32,
    #[serde(default)]
    pub eta: i64,
    #[serde(default)]
    pub ratio: f64,
    #[serde(default)]
    pub completed: u64,
}

pub struct QbClient {
    client: reqwest::Client,
    base_url: String,
    username: String,
    password: String,
    login_lock: Mutex<()>,
}

impl QbClient {
    pub fn new(cfg: &Config) -> anyhow::Result<Self> {
        let client = reqwest::ClientBuilder::new()
            .cookie_store(true)
            .danger_accept_invalid_certs(true)
            .build()
            .context("Failed to build HTTP client")?;
        Ok(Self {
            client,
            base_url: cfg.qb_url.trim_end_matches('/').to_string(),
            username: cfg.qb_username.clone(),
            password: cfg.qb_password.clone(),
            login_lock: Mutex::new(()),
        })
    }

    /// Log in to qBittorrent WebUI and store the session cookie.
    pub async fn login(&self) -> anyhow::Result<()> {
        let url = format!("{}/api/v2/auth/login", self.base_url);
        let resp = self
            .client
            .post(&url)
            .form(&[("username", &self.username), ("password", &self.password)])
            .send()
            .await
            .context("qBittorrent login request failed")?;
        let body = resp.text().await?;
        if body.trim() == "Ok." {
            tracing::info!("Logged in to qBittorrent");
            Ok(())
        } else {
            anyhow::bail!("qBittorrent login failed: {}", body.trim())
        }
    }

    /// Re-login under a lock to prevent concurrent login storms.
    async fn relogin(&self) -> anyhow::Result<()> {
        let _guard = self.login_lock.lock().await;
        self.login().await
    }

    /// List torrents, optionally filtered client-side by a set of state strings.
    pub async fn list_torrents(
        &self,
        state_filter: Option<&[&str]>,
    ) -> anyhow::Result<Vec<TorrentInfo>> {
        let result = self.try_list_torrents().await;
        match result {
            Err(ref e) if is_forbidden(e) => {
                tracing::info!("Session expired, re-logging in");
                self.relogin().await?;
                self.try_list_torrents().await.map(|t| filter_by_state(t, state_filter))
            }
            Ok(torrents) => Ok(filter_by_state(torrents, state_filter)),
            Err(e) => Err(e),
        }
    }

    async fn try_list_torrents(&self) -> anyhow::Result<Vec<TorrentInfo>> {
        let url = format!("{}/api/v2/torrents/info", self.base_url);
        let resp = self.client.get(&url).send().await.context("list_torrents request failed")?;
        if resp.status() == StatusCode::FORBIDDEN {
            anyhow::bail!("forbidden");
        }
        let torrents: Vec<TorrentInfo> = resp.json().await.context("Failed to parse torrent list")?;
        Ok(torrents)
    }

    /// Add a torrent by magnet link or HTTP URL.
    pub async fn add_torrent_url(
        &self,
        url: &str,
        save_path: Option<&str>,
        paused: bool,
        category: Option<&str>,
    ) -> anyhow::Result<String> {
        let result = self.try_add_torrent_url(url, save_path, paused, category).await;
        match result {
            Err(ref e) if is_forbidden(e) => {
                self.relogin().await?;
                self.try_add_torrent_url(url, save_path, paused, category).await
            }
            other => other,
        }
    }

    async fn try_add_torrent_url(
        &self,
        url: &str,
        save_path: Option<&str>,
        paused: bool,
        category: Option<&str>,
    ) -> anyhow::Result<String> {
        let api_url = format!("{}/api/v2/torrents/add", self.base_url);
        let mut form: Vec<(&str, String)> = vec![("urls", url.to_string())];
        if let Some(sp) = save_path.filter(|s| !s.is_empty()) {
            form.push(("savepath", sp.to_string()));
        }
        if paused {
            form.push(("paused", "true".to_string()));
        }
        if let Some(cat) = category.filter(|s| !s.is_empty()) {
            form.push(("category", cat.to_string()));
        }

        let resp = self
            .client
            .post(&api_url)
            .form(&form)
            .send()
            .await
            .context("add_torrent_url request failed")?;
        if resp.status() == StatusCode::FORBIDDEN {
            anyhow::bail!("forbidden");
        }
        let body = resp.text().await?;
        if body.trim() == "Ok." || body.contains("Duplicate") {
            Ok("Torrent added successfully".to_string())
        } else {
            anyhow::bail!("qBittorrent rejected torrent: {}", body.trim())
        }
    }

    /// Add a torrent from raw .torrent file bytes.
    pub async fn add_torrent_file(
        &self,
        data: Vec<u8>,
        save_path: Option<&str>,
        paused: bool,
        category: Option<&str>,
    ) -> anyhow::Result<String> {
        let result = self.try_add_torrent_file(data.clone(), save_path, paused, category).await;
        match result {
            Err(ref e) if is_forbidden(e) => {
                self.relogin().await?;
                self.try_add_torrent_file(data, save_path, paused, category).await
            }
            other => other,
        }
    }

    async fn try_add_torrent_file(
        &self,
        data: Vec<u8>,
        save_path: Option<&str>,
        paused: bool,
        category: Option<&str>,
    ) -> anyhow::Result<String> {
        let api_url = format!("{}/api/v2/torrents/add", self.base_url);
        let mut form = multipart::Form::new().part(
            "torrents",
            multipart::Part::bytes(data)
                .file_name("upload.torrent")
                .mime_str("application/x-bittorrent")
                .context("Invalid MIME type")?,
        );
        if let Some(sp) = save_path.filter(|s| !s.is_empty()) {
            form = form.text("savepath", sp.to_string());
        }
        if paused {
            form = form.text("paused", "true");
        }
        if let Some(cat) = category.filter(|s| !s.is_empty()) {
            form = form.text("category", cat.to_string());
        }

        let resp = self
            .client
            .post(&api_url)
            .multipart(form)
            .send()
            .await
            .context("add_torrent_file request failed")?;
        if resp.status() == StatusCode::FORBIDDEN {
            anyhow::bail!("forbidden");
        }
        let body = resp.text().await?;
        if body.trim() == "Ok." || body.contains("Duplicate") {
            Ok("Torrent file added successfully".to_string())
        } else {
            anyhow::bail!("qBittorrent rejected torrent file: {}", body.trim())
        }
    }
}

fn filter_by_state(torrents: Vec<TorrentInfo>, filter: Option<&[&str]>) -> Vec<TorrentInfo> {
    match filter {
        None => torrents,
        Some(states) => torrents.into_iter().filter(|t| states.contains(&t.state.as_str())).collect(),
    }
}

fn is_forbidden(e: &anyhow::Error) -> bool {
    e.to_string().contains("forbidden")
}
