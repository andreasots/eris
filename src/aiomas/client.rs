use std::collections::HashMap;
#[cfg(unix)]
use std::path::PathBuf;

use anyhow::{Context, Error};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde_json::Value;
#[cfg(not(unix))]
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::error;

use super::codec::{self, Exception, Request};

pub struct Connector {
    running: watch::Receiver<bool>,
    handler_tx: mpsc::Sender<JoinHandle<()>>,

    #[cfg(unix)]
    path: PathBuf,

    #[cfg(not(unix))]
    port: u16,
}

#[cfg(unix)]
impl Connector {
    pub fn new<P: Into<PathBuf>>(
        running: watch::Receiver<bool>,
        handler_tx: mpsc::Sender<JoinHandle<()>>,
        path: P,
    ) -> Connector {
        Connector { running, handler_tx, path: path.into() }
    }

    pub async fn connect(&self) -> Result<Client, Error> {
        Ok(Client::from_stream(
            self.running.clone(),
            self.handler_tx.clone(),
            UnixStream::connect(&self.path).await?,
        )
        .await)
    }
}

#[cfg(not(unix))]
impl Connector {
    pub fn new(
        running: Receiver<bool>,
        handler_tx: Sender<JoinHandle<()>>,
        port: u16,
    ) -> Connector {
        Connector { running, handler_tx, port }
    }

    pub async fn connect(&self) -> Result<Client, Error> {
        use std::net::{IpAddr, Ipv6Addr, SocketAddr};

        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)), self.port);
        Ok(Client::from_stream(
            self.running.clone(),
            self.handler_tx.clone(),
            TcpStream::connect(&addr).await?,
        )
        .await)
    }
}

pub struct Client {
    channel: mpsc::Sender<(Request, oneshot::Sender<Result<Value, Exception>>)>,
}

impl Client {
    async fn from_stream<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static>(
        running: watch::Receiver<bool>,
        handler_tx: mpsc::Sender<JoinHandle<()>>,
        stream: S,
    ) -> Client {
        let (tx, rx) = mpsc::channel(16);

        let _ = handler_tx
            .send(tokio::spawn(Client::dispatch(running, rx, codec::client(stream))))
            .await;

        Client { channel: tx }
    }

    async fn dispatch<T>(
        mut running: watch::Receiver<bool>,
        mut channel: mpsc::Receiver<(Request, oneshot::Sender<Result<Value, Exception>>)>,
        stream: T,
    ) where
        T: Sink<(u64, Request), Error = Error>
            + Stream<Item = Result<(u64, Result<Value, Exception>), Error>>,
    {
        let (mut sink, mut stream) = stream.split();

        let mut pending = HashMap::<u64, oneshot::Sender<Result<Value, Exception>>>::new();
        let mut next_request_id = 0;

        while *running.borrow() || !pending.is_empty() {
            tokio::select! {
                _ = running.changed() => continue,
                new_request = channel.recv() => {
                    match new_request {
                        Some((request, channel)) => {
                            let request_id = next_request_id;
                            next_request_id += 1;

                            pending.insert(request_id, channel);

                            if let Err(error) = sink.send((request_id, request)).await {
                                error!(?error, "Failed to send the request");
                                return;
                            };
                        },
                        None => return,
                    }
                },
                new_response = stream.next() => {
                    match new_response {
                        Some(Ok((request_id, response))) => {
                            if let Some(channel) = pending.remove(&request_id) {
                                let _ = channel.send(response);
                            }
                        },
                        Some(Err(error)) => {
                            error!(?error, "Failed to read a response");
                            return;
                        },
                        None => return,
                    }
                },
            }
        }
    }

    pub async fn call(
        &mut self,
        req: Request,
    ) -> Result<oneshot::Receiver<Result<Value, Exception>>, Error> {
        let (tx, rx) = oneshot::channel();

        self.channel.send((req, tx)).await.context("failed to queue the request")?;

        Ok(rx)
    }
}

// TODO: #[cfg(not(unix))]
#[cfg(all(test, unix))]
mod tests {
    use std::collections::HashMap;

    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    use super::Client;

    #[tokio::test]
    async fn smoke_test() {
        const REQUEST: &[u8] =
            b"\x00\x00\x00\x14[0,0,[\"test\",[],{}]]\x00\x00\x00\x14[0,1,[\"test\",[],{}]]";
        const RESPONSE: &[u8] = b"\x00\x00\x00\x09[1, 1, 1]\x00\x00\x00\x09[1, 0, 0]";

        let (read, mut write) = UnixStream::pair().expect("failed to create a socket pair");

        let (_running_tx, running_rx) = tokio::sync::watch::channel(true);
        let (handles_tx, _handles_rx) = tokio::sync::mpsc::channel(8);

        let mut client = Client::from_stream(running_rx, handles_tx, read).await;

        let first =
            client.call((String::from("test"), vec![], HashMap::new())).await.expect("queue first");
        let second = client
            .call((String::from("test"), vec![], HashMap::new()))
            .await
            .expect("queue second");

        let mut buf = [0; REQUEST.len()];
        write.read_exact(&mut buf[..]).await.expect("failed to read request");
        assert_eq!(&buf[..], REQUEST);
        write.write_all(RESPONSE).await.expect("failed to write response");

        assert_eq!(first.await.expect("first"), Ok(Value::Number(0.into())));
        assert_eq!(second.await.expect("second"), Ok(Value::Number(1.into())));
    }
}
