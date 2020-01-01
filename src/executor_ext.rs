use futures::prelude::*;
use tokio::runtime::Handle;

pub trait ExecutorExt {
    fn block_on<T: Send + 'static, F: Future<Output = T> + Send + 'static>(&self, future: F) -> T;
}

impl ExecutorExt for Handle {
    fn block_on<T: Send + 'static, F: Future<Output = T> + Send + 'static>(&self, future: F) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        self.spawn(future.map(move |res| tx.send(res).unwrap()));
        rx.recv().unwrap()
    }
}
