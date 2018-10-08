use super::codec::{ClientCodec, Exception, Request};
use failure::{Error, Fail};
use futures::channel::{mpsc, oneshot};
use futures::compat::{Future01CompatExt, Stream01CompatExt};
use futures::prelude::*;
use futures::select;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio;
use tokio::codec::Framed;

use tokio::prelude::Sink as Sink01;
use tokio::prelude::Stream as Stream01;

#[cfg(unix)]
use tokio::net::unix::UnixStream;

#[cfg(not(unix))]
use tokio::net::TcpStream;

pub struct NewClient {
    #[cfg(unix)]
    path: PathBuf,

    #[cfg(not(unix))]
    port: u16,
}

#[cfg(unix)]
impl NewClient {
    pub fn new<P: Into<PathBuf>>(path: P) -> NewClient {
        NewClient { path: path.into() }
    }

    pub async fn new_client(&self) -> Result<Client, Error> {
        Ok(Client::from_stream(await!(
            UnixStream::connect(&self.path).compat()
        )?))
    }
}

#[cfg(not(unix))]
impl NewClient {
    pub fn new(port: u16) -> NewClient {
        NewClient { port }
    }

    pub async fn new_service(&self) -> Result<Client, Error> {
        use std::net::{IpAddr, Ipv6Addr, SocketAddr};

        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)), self.port);
        Ok(Client::from_stream(await!(
            TcpStream::connect(&addr).compat()
        )?))
    }
}

pub struct Client {
    channel: mpsc::Sender<(Request, oneshot::Sender<Result<Value, Exception>>)>,
}

impl Client {
    fn from_stream<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static>(
        stream: S,
    ) -> Client {
        let (tx, rx) = mpsc::channel(16);

        tokio::spawn(
            Client::dispatch(rx, Framed::new(stream, ClientCodec))
                .unit_error()
                .boxed()
                .compat(),
        );

        Client { channel: tx }
    }

    async fn dispatch<T, E>(
        mut channel: mpsc::Receiver<(Request, oneshot::Sender<Result<Value, Exception>>)>,
        stream: T,
    ) where
        T: Sink01<SinkItem = (u64, Request), SinkError = E>
            + Stream01<Item = (u64, Result<Value, Exception>), Error = E>,
        E: Fail,
    {
        let (mut sink, stream) = stream.split();
        let mut stream = stream.compat();

        let mut pending = HashMap::<u64, oneshot::Sender<Result<Value, Exception>>>::new();
        let mut next_request_id = 0;

        loop {
            let mut new_request = channel.next();
            let mut new_response = stream.next();

            select! {
                new_request => {
                    match new_request {
                        Some((request, channel)) => {
                            let request_id = next_request_id;
                            next_request_id += 1;

                            pending.insert(request_id, channel);

                            sink = match await!(sink.send((request_id, request)).compat()) {
                                Ok(sink) => sink,
                                Err(err) => {
                                    eprintln!("failed to send the request: {:?}", err);
                                    return;
                                },
                            };
                        },
                        None => return,
                    }
                },
                new_response => {
                    match new_response {
                        Some(Ok((request_id, response))) => {
                            if let Some(channel) = pending.remove(&request_id) {
                                let _ = channel.send(response);
                            }
                        },
                        Some(Err(err)) => {
                            eprintln!("failed to read a response: {:?}", err);
                            return;
                        },
                        None => return,
                    }
                },
            }
        }
    }

    pub async fn call(&mut self, req: Request) -> Result<Result<Value, Exception>, Error> {
        let (tx, rx) = oneshot::channel();
        await!(self.channel.send((req, tx)))?;
        Ok(await!(rx)?)
    }
}
