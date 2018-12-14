use super::codec::{ClientCodec, Exception, Request};
use failure::{Error, Fail};
use futures::channel::{mpsc, oneshot};
use futures::compat::{Future01CompatExt, Stream01CompatExt};
use futures::prelude::*;
use futures::select;
use serde_json::Value;
use slog::slog_error;
use slog_scope::error;
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
        Ok(Client::from_stream(await!(UnixStream::connect(
            &self.path
        )
        .compat())?))
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

#[derive(Clone)]
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
            select! {
                new_request = channel.next().fuse() => {
                    match new_request {
                        Some((request, channel)) => {
                            let request_id = next_request_id;
                            next_request_id += 1;

                            pending.insert(request_id, channel);

                            sink = match await!(sink.send((request_id, request)).compat()) {
                                Ok(sink) => sink,
                                Err(err) => {
                                    error!("Failed to send the request"; "error" => ?err);
                                    return;
                                },
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
                        Some(Err(err)) => {
                            error!("Failed to read a response"; "error" => ?err);
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

// TODO: #[cfg(not(unix))]
#[cfg(all(test, unix))]
mod tests {
    use super::Client;
    use failure::{Error, ResultExt};
    use futures::compat::Future01CompatExt;
    use futures::poll;
    use futures::prelude::*;
    use pin_utils::pin_mut;
    use serde_json::Value;
    use std::collections::HashMap;
    use tokio::io;
    use tokio::net::UnixStream;
    use tokio::runtime::Runtime;

    #[test]
    fn smoke_test() {
        const REQUEST: &[u8] =
            b"\x00\x00\x00\x14[0,0,[\"test\",[],{}]]\x00\x00\x00\x14[0,1,[\"test\",[],{}]]";
        const RESPONSE: &[u8] = b"\x00\x00\x00\x09[1, 1, 1]\x00\x00\x00\x09[1, 0, 0]";

        let mut runtime = Runtime::new().unwrap();
        runtime
            .block_on::<_, (), Error>(
                async move {
                    let (read, mut write) =
                        UnixStream::pair().context("failed to create a socket pair")?;

                    let mut client = Client::from_stream(read);
                    let mut client2 = client.clone();

                    let first = client2.call((String::from("test"), vec![], HashMap::new()));
                    let second = client.call((String::from("test"), vec![], HashMap::new()));

                    pin_mut!(first);
                    pin_mut!(second);

                    poll!(&mut first);
                    poll!(&mut second);

                    let mut buf = [0; REQUEST.len()];
                    await!(io::read_exact(&mut write, &mut buf[..]).compat())
                        .context("failed to read request")?;
                    assert_eq!(&buf[..], REQUEST);
                    await!(io::write_all(&mut write, RESPONSE).compat())
                        .context("failed to write response")?;

                    assert_eq!(await!(first).context("first")?, Ok(Value::Number(0.into())));
                    assert_eq!(
                        await!(second).context("second")?,
                        Ok(Value::Number(1.into()))
                    );

                    Ok(())
                }
                    .boxed()
                    .compat(),
            )
            .unwrap();
    }
}
