use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::error;

pub fn wait_for_outstanding(
    running: watch::Receiver<bool>,
) -> (JoinHandle<()>, mpsc::Sender<JoinHandle<()>>) {
    let (tx, rx) = mpsc::channel(8);

    (tokio::spawn(waiter(running, rx)), tx)
}

async fn waiter(mut running: watch::Receiver<bool>, mut rx: mpsc::Receiver<JoinHandle<()>>) {
    let mut tasks = FuturesUnordered::new();
    loop {
        tokio::select! {
            _ = running.changed() => break,
            task = rx.recv() => {
                if let Some(task) = task {
                    tasks.push(task);
                } else {
                    break
                }
            },
            Some(res) = tasks.next() => {
                if let Err(error) = res {
                    error!(?error, "handler failed");
                }
            },
        }
    }

    while let Some(res) = tasks.next().await {
        if let Err(error) = res {
            error!(?error, "handler failed");
        }
    }
}
