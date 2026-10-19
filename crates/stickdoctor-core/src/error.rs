use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Win32(u32, String),
    Io(std::io::Error),
    InvalidDevice(String),
    Refused(String),
    Internal(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Win32(code, msg) => write!(f, "Win32 error {code}: {msg}"),
            Error::Io(e) => write!(f, "IO error: {e}"),
            Error::InvalidDevice(msg) => write!(f, "Invalid device: {msg}"),
            Error::Refused(msg) => write!(f, "Refused: {msg}"),
            Error::Internal(msg) => write!(f, "Internal error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Format a Win32 error code. Uses `std::io::Error::from_raw_os_error` which
/// internally calls FormatMessageW on Windows, avoiding a direct binding to an
/// API that isn't exposed by windows-sys 0.59.
pub fn win32_error_message(code: u32) -> String {
    use windows_sys::Win32::Foundation::GetLastError;
    if code == 0 {
        let code = unsafe { GetLastError() };
        if code == 0 {
            return "no error".to_string();
        }
        return win32_error_message(code);
    }
    let io = std::io::Error::from_raw_os_error(code as i32);
    let msg = io.to_string();
    if msg.is_empty() || msg.contains("The operation completed successfully") {
        format!("error code {code} (0x{code:08X})")
    } else {
        msg
    }
}

pub fn last_error() -> Error {
    use windows_sys::Win32::Foundation::GetLastError;
    let code = unsafe { GetLastError() };
    Error::Win32(code, win32_error_message(code))
}
