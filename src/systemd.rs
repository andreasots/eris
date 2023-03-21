use std::ffi::{c_char, c_int, CString};

use anyhow::{Context, Error};
use libloading::Library;

pub struct Notify {
    libsystemd: Library,
}

impl Notify {
    pub fn new() -> Result<Self, Error> {
        let libsystemd = unsafe {
            Library::new(libloading::library_filename("systemd"))
                .context("failed to load libsystemd")?
        };

        Ok(Self { libsystemd })
    }

    fn notify(&self, state: &str) -> Result<bool, Error> {
        let state = CString::new(state).context("failed to convert the state")?;
        unsafe {
            let sd_notify = self
                .libsystemd
                .get::<unsafe extern "C" fn(c_int, *const c_char) -> c_int>(b"sd_notify")
                .context("failed to find `sd_notify` in libsystemd")?;
            match sd_notify(0, state.as_ptr()) {
                error if error < 0 => {
                    return Err(Error::from(std::io::Error::from_raw_os_error(-error))
                        .context("`sd_notify(3)` failed"))
                }
                0 => Ok(false),
                _ => Ok(true),
            }
        }
    }

    pub fn ready(&self) -> Result<(), Error> {
        self.notify("READY=1")?;
        Ok(())
    }

    pub fn feed_watchdog(&self) -> Result<(), Error> {
        self.notify("WATCHDOG=1")?;
        Ok(())
    }
}
