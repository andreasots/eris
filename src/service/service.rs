// From `tower-service`. Adapted for futures 0.3.

use std::future::Future;

pub trait Service {
    type Request;
    type Response;
    type Error;
    type Future<'a>: Future<Output = Result<Self::Response, Self::Error>> + 'a;
    type ReadyFuture<'a>: Future<Output = Result<(), Self::Error>> + 'a;

    fn ready(&mut self) -> Self::ReadyFuture;
    fn call(&mut self, req: Self::Request) -> Self::Future;
}

pub trait NewService {
    type Service: Service;
    type InitError;
    type Future<'a>: Future<Output = Result<Self::Service, Self::InitError>> + 'a;
    fn new_service(&self) -> Self::Future;
}
