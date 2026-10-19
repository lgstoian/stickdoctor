use crate::error::{Error, Result};
use crate::log::LogLevel;
use crate::repair::FsChoice;
use crate::volumes::VolumeInfo;
use crate::win32_const::{FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, FSCTL_UNLOCK_VOLUME};
use crate::SharedLog;
use std::cell::RefCell;
use std::io::Write;
use std::mem;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::atomic::{AtomicU8, Ordering};
use windows_sys::Win32::Foundation::{
    CloseHandle, FreeLibrary, GENERIC_READ, GENERIC_WRITE, HMODULE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

/// Absolute path to a system tool, so the Windows search order cannot be
/// hijacked by a binary planted in the current directory or on PATH.
fn system32(name: &str) -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    PathBuf::from(root).join("System32").join(name)
}

/// Media type passed to fmifs FormatEx. USB sticks report removable media.
const FMIFS_REMOVABLE_MEDIA: u32 = 11;

/// Live percentage reported by fmifs while formatting (0..=100).
static FORMAT_PROGRESS: AtomicU8 = AtomicU8::new(0);

pub fn format_progress() -> u8 {
    FORMAT_PROGRESS.load(Ordering::Relaxed)
}

pub fn reset_format_progress() {
    FORMAT_PROGRESS.store(0, Ordering::Relaxed);
}

thread_local! {
    static FORMAT_LOG: RefCell<Option<SharedLog>> = const { RefCell::new(None) };
}

/// Format a volume. Tries fmifs (precise errors), then diskpart, then format.com.
pub fn format_volume(
    volume: &VolumeInfo,
    fs: FsChoice,
    label: &str,
    quick: bool,
    log: &SharedLog,
) -> Result<()> {
    let root = volume_root(volume)?;
    let fs_name = fs.fmifs_name();

    let mut errors: Vec<String> = Vec::new();

    // 1) fmifs FormatEx
    note(
        log,
        LogLevel::Step,
        &format!("Formatting {} as {} (fmifs)...", root, fs_name),
    );
    match format_with_fmifs(&root, fs, label, quick, log) {
        Ok(()) => {
            note(log, LogLevel::Success, "fmifs format succeeded");
            return Ok(());
        }
        Err(e) => {
            note(log, LogLevel::Warn, &format!("fmifs format failed: {}", e));
            errors.push(format!("fmifs: {}", e));
        }
    }

    // 2) diskpart on the specific volume
    note(log, LogLevel::Step, "Retrying format with diskpart...");
    match format_with_diskpart(volume, fs, label, quick) {
        Ok(()) => {
            note(log, LogLevel::Success, "diskpart format succeeded");
            return Ok(());
        }
        Err(e) => {
            note(
                log,
                LogLevel::Warn,
                &format!("diskpart format failed: {}", e),
            );
            errors.push(format!("diskpart: {}", e));
        }
    }

    // 3) format.com
    note(log, LogLevel::Step, "Retrying format with format.com...");
    match format_with_format_com(&root, fs, label, quick) {
        Ok(()) => {
            note(log, LogLevel::Success, "format.com succeeded");
            return Ok(());
        }
        Err(e) => {
            note(log, LogLevel::Warn, &format!("format.com failed: {}", e));
            errors.push(format!("format.com: {}", e));
        }
    }

    Err(Error::Internal(format!(
        "all formatting methods failed ({}). The stick may be write-protected or failing.",
        errors.join("; ")
    )))
}

/// Re-read the filesystem and fail unless it matches `expected`.
/// Windows can take a moment to remount a volume after formatting, so this
/// retries before declaring failure.
pub fn verify_volume(root: &str, expected: &str, log: &SharedLog) -> Result<()> {
    let mut last_error = String::new();
    for attempt in 0..10 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        match read_volume(root) {
            Ok((detected, label)) => {
                if detected.eq_ignore_ascii_case(expected) {
                    note(
                        log,
                        LogLevel::Success,
                        &format!("Verified {} as {} (label \"{}\")", root, detected, label),
                    );
                    return Ok(());
                }
                last_error = format!(
                    "format did not take effect: {} reports {} but {} was requested",
                    root,
                    if detected.is_empty() {
                        "RAW"
                    } else {
                        &detected
                    },
                    expected
                );
            }
            Err(e) => last_error = e.to_string(),
        }
    }
    Err(Error::Internal(last_error))
}

