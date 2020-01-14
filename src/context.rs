use serenity::http::client::Http;
use serenity::prelude::{RwLock, ShareMap};
use serenity::{CacheAndHttp, Client};
use std::sync::Arc;

#[derive(Clone)]
pub struct ErisContext {
    pub cache_and_http: Arc<CacheAndHttp>,
    pub data: Arc<RwLock<ShareMap>>,
}

impl ErisContext {
    pub fn from_client(client: &Client) -> ErisContext {
        ErisContext { cache_and_http: client.cache_and_http.clone(), data: client.data.clone() }
    }
}

impl AsRef<Http> for ErisContext {
    fn as_ref(&self) -> &Http {
        &self.cache_and_http.http
    }
}
