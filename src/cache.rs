use tokio::sync::watch;
use twilight_cache_inmemory::InMemoryCache;
use twilight_model::gateway::event::Event;
use twilight_model::id::marker::GuildMarker;
use twilight_model::id::Id;

/// A wrapper around [InMemoryCache] to prevent holding on to references to the cached data across
/// yield points.
pub struct Cache {
    cache: InMemoryCache,
    guild_id: Id<GuildMarker>,
    ready: watch::Sender<bool>,
}

impl Cache {
    pub fn new(guild_id: Id<GuildMarker>) -> Self {
        Self { cache: InMemoryCache::new(), ready: watch::Sender::new(false), guild_id }
    }

    pub fn with<T>(&self, f: impl FnOnce(&InMemoryCache) -> T) -> T {
        f(&self.cache)
    }

    pub fn update(&self, event: &Event) {
        self.cache.update(event);

        if let Event::GuildCreate(event) = event {
            if event.id() == self.guild_id {
                self.ready.send_replace(true);
            }
        }
    }

    pub async fn wait_until_ready(&self) {
        if let Err(_) = self.ready.subscribe().wait_for(|is_ready| *is_ready).await {
            unreachable!("`self.ready` is closed")
        }
    }
}
