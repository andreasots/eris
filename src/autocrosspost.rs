use twilight_gateway::Event;
use twilight_http::Client;
use twilight_model::channel::ChannelType;

use crate::cache::Cache;

pub async fn on_event(cache: &Cache, discord: &Client, event: &Event) {
    match event {
        Event::MessageCreate(event) => {
            // Don't crosspost messages from ourselves.
            if Some(event.author.id) == cache.with(|cache| Some(cache.current_user()?.id)) {
                return;
            }

            if let Some(ChannelType::GuildAnnouncement) =
                cache.with(|cache| Some(cache.channel(event.channel_id)?.kind))
            {
                if let Err(error) = discord.crosspost_message(event.channel_id, event.id).await {
                    tracing::error!(
                        message.id = event.id.get(),
                        message.channel_id = event.channel_id.get(),
                        ?error,
                        "failed to autocrosspost a message",
                    )
                }
            }
        }
        _ => (),
    }
}
