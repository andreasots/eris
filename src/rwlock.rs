use futures::lock::{Mutex, MutexGuard};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::ops::{Deref, DerefMut};

/// A reader-writer lock
///
/// Based on https://en.wikipedia.org/wiki/Readers%E2%80%93writer_lock#Using_two_mutexes
pub struct RwLock<T> {
    readers: AtomicUsize,
    lock: Mutex<()>,
    data: UnsafeCell<T>,
}

impl<T> RwLock<T> {
    pub fn new(value: T) -> RwLock<T> {
        RwLock {
            readers: AtomicUsize::new(0),
            lock: Mutex::new(()),
            data: UnsafeCell::new(value),
        }
    }

    pub async fn read(&self) -> ReadGuard<T> {
        // TODO: possbly Aquire?
        let b = self.readers.fetch_add(1, Ordering::SeqCst);

        let guard = if b == 0 {
            Some(self.lock.lock().await)
        } else {
            None
        };

        ReadGuard {
            lock: self,
            _guard: guard,
        }
    }

    pub async fn write(&self) -> WriteGuard<T> {
        WriteGuard {
            lock: self,
            _guard: self.lock.lock().await,
        }
    }
}

unsafe impl<T: Send> Send for RwLock<T> {}
unsafe impl<T: Send> Sync for RwLock<T> {}

pub struct ReadGuard<'a, T> {
    lock: &'a RwLock<T>,
    _guard: Option<MutexGuard<'a, ()>>,
}

impl<T> Deref for ReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe {
            &*self.lock.data.get()
        }
    }
}

impl<T> Drop for ReadGuard<'_, T> {
    fn drop(&mut self) {
        // TODO: possbly Release?
        self.lock.readers.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct WriteGuard<'a, T> {
    lock: &'a RwLock<T>,
    _guard: MutexGuard<'a, ()>,
}

impl<T> Deref for WriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe {
            &*self.lock.data.get()
        }
    }
}

impl<T> DerefMut for WriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe {
            &mut *self.lock.data.get()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RwLock;
    use failure::{Error, Compat};
    use futures::{FutureExt, TryFutureExt};
    use tokio::runtime::Runtime;
    use tokio::util::FutureExt as TokioFutureExt;
    use tokio::prelude::Future;
    use std::time::Duration;

    const DEADLOCK_TIMEOUT: Duration = Duration::from_millis(10);

    #[test]
    fn two_reads() {
        let lock = RwLock::new(0u8);

        Runtime::new().unwrap()
            .block_on::<_, _, Error>(
                async move {
                    let _guard1 = lock.read().await;
                    let _guard2 = lock.read().await;
                    Ok::<_, Compat<Error>>(())
                }
                    .boxed()
                    .compat()
                    .timeout(DEADLOCK_TIMEOUT)
                    .from_err()
            )
            .unwrap();
    }

    #[test]
    fn two_writes() {
        let lock = RwLock::new(0u8);

        Runtime::new().unwrap()
            .block_on::<_, _, Error>(
                async move {
                    let _guard1 = lock.write().await;
                    let _guard2 = lock.write().await;
                    Ok::<_, Compat<Error>>(())
                }
                    .boxed()
                    .compat()
                    .timeout(DEADLOCK_TIMEOUT)
                    .then(|res| {
                        match res {
                            Ok(()) => Err(failure::err_msg("locking succeeded, two mutable aliases to the same variable")),
                            Err(err) => {
                                if err.is_elapsed() {
                                    Ok(())
                                } else {
                                    Err(err.into())
                                }
                            }
                        }
                    })
            )
            .unwrap();
    }

    #[test]
    fn a_read_and_a_write() {
        let lock = RwLock::new(0u8);

        Runtime::new().unwrap()
            .block_on::<_, _, Error>(
                async move {
                    let _guard1 = lock.read().await;
                    let _guard2 = lock.write().await;
                    Ok::<_, Compat<Error>>(())
                }
                    .boxed()
                    .compat()
                    .timeout(DEADLOCK_TIMEOUT)
                    .then(|res| {
                        match res {
                            Ok(()) => Err(failure::err_msg("locking succeeded, two mutable aliases to the same variable")),
                            Err(err) => {
                                if err.is_elapsed() {
                                    Ok(())
                                } else {
                                    Err(err.into())
                                }
                            }
                        }
                    })
            )
            .unwrap();
    }

    #[test]
    fn a_write_and_a_read() {
        let lock = RwLock::new(0u8);

        Runtime::new().unwrap()
            .block_on::<_, _, Error>(
                async move {
                    let _guard1 = lock.write().await;
                    let _guard2 = lock.read().await;
                    Ok::<_, Compat<Error>>(())
                }
                    .boxed()
                    .compat()
                    .timeout(DEADLOCK_TIMEOUT)
                    .then(|res| {
                        match res {
                            Ok(()) => Err(failure::err_msg("locking succeeded, two mutable aliases to the same variable")),
                            Err(err) => {
                                if err.is_elapsed() {
                                    Ok(())
                                } else {
                                    Err(err.into())
                                }
                            }
                        }
                    })
            )
            .unwrap();
    }
}