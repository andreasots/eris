use crate::service::{NewService, Service};
use std::future::Future;

pub enum Error<T: NewService> {
    Connecting(T::InitError),
    Service(<T::Service as Service>::Error),
    NotReady,
}

enum State<T: NewService> {
    Idle,
    Connecting(T::Future),
    Connected(T::Service),
}

pub struct Reconnect<T: NewService> {
    factory: T,
    state: State<T>,
}

impl<T: NewService> Reconnect<T> {
    pub fn new(factory: T) -> Reconnect<T> {
        Reconnect {
            factory,
            state: State::Idle,
        }
    }
}

impl<T: NewService> Service for Reconnect<T> {
    type Request = <T::Service as Service>::Request;
    type Response = <T::Service as Service>::Response;
    type Error = Error<T>;
    existential type ReadyFuture<'a>: Future<Output = Result<(), Self::Error>> + 'a;
    existential type Future<'a>: Future<Output = Result<<T::Service as Service>::Response, Error<T>>> + 'a;

    fn ready(&mut self) -> Self::ReadyFuture {
        async {
            Ok(())
        }
    }

    fn call(&mut self, req: Self::Request) -> Self::Future {
        async {
            match self.state {
                State::Connected(service) => await!(service.call(req)).map_err(Error::Service),
                _ => Err(Error::NotReady),
            }
        }
    }
}
