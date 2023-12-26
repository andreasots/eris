use anyhow::Error;
use chrono::{LocalResult, NaiveDate, NaiveDateTime, TimeZone};

#[derive(Clone, Debug)]
#[repr(transparent)]
pub struct Tz(#[cfg(unix)] tzfile::Tz, #[cfg(not(unix))] chrono_tz::Tz);

impl Tz {
    pub fn from_name(name: &str) -> Result<Self, Error> {
        #[cfg(unix)]
        {
            anyhow::ensure!(Self::validate_name(name), "no such timezone");

            Self::from_path(name, &Self::tzdir().join(name))
        }

        #[cfg(not(unix))]
        {
            Ok(Self(name.parse().map_err(Error::msg)?))
        }
    }

    pub fn from_name_case_insensitive(name: &str) -> Result<Self, Error> {
        #[cfg(unix)]
        {
            anyhow::ensure!(Self::validate_name(name), "no such timezone");

            let root = Self::tzdir();
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
                    return Self::from_path(name, entry.path());
                }
            }

            anyhow::bail!("no such timezone")
        }

        #[cfg(not(unix))]
        {
            Ok(Self(chrono_tz::Tz::from_str_insensitive(name).map_err(Error::msg)?))
        }
    }

    pub fn utc() -> Self {
        #[cfg(unix)]
        {
            Self(chrono::Utc.into())
        }

        #[cfg(not(unix))]
        {
            Self(chrono_tz::UTC)
        }
    }

    #[cfg(unix)]
    fn from_path(name: &str, path: &std::path::Path) -> Result<Self, Error> {
        use std::io::ErrorKind;

        use anyhow::Context;

        let source = match std::fs::read(path) {
            Err(err) if err.kind() == ErrorKind::NotFound => anyhow::bail!("no such timezone"),
            res => res.context("failed to read the zone file")?,
        };

        match tzfile::Tz::parse(name, &source) {
            Err(tzfile::Error::InvalidMagic) => anyhow::bail!("no such timezone"),
            res => Ok(Self(res.context("invalid zone file")?)),
        }
    }

    #[cfg(unix)]
    fn tzdir() -> std::path::PathBuf {
        match std::env::var_os("TZDIR") {
            Some(dir) => std::path::PathBuf::from(dir),
            None => std::path::PathBuf::from("/usr/share/zoneinfo"),
        }
    }

    #[cfg(unix)]
    fn validate_name(name: &str) -> bool {
        use std::sync::OnceLock;

        use regex::Regex;

        static REGEX: OnceLock<Regex> = OnceLock::new();
        let regex = REGEX.get_or_init(|| {
            Regex::new(r"^[a-zA-Z0-9_+-]{1,14}(/[a-zA-Z0-9_+-]{1,14}){0,2}$").unwrap()
        });

        regex.is_match(name)
            && !name.starts_with("right/")
            && !name.starts_with("posix/")
            && name != "posixrules"
    }
}

#[cfg(unix)]
impl<'a> TimeZone for &'a Tz {
    type Offset = tzfile::Offset<&'a tzfile::Tz>;

    fn from_offset(offset: &Self::Offset) -> Self {
        // SAFETY: `Self` is a #[repr(transparent)] wrapper of `tzfile::Tz`.
        unsafe { std::mem::transmute(<&'a tzfile::Tz>::from_offset(offset)) }
    }

    fn offset_from_local_date(&self, local: &NaiveDate) -> LocalResult<Self::Offset> {
        (&self.0).offset_from_local_date(local)
    }

    fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> LocalResult<Self::Offset> {
        (&self.0).offset_from_local_datetime(local)
    }

    fn offset_from_utc_date(&self, utc: &NaiveDate) -> Self::Offset {
        (&self.0).offset_from_utc_date(utc)
    }

    fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> Self::Offset {
        (&self.0).offset_from_utc_datetime(utc)
    }
}

#[cfg(not(unix))]
#[derive(Clone, Debug)]
pub struct Offset<'a> {
    offset: <chrono_tz::Tz as TimeZone>::Offset,
    tz: &'a Tz,
}

#[cfg(not(unix))]
impl chrono::offset::Offset for Offset<'_> {
    fn fix(&self) -> chrono::FixedOffset {
        chrono::offset::Offset::fix(&self.offset)
    }
}

#[cfg(not(unix))]
impl std::fmt::Display for Offset<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.offset.fmt(f)
    }
}

#[cfg(not(unix))]
impl<'a> TimeZone for &'a Tz {
    type Offset = Offset<'a>;

    fn from_offset(offset: &Self::Offset) -> Self {
        offset.tz
    }

    fn offset_from_local_date(&self, local: &NaiveDate) -> LocalResult<Self::Offset> {
        self.0.offset_from_local_date(local).map(|offset| Self::Offset { offset, tz: self })
    }

    fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> LocalResult<Self::Offset> {
        self.0.offset_from_local_datetime(local).map(|offset| Self::Offset { offset, tz: self })
    }

    fn offset_from_utc_date(&self, utc: &NaiveDate) -> Self::Offset {
        Self::Offset { offset: self.0.offset_from_utc_date(utc), tz: self }
    }

    fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> Self::Offset {
        Self::Offset { offset: self.0.offset_from_utc_datetime(utc), tz: self }
    }
}
