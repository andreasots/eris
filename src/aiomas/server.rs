use std::collections::HashMap;
use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
#[cfg(unix)]
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Error};
use futures_util::{future, FutureExt, Sink, SinkExt, Stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(not(unix))]
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::error;

use crate::aiomas::codec::{self, Exception, Request};

// Need to have the `Args` parameter on the trait otherwise the argument types are "unconstrained".
// But then we need a second trait and a struct to erase it...
// `rustc` pls
pub trait Route<Args> {
    fn handle(
        &self,
        args: Vec<Value>,
        kwargs: HashMap<String, Value>,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Exception>> + Send + 'static>>;
}

impl<Fun, Fut, R, E, T0> Route<(T0,)> for Fun
where
    Fun: Fn(T0) -> Fut + Sync,
    Fut: Future<Output = Result<R, E>> + Send + 'static,
    R: Serialize + Send + 'static,
    E: Debug + Send + 'static,
    T0: for<'a> Deserialize<'a> + Send,
{
    fn handle(
        &self,
        args: Vec<Value>,
        kwargs: HashMap<String, Value>,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Exception>> + Send + 'static>> {
        if !kwargs.is_empty() {
            return future::ready(Err(String::from("function takes no keyword arguments"))).boxed();
        }

        if args.len() != 1 {
            return future::ready(Err(format!(
                "function only takes a single argument ({} given)",
                args.len()
            )))
            .boxed();
        }

        let mut iter = args.into_iter();
        let arg0 = match serde_json::from_value(iter.next().unwrap()) {
            Ok(arg) => arg,
            Err(err) => {
                return future::ready(Err(format!("failed to deserialize argument 0: {err:?}")))
                    .boxed()
            }
        };

        self(arg0)
            .then(|res| async move {
                match res {
                    Ok(val) => serde_json::to_value(val)
                        .map_err(|err| format!("failed to serialize the return value: {err:?}")),
                    Err(err) => Err(format!("function returned an error: {err:?}")),
                }
            })
            .boxed()
    }
}

trait Handler {
    fn handle(
        &self,
        args: Vec<Value>,
        kwargs: HashMap<String, Value>,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Exception>> + Send + 'static>>;
}

struct RouteHandler<R, Args> {
    route: R,
    _marker: PhantomData<fn(Args)>,
}

impl<R, Args> Handler for RouteHandler<R, Args>
where
    R: Route<Args>,
{
    fn handle(
        &self,
        args: Vec<Value>,
        kwargs: HashMap<String, Value>,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Exception>> + Send + 'static>> {
        self.route.handle(args, kwargs)
    }
}

pub struct Server {
    methods: HashMap<String, Box<dyn Handler + Send + Sync + 'static>>,

    #[cfg(unix)]
    listener: UnixListener,

    #[cfg(not(unix))]
    listener: TcpListener,
}

impl Server {
    #[cfg(unix)]
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let listener = UnixListener::bind(path).context("failed to create a listening socket")?;

        Ok(Server { listener, methods: HashMap::new() })
    }

    #[cfg(not(unix))]
    pub async fn new(port: u16) -> Result<Self, Error> {
        use std::net::{IpAddr, Ipv6Addr, SocketAddr};

        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)), port);
        let listener =
            TcpListener::bind(&addr).await.context("failed to create a listening socket")?;

        Ok(Server { listener, methods: HashMap::new() })
    }

    pub fn register<Args: 'static>(
        &mut self,
        method: impl Into<String>,
        route: impl Route<Args> + Send + Sync + 'static,
    ) {
        self.methods.insert(method.into(), Box::new(RouteHandler { route, _marker: PhantomData }));
    }

    pub async fn serve(
        self,
        mut running: watch::Receiver<bool>,
        handler_tx: mpsc::Sender<JoinHandle<()>>,
    ) {
        let Server { methods, listener } = self;

        let methods = Arc::new(methods);

        loop {
            tokio::select! {
                _ = running.changed() => break,
                res = listener.accept() => match res {
                    Ok((socket, _remote_addr)) => {
                        let _ = handler_tx.send(tokio::spawn(Server::process(running.clone(), handler_tx.clone(), methods.clone(), codec::server(socket)))).await;
                    }
                    Err(error) => error!(?error, "Failed to accept an incoming connection"),
                },
            }
        }
    }

    async fn process<T>(
        mut running: watch::Receiver<bool>,
        handler_tx: mpsc::Sender<JoinHandle<()>>,
        methods: Arc<HashMap<String, Box<dyn Handler + Send + Sync + 'static>>>,
        transport: T,
    ) where
        T: Sink<(u64, Result<Value, Exception>), Error = Error>
            + Stream<Item = Result<(u64, Request), Error>>
            + Send
            + Sync
            + 'static,
    {
        let (mut sink, mut stream) = transport.split();
        let (tx, mut rx) = mpsc::channel(16);
        let _ = handler_tx
            .send(tokio::spawn(async move {
                while let Some(response) = rx.recv().await {
                    if let Err(error) = sink.send(response).await {
                        error!(?error, "Failed to send a response");
                        break;
                    }
                }
            }))
            .await;

        loop {
            tokio::select! {
                _ = running.changed() => break,
                // Probably not cancel-safe but we're not continuing anyway.
                req = stream.try_next() => match req {
                    Ok(Some((id, (method, args, kwargs)))) => {
                        let tx = tx.clone();
                        let future = match methods.get(&method) {
                            Some(handler) => handler.handle(args, kwargs),
                            None => async move { Err(format!("no such method: {}", method)) }.boxed(),
                        };

                        let _ = handler_tx
                            .send(tokio::spawn(async move {
                                let _ = tx.send((id, future.await)).await;
                            }))
                            .await;
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
}
