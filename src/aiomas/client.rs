use std::collections::HashMap;
use std::future::{ready, Future, Ready};
use std::net::Ipv6Addr;
#[cfg(unix)]
use std::path::PathBuf;
use std::pin::Pin;
use std::task::Poll;

use anyhow::Error;
use futures_util::future::{Either, ErrInto};
use futures_util::{Sink, SinkExt, Stream, StreamExt, TryFutureExt};
use serde_json::Value;
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::PollSender;
use tower::Service;
use tracing::error;

use super::codec::{self, Exception, Request};

#[derive(Clone)]
pub struct MakeClient {
    running: watch::Receiver<bool>,
    handler_tx: mpsc::Sender<JoinHandle<()>>,
}

impl MakeClient {
    pub fn new(
        running: watch::Receiver<bool>,
        handler_tx: mpsc::Sender<JoinHandle<()>>,
    ) -> MakeClient {
        MakeClient { running, handler_tx }
    }
}

#[cfg(unix)]
impl Service<PathBuf> for MakeClient {
    type Response = Client;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + Sync>>;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, path: PathBuf) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            let connection = UnixStream::connect(&path).await?;

            Ok(Client::from_stream(this.running.clone(), this.handler_tx.clone(), connection).await)
        })
    }
}

impl Service<u16> for MakeClient {
    type Response = Client;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + Sync>>;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, port: u16) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            let connection = TcpStream::connect(&(Ipv6Addr::LOCALHOST, port)).await?;

            Ok(Client::from_stream(this.running.clone(), this.handler_tx.clone(), connection).await)
        })
    }
}

pub struct Client {
    channel: PollSender<(Request, oneshot::Sender<Result<Value, Exception>>)>,
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

        Client { channel: PollSender::new(tx) }
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
}

impl Service<Request> for Client {
    type Response = Result<Value, Exception>;
    type Error = Error;
    type Future = Either<
        Ready<Result<Self::Response, Self::Error>>,
        ErrInto<oneshot::Receiver<Self::Response>, Self::Error>,
    >;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.channel.poll_reserve(cx).map_err(|error| {
            Error::from(error).context("failed to reserve a slot on the request queue")
        })
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let (tx, rx) = oneshot::channel();

        if let Err(error) = self.channel.send_item((req, tx)) {
            return Either::Left(ready(Err(
                Error::from(error).context("failed to queue the request")
            )));
        };

        Either::Right(rx.err_into())
    }
}

// TODO: #[cfg(not(unix))]
#[cfg(all(test, unix))]
mod tests {
    use std::collections::HashMap;

    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;
    use tower::Service;

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

        std::future::poll_fn(|cx| client.poll_ready(cx)).await.unwrap();
        let first = client.call((String::from("test"), vec![], HashMap::new()));

        std::future::poll_fn(|cx| client.poll_ready(cx)).await.unwrap();
        let second = client.call((String::from("test"), vec![], HashMap::new()));

        let mut buf = [0; REQUEST.len()];
        write.read_exact(&mut buf[..]).await.expect("failed to read request");
        assert_eq!(&buf[..], REQUEST);
        write.write_all(RESPONSE).await.expect("failed to write response");

        assert_eq!(first.await.expect("first"), Ok(Value::Number(0.into())));
        assert_eq!(second.await.expect("second"), Ok(Value::Number(1.into())));
    }
}
