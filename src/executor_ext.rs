use futures::prelude::*;
use tokio::runtime::TaskExecutor;

pub trait ExecutorExt {
    fn block_on<T: Send + 'static, F: Future<Output = T> + Send + 'static>(&self, future: F) -> T;
}

impl ExecutorExt for TaskExecutor {
    fn block_on<T: Send + 'static, F: Future<Output = T> + Send + 'static>(&self, future: F) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        self.spawn(
            async move {
                let res = future.await;
                tx.send(res).unwrap();
            }
                .unit_error()
                .boxed()
                .compat(),
        );
        rx.recv().unwrap()
    }
}
