use std::fmt;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
    Success,
    Step,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Warn => write!(f, "WARN"),
            LogLevel::Error => write!(f, "ERROR"),
            LogLevel::Success => write!(f, "OK"),
            LogLevel::Step => write!(f, "STEP"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogEvent {
    pub level: LogLevel,
    pub timestamp: SystemTime,
    pub message: String,
    pub detail: Option<String>,
}

impl fmt::Display for LogEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            Some(d) => write!(f, "[{}] {} {}", self.level, self.message, d),
            None => write!(f, "[{}] {}", self.level, self.message),
        }
    }
}

#[derive(Debug, Default)]
pub struct Logger {
    events: Vec<LogEvent>,
}

impl Logger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn log(&mut self, level: LogLevel, message: impl Into<String>) {
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
    }

    pub fn render(&self) -> String {
        self.events
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
