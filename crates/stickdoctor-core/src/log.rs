use std::fmt;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
    Success,
    Step,
}

impl LogLevel {
    /// Short tag used in the rendered log.
    fn tag(self) -> &'static str {
        match self {
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "fail",
            LogLevel::Success => " ok ",
            LogLevel::Step => "step",
        }
    }

    /// Single character marker.
    fn mark(self) -> &'static str {
        match self {
            LogLevel::Info => "i",
            LogLevel::Warn => "!",
            LogLevel::Error => "x",
            LogLevel::Success => "+",
            LogLevel::Step => ">",
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.tag())
    }
}

#[derive(Debug, Clone)]
pub struct LogEvent {
    pub level: LogLevel,
    pub timestamp: SystemTime,
    pub message: String,
    pub detail: Option<String>,
}

impl LogEvent {
    fn render(&self, elapsed: Duration) -> String {
        let secs = elapsed.as_secs_f64();
        let marker = self.level.mark();
        let tag = self.level.tag();
        let detail = match &self.detail {
            Some(d) => format!("  |  {}", d),
            None => String::new(),
        };
        format!(
            "{:>8.3}s  [{}] {}  {}{}",
            secs, tag, marker, self.message, detail
        )
    }
}

#[derive(Debug, Default)]
pub struct Logger {
    events: Vec<LogEvent>,
    start: Option<SystemTime>,
}

impl Logger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn log(&mut self, level: LogLevel, message: impl Into<String>) {
        self.ensure_start();
        self.events.push(LogEvent {
            level,
            timestamp: SystemTime::now(),
            message: message.into(),
            detail: None,
        });
    }

    pub fn log_detail(
        &mut self,
        level: LogLevel,
        message: impl Into<String>,
        detail: impl Into<String>,
    ) {
        self.ensure_start();
        self.events.push(LogEvent {
            level,
            timestamp: SystemTime::now(),
            message: message.into(),
            detail: Some(detail.into()),
        });
    }

    pub fn info(&mut self, msg: impl Into<String>) {
        self.log(LogLevel::Info, msg);
    }
    pub fn warn(&mut self, msg: impl Into<String>) {
        self.log(LogLevel::Warn, msg);
    }
    pub fn error(&mut self, msg: impl Into<String>) {
        self.log(LogLevel::Error, msg);
    }
    pub fn success(&mut self, msg: impl Into<String>) {
        self.log(LogLevel::Success, msg);
    }
    pub fn step(&mut self, msg: impl Into<String>) {
        self.log(LogLevel::Step, msg);
    }

    pub fn events(&self) -> &[LogEvent] {
        &self.events
    }

    pub fn clear(&mut self) {
        self.events.clear();
        self.start = None;
    }

    fn ensure_start(&mut self) {
        if self.start.is_none() {
            self.start = Some(SystemTime::now());
        }
    }

    /// Elegant, human-readable rendering with elapsed time, level tag and an
    /// optional detail column.
    pub fn render(&self) -> String {
        let start = match self.start {
            Some(s) => s,
            None => return String::new(),
        };
        self.events
            .iter()
            .map(|e| {
                let elapsed = e.timestamp.duration_since(start).unwrap_or_default();
                e.render(elapsed)
            })
            .collect::<Vec<_>>()
            .join("\r\n")
    }

    /// A single-line summary (used by the status bar / quick views).
    pub fn summary(&self) -> String {
        match self.events.last() {
            Some(e) => e.message.clone(),
            None => String::new(),
        }
    }
}