/// Write a small file, flush it, read it back and compare. This proves the
/// formatted filesystem is actually usable on the device, not just present in
/// the partition table.
pub fn verify_read_write(root: &str, log: &SharedLog) -> Result<()> {
    let file = std::path::Path::new(root).join("stickdoctor-test.tmp");
    let data: Vec<u8> = (0..64 * 1024usize).map(|i| (i % 251) as u8).collect();

    std::fs::write(&file, &data).map_err(|e| Error::Internal(format!("write test failed: {e}")))?;
    {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .map_err(|e| Error::Internal(format!("reopen for flush failed: {e}")))?;
        f.sync_all()
            .map_err(|e| Error::Internal(format!("flush failed: {e}")))?;
    }
    let read =
        std::fs::read(&file).map_err(|e| Error::Internal(format!("read-back failed: {e}")))?;
    let _ = std::fs::remove_file(&file);

    if read != data {
        return Err(Error::Internal(
            "read/write test mismatch: the formatted volume may be failing".into(),
        ));
    }
    note(log, LogLevel::Success, "Read/write test passed (64 KiB)");
    Ok(())
}
fn read_volume(root: &str) -> Result<(String, String)> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
    let mut fs_buf = vec![0u16; 64];
    let mut label_buf = vec![0u16; 64];
    let ok = unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            label_buf.as_mut_ptr(),
            label_buf.len() as u32,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            fs_buf.as_mut_ptr(),
            fs_buf.len() as u32,
        )
    };
    if ok == 0 {
        return Err(Error::Internal(format!(
            "cannot read filesystem on {} (volume may be RAW): {}",
            root,
            crate::error::last_error()
        )));
    }
    Ok((
        String::from_utf16_lossy(&fs_buf)
            .trim_end_matches('\0')
            .to_string(),
        String::from_utf16_lossy(&label_buf)
            .trim_end_matches('\0')
            .to_string(),
    ))
}

/// Drive root such as `E:\`, or the volume GUID path when there is no letter.
pub fn volume_root(volume: &VolumeInfo) -> Result<String> {
    if let Some(ref letter) = volume.drive_letter {
        return Ok(format!("{}\\", letter));
    }
    if volume.device_path.starts_with(r"\\?\Volume{") {
        let mut p = volume.device_path.clone();
        if !p.ends_with('\\') {
            p.push('\\');
        }
        return Ok(p);
    }
    Err(Error::Internal(
        "volume has neither a drive letter nor a volume path".into(),
    ))
}

// ---------------------------------------------------------------- fmifs

type FormatCallback = unsafe extern "system" fn(u32, u32, *mut std::ffi::c_void) -> i32;
type FormatExFn = unsafe extern "system" fn(
    *const u16,
    u32,
    *const u16,
    *const u16,
    i32,
    u32,
    FormatCallback,
) -> i32;

unsafe extern "system" fn format_callback(
    action: u32,
    _sub: u32,
    data: *mut std::ffi::c_void,
) -> i32 {
    // action 1 = OutputMsg (wide string), action 2 = PercentDone (data is a DWORD*)
    if action == 2 && !data.is_null() {
        let pct = unsafe { *(data as *const u32) } as u8;
        FORMAT_PROGRESS.store(pct.min(100), Ordering::Relaxed);
    }
    if action == 1 && !data.is_null() {
        let p = data as *const u16;
        let mut len = 0usize;
        while unsafe { *p.add(len) } != 0 && len < 512 {
            len += 1;
        }
        let msg = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) });
        let msg = msg.trim().to_string();
        if !msg.is_empty() {
            FORMAT_LOG.with(|l| {
                if let Some(log) = l.borrow().as_ref() {
                    if let Ok(mut g) = log.lock() {
                        g.info(format!("  fmifs: {}", msg));
                    }
                }
            });
        }
    }
    1 // continue
}

