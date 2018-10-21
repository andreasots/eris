use crate::aiomas::codec::{Exception, Request, ServerCodec};
use failure::{Error, Fail, ResultExt};
use futures::channel::mpsc;
use futures::compat::{Future01CompatExt, Stream01CompatExt};
use futures::future::FutureObj;
use futures::prelude::*;
use serde_json::Value;
use slog::slog_error;
use slog_scope::error;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio;
use tokio::codec::Framed;
use tokio::prelude::Sink as Sink01;
use tokio::prelude::Stream as Stream01;

#[cfg(unix)]
use tokio::net::unix::UnixListener;

#[cfg(not(unix))]
use tokio::net::TcpListener;

pub trait Handler {
    fn handle(
        &self,
        args: Vec<Value>,
        kwargs: HashMap<String, Value>,
    ) -> FutureObj<'static, Result<Value, Exception>>;
}

impl<Fun, Fut> Handler for Fun
where
    Fun: Fn(Vec<Value>, HashMap<String, Value>) -> Fut,
    Fut: Future<Output = Result<Value, Exception>> + Send + 'static,
{
    fn handle(
        &self,
        args: Vec<Value>,
        kwargs: HashMap<String, Value>,
    ) -> FutureObj<'static, Result<Value, Exception>> {
        FutureObj::new(self(args, kwargs).boxed())
    }
}

pub struct Server {
    methods: HashMap<String, Box<Handler + Send + Sync>>,

    #[cfg(unix)]
    listener: UnixListener,

    #[cfg(not(unix))]
    listener: TcpListener,
}

impl Server {
    #[cfg(unix)]
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Server, Error> {
        let listener = UnixListener::bind(path).context("failed to create a listening socket")?;

        Ok(Server {
            listener,
            methods: HashMap::new(),
        })
    }

    #[cfg(not(unix))]
    pub fn new(port: u16) -> Result<Server, Error> {
        use std::net::{IpAddr, Ipv6Addr, SocketAddr};

        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)), port);
        let listener = TcpListener::bind(&addr).context("failed to create a listening socket")?;

        Ok(Server {
            listener,
            methods: HashMap::new(),
        })
    }

    pub fn register<S: Into<String>, H: Handler + Send + Sync + 'static>(
        &mut self,
        method: S,
        handler: H,
    ) {
        self.methods.insert(method.into(), Box::new(handler));
    }

    pub async fn serve(self) {
        let Server { methods, listener } = self;

        let mut listener = listener.incoming().compat().boxed();
        let methods = Arc::new(methods);

        loop {
            match await!(listener.try_next()) {
                Ok(Some(socket)) => {
                    tokio::spawn(
                        Server::process(methods.clone(), Framed::new(socket, ServerCodec))
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
        methods: Arc<HashMap<String, Box<Handler + Send + Sync + 'static>>>,
        transport: T,
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
        tokio::spawn(
            async move {
                loop {
                    match await!(rx.next()) {
                        Some(response) => {
                            sink = match await!(sink.send(response).compat()) {
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
            match await!(stream.try_next()) {
                Ok(Some((id, (method, args, kwargs)))) => {
                    let mut tx = tx.clone();
                    let future = match methods.get(&method) {
                        Some(handler) => handler.handle(args, kwargs),
                        None => FutureObj::new(
                            async move { Err(format!("no such method: {}", method)) }.boxed(),
                        ),
                    };

                    tokio::spawn(
                        future
                            .then(move |res| {
                                async move {
                                    let _ = await!(tx.send((id, res)));
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
