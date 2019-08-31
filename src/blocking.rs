use futures::compat::Future01CompatExt;
use tokio::prelude::future::poll_fn;
use tokio_threadpool::BlockingError;

/// Exit the runtime to run a blocking section code. Wrapper around `tokio_threadpool::blocking`.
pub async fn blocking<T, F: FnOnce() -> T>(func: F) -> Result<T, BlockingError> {
    let mut func = Some(func);
    poll_fn(|| tokio_threadpool::blocking(|| func.take().expect("future already complete")()))
        .compat()
        .await
}
