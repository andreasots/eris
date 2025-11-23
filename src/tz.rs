use anyhow::Error;
use chrono::TimeZone;

pub trait LoadTimeZone: TimeZone + Sized {
    fn from_name(name: &str) -> Result<Self, Error>;
    fn from_name_case_insensitive(name: &str) -> Result<Self, Error>;
    fn utc() -> Self;
}

#[cfg(unix)]
pub type Tz = tzfile::ArcTz;

#[cfg(unix)]
mod unix {
    use std::io::ErrorKind;
    use std::sync::OnceLock;

    use anyhow::Context;
    use anyhow::Error;
    use regex::Regex;

    use super::{LoadTimeZone, Tz};

    fn from_path(name: &str, path: &std::path::Path) -> Result<Tz, Error> {
        let source = match std::fs::read(path) {
            Err(err) if err.kind() == ErrorKind::NotFound => anyhow::bail!("no such timezone"),
            res => res.context("failed to read the zone file")?,
        };

        match tzfile::Tz::parse(name, &source) {
            Err(tzfile::Error::InvalidMagic) => anyhow::bail!("no such timezone"),
            res => Ok(Tz::new(res.context("invalid zone file")?)),
        }
    }

    fn tzdir() -> std::path::PathBuf {
        match std::env::var_os("TZDIR") {
            Some(dir) => std::path::PathBuf::from(dir),
            None => std::path::PathBuf::from("/usr/share/zoneinfo"),
        }
    }

    fn validate_name(name: &str) -> bool {
        static REGEX: OnceLock<Regex> = OnceLock::new();
        let regex = REGEX.get_or_init(|| {
            Regex::new(r"^[a-zA-Z0-9_+-]{1,14}(/[a-zA-Z0-9_+-]{1,14}){0,2}$").unwrap()
        });

        regex.is_match(name)
            && !name.starts_with("right/")
            && !name.starts_with("posix/")
            && name != "posixrules"
    }

    impl LoadTimeZone for Tz {
        fn from_name(name: &str) -> Result<Self, Error> {
            anyhow::ensure!(validate_name(name), "no such timezone");

            from_path(name, &tzdir().join(name))
        }

        fn from_name_case_insensitive(name: &str) -> Result<Self, Error> {
            anyhow::ensure!(validate_name(name), "no such timezone");

            let root = tzdir();
            let iter = walkdir::WalkDir::new(&root).into_iter().filter_entry(|entry| {
                let Ok(suffix) = entry.path().strip_prefix(&root) else { return false };
                let suffix = suffix.as_os_str().as_encoded_bytes();
                let name = name.as_bytes();

                name.len() >= suffix.len() && suffix.eq_ignore_ascii_case(&name[..suffix.len()])
            });

            for entry in iter {
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_file() {
                    continue;
                }

                let Ok(suffix) = entry.path().strip_prefix(&root) else { continue };
                if suffix.as_os_str().eq_ignore_ascii_case(name) {
                    return from_path(name, entry.path());
                }
            }

            anyhow::bail!("no such timezone")
        }

        fn utc() -> Self {
            Tz::new(chrono::Utc.into())
        }
    }
}

#[cfg(not(unix))]
pub type Tz = chrono_tz::Tz;

#[cfg(not(unix))]
impl LoadTimeZone for Tz {
    fn from_name(name: &str) -> Result<Self, Error> {
        Ok(Self(name.parse().map_err(Error::msg)?))
    }

    fn from_name_case_insensitive(name: &str) -> Result<Self, Error> {
        Ok(Self(chrono_tz::Tz::from_str_insensitive(name).map_err(Error::msg)?))
    }

    fn utc() -> Self {
        chrono_tz::UTC
    }
}
