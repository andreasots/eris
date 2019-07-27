use crate::aiomas::codec::{Exception, Request, ServerCodec};
use failure::{Error, Fail, ResultExt};
use futures::channel::mpsc;
use futures::compat::{Future01CompatExt, Stream01CompatExt};
use futures::future::BoxFuture;
use futures::prelude::*;
use serde_json::Value;
use slog_scope::error;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::codec::Framed;
use tokio::prelude::Sink as Sink01;
use tokio::prelude::Stream as Stream01;
use tokio::runtime::TaskExecutor;

#[cfg(unix)]
use tokio::net::unix::UnixListener;

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
    executor: TaskExecutor,
    context: C,

    #[cfg(unix)]
    listener: UnixListener,

    #[cfg(not(unix))]
    listener: TcpListener,
}

impl<C: Clone + Send + 'static> Server<C> {
    #[cfg(unix)]
    pub fn new<P: AsRef<Path>>(
        path: P,
        executor: TaskExecutor,
        context: C,
    ) -> Result<Server<C>, Error> {
        let listener = UnixListener::bind(path).context("failed to create a listening socket")?;

        Ok(Server {
            listener,
            methods: HashMap::new(),
            context,
            executor,
        })
    }

    #[cfg(not(unix))]
    pub fn new(port: u16, executor: TaskExecutor, context: C) -> Result<Server<C>, Error> {
        use std::net::{IpAddr, Ipv6Addr, SocketAddr};

        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)), port);
        let listener = TcpListener::bind(&addr).context("failed to create a listening socket")?;

        Ok(Server {
            listener,
            methods: HashMap::new(),
            context,
            executor,
        })
    }

    pub fn register(
        &mut self,
        method: impl Into<String>,
        handler: &'static (dyn Handler<C> + Send + Sync + 'static),
    ) {
        self.methods.insert(method.into(), handler);
    }

    pub async fn serve(self) {
        let Server {
            methods,
            listener,
            executor,
            context,
        } = self;

        let mut listener = listener.incoming().compat().boxed();
        let methods = Arc::new(methods);

        loop {
            match listener.try_next().await {
                Ok(Some(socket)) => {
                    executor.spawn(
                        Server::process(
                            methods.clone(),
                            Framed::new(socket, ServerCodec),
                            executor.clone(),
                            context.clone(),
                        )
                        .unit_error()
                        .boxed()
                        .compat(),
                    );
                }
                Ok(None) => return,
                Err(err) => error!("Failed to accept an incoming connection"; "error" => ?err),
            }
        }
    }

    async fn process<T, E>(
        methods: Arc<HashMap<String, &'static (dyn Handler<C> + Send + Sync + 'static)>>,
        transport: T,
        executor: TaskExecutor,
        context: C,
    ) where
        T: Sink01<SinkItem = (u64, Result<Value, Exception>), SinkError = E>
            + Stream01<Item = (u64, Request), Error = E>
            + Send
            + Sync
            + 'static,
        E: Fail,
    {
        let (mut sink, stream) = transport.split();
        let (tx, mut rx) = mpsc::channel(16);
        executor.spawn(
            async move {
                loop {
                    match rx.next().await {
                        Some(response) => {
                            sink = match sink.send(response).compat().await {
                                Ok(sink) => sink,
                                Err(err) => {
                                    error!("Failed to send a response"; "error" => ?err);
                                    break;
                                }
                            };
                        }
                        None => break,
                    }
                }
            }
                .unit_error()
                .boxed()
                .compat(),
        );
        let mut stream = stream.compat();

        loop {
            match stream.try_next().await {
                Ok(Some((id, (method, args, kwargs)))) => {
                    let mut tx = tx.clone();
                    let future = match methods.get(&method) {
                        Some(handler) => handler.handle(context.clone(), args, kwargs),
                        None => async move { Err(format!("no such method: {}", method)) }.boxed(),
                    };

                    executor.spawn(
                        future
                            .then(move |res| {
                                async move {
                                    let _ = tx.send((id, res)).await;
                                }
                            })
                            .unit_error()
                            .boxed()
                            .compat(),
                    );
                }
                Ok(None) => break,
                Err(err) => {
                    error!("Failed to read a request"; "error" => ?err);
                    break;
                }
            }
        }
    }
}
