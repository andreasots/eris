use tokio::prelude::*;
use super::codec::{ClientCodec, Request, Exception};
use tokio::codec::Framed;
use std::path::PathBuf;
use failure::{self, Error, Fail, ResultExt};
use serde_json::Value;
use std::collections::HashMap;
use futures::channel::{mpsc, oneshot};
use tokio;
use std::mem;

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
        NewClient {
            path: path.into()
        }
    }

    pub async fn new_client(&self) -> Result<Client, Error> {
        Ok(Client::from_stream(await!(UnixStream::connect(&self.path))?))
    }
}

#[cfg(not(unix))]
impl NewClient {
    pub fn new(port: u16) -> NewClient {
        NewClient {
            port,
        }
    }

    async fn new_service(&self) -> Result<Client, Error> {
        use std::net::{IpAddr, Ipv6Addr, SocketAddr};
        
        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)), self.port);
        Ok(Client::from_stream(await!(TcpStream::connect(&addr))?))
    }
}

pub struct Client {
    channel: mpsc::Sender<(Request, oneshot::Sender<Result<Value, Exception>>)>,
}


impl Client {
    fn from_stream<S: AsyncRead + AsyncWrite + Send + 'static>(stream: S) -> Client {
        let (tx, rx) = mpsc::channel(16);

        let dispatch = Dispatch {
            next_request_id: 0,
            send_state: SendState::Waiting,
            channel: rx,
            pending: HashMap::new(),
            transport: Framed::new(stream, ClientCodec),
        };

        tokio::spawn(dispatch.map_err(|err| {eprintln!("error in dispatcher: {:?}", err); ()}));
        
        Client {
            channel: tx,
        }
    }

    pub async fn call(&mut self, req: Request) -> Result<Result<Value, Exception>, Error> {
        let (tx, rx) = oneshot::channel();
        await!(self.channel.send((req, tx)))?;
        Ok(await!(rx)?)
    }
}

enum SendState {
    Waiting,
    Sending(u64, Request),
    Flushing,
    Invalid,
}

struct Dispatch<T> {
    next_request_id: u64,
    send_state: SendState,
    channel: mpsc::Receiver<(Request, oneshot::Sender<Result<Value, Exception>>)>,
    pending: HashMap<u64, oneshot::Sender<Result<Value, Exception>>>,
    transport: T,
}

impl<T, E> Future for Dispatch<T>
    where T: Stream<Item=(u64, Result<Value, Exception>), Error=E> + Sink<SinkItem=(u64, Request), SinkError=E>,
        E: Fail,
{
    type Item = ();
    type Error = Error;

    fn poll(&mut self) -> Result<Async<()>, Error> {
        loop {
            match mem::replace(&mut self.send_state, SendState::Invalid) {
                SendState::Waiting => {
                    match self.channel.poll() {
                        Ok(Async::Ready(Some((request, channel)))) => {
                            let request_id = self.next_request_id;
                            self.next_request_id += 1;
                            self.pending.insert(request_id, channel);
                            self.send_state = SendState::Sending(request_id, request);
                        },
                        Ok(Async::Ready(None)) => {
                            eprintln!("sender closed");
                            return Ok(Async::Ready(()))
                        },
                        Ok(Async::NotReady) => {
                            self.send_state = SendState::Waiting;
                            break
                        },
                        Err(()) => return Err(failure::err_msg("outgoing request channel returned an error")),
                    }
                },
                SendState::Sending(request_id, request) => {
                    match self.transport.start_send((request_id, request)).context("failed to send request")? {
                        AsyncSink::Ready => self.send_state = SendState::Flushing,
                        AsyncSink::NotReady((request_id, request)) => {
                            self.send_state = SendState::Sending(request_id, request);
                            break;
                        }
                    }
                },
                SendState::Flushing => {
                    match self.transport.poll_complete().context("failed to flush request")? {
                        Async::Ready(()) => self.send_state = SendState::Waiting,
                        Async::NotReady => {
                            self.send_state = SendState::Flushing;
                            break;
                        }
                    }
                },
                SendState::Invalid => unreachable!(),
            }
        }

        loop {
            match self.transport.poll().context("failed to read response")? {
                Async::Ready(Some((request_id, response))) => {
                    if let Some(channel) = self.pending.remove(&request_id) {
                        let _ = channel.send(response);
                    }
                },
                Async::Ready(None) => {
                    eprintln!("transport closed");
                    return Ok(Async::Ready(()))
                }
                Async::NotReady => return Ok(Async::NotReady),
            }
        }
    }
}