fn format_with_fmifs(
    root: &str,
    fs: FsChoice,
    label: &str,
    quick: bool,
    log: &SharedLog,
) -> Result<()> {
    // Load fmifs.dll from System32 only. LOAD_LIBRARY_SEARCH_SYSTEM32 stops a
    // planted DLL next to the executable from loading into this elevated
    // process.
    let dll_path = system32("fmifs.dll");
    let dll = encode_wide(&dll_path.to_string_lossy());
    let hmod: HMODULE = unsafe {
        LoadLibraryExW(
            dll.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if hmod.is_null() {
        return Err(Error::Internal(format!(
            "cannot load {}",
            dll_path.display()
        )));
    }

    let proc = unsafe { GetProcAddress(hmod, c"FormatEx".as_ptr() as *const u8) };
    let func: FormatExFn = match proc {
        Some(f) => unsafe { mem::transmute::<unsafe extern "system" fn() -> isize, FormatExFn>(f) },
        None => {
            unsafe { FreeLibrary(hmod) };
            return Err(Error::Internal("FormatEx not exported by fmifs.dll".into()));
        }
    };

    // Lock + dismount the volume while fmifs writes to it.
    // FSCTL_LOCK_VOLUME requires the volume device path (`\\.\E:`), not `E:\`.
    let lock_path = if root.len() >= 2 && root.as_bytes()[1] == b':' {
        format!(r"\\.\{}", &root[..2])
    } else {
        root.trim_end_matches('\\').to_string()
    };
    let wide: Vec<u16> = lock_path.encode_utf16().chain(std::iter::once(0)).collect();
    let vol = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    let mut have_lock = false;
    let mut returned: u32 = 0;
    if vol != INVALID_HANDLE_VALUE {
        for _ in 0..10 {
            let ok = unsafe {
                DeviceIoControl(
                    vol,
                    FSCTL_LOCK_VOLUME,
                    ptr::null_mut(),
                    0,
                    ptr::null_mut(),
                    0,
                    &mut returned,
                    ptr::null_mut(),
                )
            };
            if ok != 0 {
                have_lock = true;
                let _ = unsafe {
                    DeviceIoControl(
                        vol,
                        FSCTL_DISMOUNT_VOLUME,
                        ptr::null_mut(),
                        0,
                        ptr::null_mut(),
                        0,
                        &mut returned,
                        ptr::null_mut(),
                    )
                };
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    FORMAT_LOG.with(|l| *l.borrow_mut() = Some(log.clone()));

    let root_w = encode_wide(&format!("{}\\", root.trim_end_matches('\\')));
    let fs_w = encode_wide(fs.fmifs_name());
    let label_w = encode_wide(label);

    let ok = unsafe {
        func(
            root_w.as_ptr(),
            FMIFS_REMOVABLE_MEDIA,
            fs_w.as_ptr(),
            label_w.as_ptr(),
            if quick { 1 } else { 0 },
            0,
            format_callback,
        )
    };

    FORMAT_LOG.with(|l| *l.borrow_mut() = None);

    if have_lock {
        unsafe {
            DeviceIoControl(
                vol,
                FSCTL_UNLOCK_VOLUME,
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null_mut(),
            )
        };
    }
    if vol != INVALID_HANDLE_VALUE {
        unsafe { CloseHandle(vol) };
    }
    unsafe { FreeLibrary(hmod) };

    if ok == 0 {
        return Err(crate::error::last_error());
    }
    Ok(())
}

// ------------------------------------------------------------- diskpart

fn format_with_diskpart(volume: &VolumeInfo, fs: FsChoice, label: &str, quick: bool) -> Result<()> {
    let target = match volume.drive_letter.clone() {
        Some(l) => l,
        None => {
            return Err(Error::Internal(
                "no drive letter for diskpart format".into(),
            ))
        }
    };
    let quick = if quick { "quick" } else { "" };
    let script = format!(
        "select volume {}\nformat fs={} label={} {}\n",
        target,
        fs.fmifs_name(),
        label,
        quick
    );
    run_diskpart(&script).map(|_| ())
}

// ------------------------------------------------------------ format.com

fn format_with_format_com(root: &str, fs: FsChoice, label: &str, quick: bool) -> Result<()> {
    let drive = format!("{}:", root.chars().next().unwrap_or('E'));
    let fs_arg = match fs {
        FsChoice::Fat32 => "FAT32",
        FsChoice::ExFat => "exFAT",
        FsChoice::Ntfs => "NTFS",
    };
    let mut cmd = Command::new(system32("format.com"));
    cmd.arg(&drive).arg(format!("/FS:{}", fs_arg)).arg("/Y");
    if quick {
        cmd.arg("/Q");
    }
    if !label.is_empty() {
        cmd.arg(format!("/V:{}", label));
    }
    let out = cmd.output().map_err(Error::Io)?;
    if !out.status.success() {
        return Err(Error::Internal(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// Run a diskpart script supplied on stdin.
///
/// No temporary file is written: the script never touches the filesystem, which
/// removes the symlink/TOCTOU risk of a predictable temp path. diskpart.exe is
/// resolved from System32 so a planted binary cannot hijack the elevated
/// process.
pub fn run_diskpart(script: &str) -> Result<String> {
    let mut child = Command::new(system32("diskpart.exe"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(Error::Io)?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Internal("cannot open diskpart stdin".into()))?;
        stdin.write_all(script.as_bytes()).map_err(Error::Io)?;
        // Dropping `stdin` here closes the pipe and signals EOF.
    }

    let out = child.wait_with_output().map_err(Error::Io)?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let combined = format!("{}\n{}", stdout, stderr);

    // We do not rely on English error strings for the decision: the caller
    // verifies the result independently. We only surface a non-zero exit.
    if !out.status.success() {
        return Err(Error::Internal(format!(
            "diskpart exited with {}: {}",
            out.status,
            combined.trim()
        )));
    }
    Ok(combined)
}

/// Read the partition's boot sector and check the filesystem signature.
/// This does not require a mounted volume, so it works even when Windows
/// has not registered a volume for the partition yet.
pub fn verify_boot_sector(index: u32, expected_fs: &str, log: &SharedLog) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, SetFilePointerEx};
    let path = format!(r"\\.\PhysicalDrive{}", index);
    let path_w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let h = unsafe {
        CreateFileW(
            path_w.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(crate::error::last_error());
    }
    let mut pos: i64 = 0;
    let ok = unsafe { SetFilePointerEx(h, 2048 * 512, &mut pos, 0) };
    if ok == 0 {
        let e = crate::error::last_error();
        unsafe { CloseHandle(h) };
        return Err(e);
    }
    let mut buf = [0u8; 512];
    let mut read: u32 = 0;
    let ok = unsafe { ReadFile(h, buf.as_mut_ptr(), 512, &mut read, ptr::null_mut()) };
    unsafe { CloseHandle(h) };
    if ok == 0 || read < 512 {
        return Err(Error::Internal(
            "cannot read the partition boot sector".into(),
        ));
    }
    let sig_ok = match expected_fs {
        "FAT32" => &buf[82..90] == b"FAT32   ",
        "exFAT" => &buf[3..11] == b"EXFAT   ",
        "NTFS" => &buf[3..11] == b"NTFS    ",
        _ => true,
    };
    if !sig_ok {
        return Err(Error::Internal(format!(
            "boot sector does not contain a {} signature (format did not take effect)",
            expected_fs
        )));
    }
    note(
        log,
        LogLevel::Success,
        &format!("Boot sector confirms {} filesystem.", expected_fs),
    );
    Ok(())
}
fn note(log: &SharedLog, level: LogLevel, msg: &str) {
    if let Ok(mut l) = log.lock() {
        l.log(level, msg);
    }
}

fn encode_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
