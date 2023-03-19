use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use tokio::sync::watch::Receiver;
use tokio::sync::RwLock;
use tracing::error;
use twitch_api::twitch_oauth2::{AppAccessToken, TwitchToken};

pub async fn renew_helix(
    mut running: Receiver<bool>,
    helix_token: Arc<RwLock<AppAccessToken>>,
    http_client: Client,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(15 * 60));

    loop {
        tokio::select! {
            _ = running.changed() => break,
            _ = interval.tick() => {
                if helix_token.read().await.expires_in() < Duration::from_secs(60 * 60) {
                    if let Err(error) = helix_token.write().await.refresh_token(&http_client).await {
                        error!(?error, "failed to refresh the Twitch app token");
                    }
                }
            },
        }
    }
}
