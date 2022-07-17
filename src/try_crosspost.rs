use anyhow::Error;
use serenity::async_trait;
use serenity::client::Cache;
use serenity::http::CacheHttp;
use serenity::model::channel::{ChannelType, Message};
use serenity::model::ModelError;
use serenity::Error as SerenityError;

#[async_trait]
pub trait TryCrosspost {
    // FIXME: perhaps the `try_` prefix is not correct here
    async fn try_crosspost(
        &self,
        ctx: impl CacheHttp + AsRef<Cache> + 'async_trait,
    ) -> Result<(), Error>;
}

#[async_trait]
impl TryCrosspost for Message {
    async fn try_crosspost(
        &self,
        ctx: impl CacheHttp + AsRef<Cache> + 'async_trait,
    ) -> Result<(), Error> {
        let channel = match self.channel(&ctx).await?.guild() {
            Some(channel) => channel,
            None => return Ok(()),
        };

        if channel.kind != ChannelType::News {
            return Ok(());
        }

        match self.crosspost(ctx).await {
            Ok(_) => Ok(()),
            Err(SerenityError::Model(ModelError::MessageAlreadyCrossposted)) => Ok(()),
            Err(SerenityError::Model(ModelError::CannotCrosspostMessage)) => Ok(()),
            Err(err) => Err(Error::from(err)),
        }
    }
}
