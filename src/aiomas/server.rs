use crate::aiomas::codec::{self, Exception, Request};
use anyhow::{Context, Error};
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::prelude::*;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::error;

#[cfg(unix)]
use tokio::net::UnixListener;
#[cfg(unix)]
use std::path::Path;

#[cfg(not(unix))]
use tokio::net::TcpListener;

pub trait Handler<C> {
    fn handle(
        &self,
        ctx: C,
        args: Vec<Value>,
        kwargs: HashMap<String, Value>,
    ) -> BoxFuture<'static, Result<Value, Exception>>;
}

impl<C, Fun, Fut> Handler<C> for Fun
where
    Fun: Fn(C, Vec<Value>, HashMap<String, Value>) -> Fut,
    Fut: Future<Output = Result<Value, Exception>> + Send + 'static,
{
    fn handle(
        &self,
        ctx: C,
        args: Vec<Value>,
        kwargs: HashMap<String, Value>,
    ) -> BoxFuture<'static, Result<Value, Exception>> {
        self(ctx, args, kwargs).boxed()
    }
}

pub struct Server<C: 'static> {
    methods: HashMap<String, &'static (dyn Handler<C> + Send + Sync + 'static)>,
    context: C,

    #[cfg(unix)]
    listener: UnixListener,

    #[cfg(not(unix))]
    listener: TcpListener,
}

impl<C: Clone + Send + 'static> Server<C> {
    #[cfg(unix)]
    pub fn new<P: AsRef<Path>>(path: P, context: C) -> Result<Server<C>, Error> {
        let listener = UnixListener::bind(path).context("failed to create a listening socket")?;

        Ok(Server { listener, methods: HashMap::new(), context })
    }

    #[cfg(not(unix))]
    pub async fn new(port: u16, context: C) -> Result<Server<C>, Error> {
        use std::net::{IpAddr, Ipv6Addr, SocketAddr};

        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)), port);
        let listener = TcpListener::bind(&addr).await.context("failed to create a listening socket")?;

        Ok(Server { listener, methods: HashMap::new(), context })
    }

    pub fn register(
        &mut self,
        method: impl Into<String>,
        handler: &'static (dyn Handler<C> + Send + Sync + 'static),
    ) {
        self.methods.insert(method.into(), handler);
    }

    pub async fn serve(self) {
        let Server { methods, listener, context } = self;

        let methods = Arc::new(methods);

        loop {
            match listener.accept().await {
                Ok((socket, _remote_addr)) => {
                    tokio::spawn(Server::process(
                        methods.clone(),
                        codec::server(socket),
                        context.clone(),
                    ));
                }
                Err(error) => error!(?error, "Failed to accept an incoming connection"),
            }
        }
    }

    async fn process<T>(
        methods: Arc<HashMap<String, &'static (dyn Handler<C> + Send + Sync + 'static)>>,
        transport: T,
        context: C,
    ) where
        T: Sink<(u64, Result<Value, Exception>), Error = Error>
            + Stream<Item = Result<(u64, Request), Error>>
            + Send
            + Sync
            + 'static,
    {
        let (mut sink, mut stream) = transport.split();
        let (tx, mut rx) = mpsc::channel(16);
        tokio::spawn(async move {
            while let Some(response) = rx.next().await {
                if let Err(error) = sink.send(response).await {
                    error!(?error, "Failed to send a response");
                    break;
                }
            }
        });

        loop {
            match stream.try_next().await {
                Ok(Some((id, (method, args, kwargs)))) => {
                    let mut tx = tx.clone();
                    let future = match methods.get(&method) {
                        Some(handler) => handler.handle(context.clone(), args, kwargs),
                        None => async move { Err(format!("no such method: {}", method)) }.boxed(),
                    };

                    tokio::spawn(async move {
                        let _ = tx.send((id, future.await)).await;
                    });
                }
                Ok(None) => break,
                Err(error) => {
                    error!(?error, "Failed to read a request");
                    break;
                }
            }
        }
    }
}
