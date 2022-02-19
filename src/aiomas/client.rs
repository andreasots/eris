use super::codec::{self, Exception, Request};
use anyhow::{Context, Error};
use futures::channel::{mpsc, oneshot};
use futures::prelude::*;
use futures::select;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::error;

#[cfg(unix)]
use tokio::net::UnixStream;

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
        Ok(Client::from_stream(UnixStream::connect(&self.path).await?))
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
        Ok(Client::from_stream(TcpStream::connect(&addr).await?))
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

        tokio::spawn(Client::dispatch(rx, codec::client(stream)));

        Client { channel: tx }
    }

    async fn dispatch<T>(
        mut channel: mpsc::Receiver<(Request, oneshot::Sender<Result<Value, Exception>>)>,
        stream: T,
    ) where
        T: Sink<(u64, Request), Error = Error>
            + Stream<Item = Result<(u64, Result<Value, Exception>), Error>>,
    {
        let (mut sink, mut stream) = stream.split();

        let mut pending = HashMap::<u64, oneshot::Sender<Result<Value, Exception>>>::new();
        let mut next_request_id = 0;

        loop {
            select! {
                new_request = channel.next().fuse() => {
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
                new_response = stream.next().fuse() => {
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
    use super::Client;
    use serde_json::Value;
    use std::collections::HashMap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn smoke_test() {
        const REQUEST: &[u8] =
            b"\x00\x00\x00\x14[0,0,[\"test\",[],{}]]\x00\x00\x00\x14[0,1,[\"test\",[],{}]]";
        const RESPONSE: &[u8] = b"\x00\x00\x00\x09[1, 1, 1]\x00\x00\x00\x09[1, 0, 0]";

        let (read, mut write) = UnixStream::pair().expect("failed to create a socket pair");

        let mut client = Client::from_stream(read);

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
