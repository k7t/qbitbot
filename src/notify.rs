use crate::config::Config;
use crate::server::EventPayload;

/// Called when qBittorrent triggers the "Run external program" hook.
/// Sends the message string to the running bot's event server and exits.
pub async fn run(cfg: Config, message: String) -> anyhow::Result<()> {
    let url = format!("http://127.0.0.1:{}/event", cfg.event_server_port);
    let payload = EventPayload { message };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    match client.post(&url).json(&payload).send().await {
        Ok(resp) => {
            tracing::info!("Event delivered, server responded: {}", resp.status());
        }
        Err(e) => {
            // Bot may not be running — log and exit cleanly so qBittorrent isn't blocked.
            tracing::warn!("Could not deliver event to bot server (is it running?): {}", e);
        }
    }

    Ok(())
}
