// Copied from slog_scope_stdlog: https://github.com/slog-rs/scope-stdlog
// Licensed under MPL-2.0/MIT/Apache-2.0

use log::{Level as LogLevel, Log, Metadata, Record as LogRecord};
use slog::{b, Level as SlogLevel, Record as SlogRecord, RecordLocation, RecordStatic};

pub static LOGGER: Logger = Logger;

pub struct Logger;

fn log_to_slog_level(level: LogLevel) -> SlogLevel {
    match level {
        LogLevel::Trace => SlogLevel::Trace,
        LogLevel::Debug => SlogLevel::Debug,
        LogLevel::Info => SlogLevel::Info,
        LogLevel::Warn => SlogLevel::Warning,
        LogLevel::Error => SlogLevel::Error,
    }
}

impl Log for Logger {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }

    fn log(&self, r: &LogRecord) {
        let level = log_to_slog_level(r.metadata().level());

        let location = RecordLocation {
            file: "<unknown>", // r.file() returns a `&'a str` which is not `'static`
            line: r.line().unwrap_or(0),
            column: 0,
            function: "<unknown>",
            module: "<unknown>", // r.module() returns a `&'a str` which is not `'static`
        };

        let tag = "";

        let s = RecordStatic { location: &location, tag, level };
        slog_scope::logger().log(&SlogRecord::new(&s, r.args(), b!()));
    }

    fn flush(&self) {}
}
