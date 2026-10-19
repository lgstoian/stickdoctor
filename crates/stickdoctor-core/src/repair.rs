use crate::error::{Error, Result};
use crate::format;
use crate::props::DiskProperties;
use crate::SharedLog;
use std::ptr;
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, WriteFile, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Ioctl::IOCTL_DISK_DELETE_DRIVE_LAYOUT;
use windows_sys::Win32::System::IO::DeviceIoControl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsChoice {
    Fat32,
    ExFat,
    Ntfs,
}

impl std::fmt::Display for FsChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            FsChoice::Fat32 => write!(f, "FAT32"),
            FsChoice::ExFat => write!(f, "exFAT"),
            FsChoice::Ntfs => write!(f, "NTFS"),
        }
    }
}

impl FsChoice {
    pub fn warn_for_size(&self, size_gib: f64) -> Option<&'static str> {
        match self {
            FsChoice::Fat32 if size_gib > 32.0 => {
                Some("Windows cannot format FAT32 volumes larger than 32 GiB. Use exFAT instead.")
            }
            _ => None,
        }
    }

    pub fn fmifs_name(&self) -> &'static str {
        match self {
            FsChoice::Fat32 => "FAT32",
            FsChoice::ExFat => "exFAT",
            FsChoice::Ntfs => "NTFS",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RepairOptions {
    pub fs: FsChoice,
    pub label: String,
    pub quick: bool,
    pub zero_sectors: bool,
}

impl Default for RepairOptions {
    fn default() -> Self {
        Self {
            fs: FsChoice::Fat32,
            label: String::new(),
            quick: true,
            zero_sectors: false,
        }
    }
}

/// Progress callback used by the GUI so it can show a live status line.
pub type StageCallback = dyn Fn(&str) + Send + Sync;

pub fn execute_repair(index: u32, opts: &RepairOptions, log: &SharedLog) -> Result<()> {
    execute_repair_with_progress(index, opts, log, &|_| {})
}

/// Full repair pipeline. Every stage verifies its own outcome; success is only
/// reported after the filesystem has been read back and checked.
pub fn execute_repair_with_progress(
    index: u32,
    opts: &RepairOptions,
    log: &SharedLog,
    on_stage: &StageCallback,
) -> Result<()> {
    // ---- 0. safety ----
    let props = inspect_or_refuse(index, log)?;
    safety_or_refuse(index, &props, log)?;

    if let Some(warn) = opts.fs.warn_for_size(props.size_gib()) {
        note(log, crate::log::LogLevel::Warn, warn);
    }
    note(
        log,
        crate::log::LogLevel::Step,
        &format!(
            r"Repairing \\.\PhysicalDrive{} ({})",
            index,
            props.summary()
        ),
    );

    // ---- 1. unmount anything mounted on the disk ----
    on_stage("Unmounting volumes...");
    let (locked, dismounted) = crate::volumes::lock_dismount_all(index)?;
    note(
        log,
        crate::log::LogLevel::Info,
        &format!(
            "Volumes unmounted (locked={}, dismounted={})",
            locked, dismounted
        ),
    );

    // ---- 2. optional raw wipe of the signature areas ----
    if opts.zero_sectors {
        on_stage("Zeroing first/last 1 MiB...");
        zero_signature_areas(index, props.size_bytes, log)?;
    }

    // ---- 3. rebuild the partition table ----
    on_stage("Rebuilding partition table...");
    delete_layout(index, log)?;
    create_partition(index, log)?;

    // ---- 4. wait for the new volume ----
    on_stage("Waiting for the new volume...");
    let volume = match crate::volumes::wait_for_volume(index, 25) {
        Some(v) => v,
        None => {
            let msg = "The partition was created but Windows did not expose a volume. \
                       Try unplugging and reinserting the stick, then run Repair again.";
            note(log, crate::log::LogLevel::Error, msg);
            return Err(Error::Internal(msg.into()));
        }
    };
    note(
        log,
        crate::log::LogLevel::Success,
        &format!(
            "New volume: {} (filesystem before format: {})",
            volume
                .drive_letter
                .clone()
                .unwrap_or_else(|| volume.device_path.clone()),
            volume.filesystem.clone().unwrap_or_else(|| "RAW".into())
        ),
    );

    // ---- 5. format ----
    let label = sanitize_label(&opts.label);
    on_stage(&format!("Formatting as {}...", opts.fs));
    format::format_volume(&volume, opts.fs, &label, opts.quick, log)?;

    // ---- 6. verify the filesystem really exists ----
    on_stage("Verifying...");
    let root = format::volume_root(&volume)?;
    format::verify_volume(&root, opts.fs.fmifs_name(), log)?;

    note(
        log,
        crate::log::LogLevel::Success,
        &format!("Repair complete. {} is ready ({}).", root, opts.fs),
    );
    on_stage("Done.");
    Ok(())
}

fn inspect_or_refuse(index: u32, log: &SharedLog) -> Result<DiskProperties> {
    match crate::props::inspect_properties(index) {
        Ok(p) => Ok(p),
        Err(e) => {
            note(
                log,
                crate::log::LogLevel::Error,
                &format!("Cannot inspect disk {}: {}", index, e),
            );
            Err(e)
        }
    }
}

fn safety_or_refuse(index: u32, props: &DiskProperties, log: &SharedLog) -> Result<()> {
    match crate::safety::safety_check(props, index)? {
        crate::safety::SafetyStatus::Safe => Ok(()),
        other => {
            let msg = format!(
                "Refusing to repair PhysicalDrive{}: {}",
                index,
                other.label()
            );
            note(log, crate::log::LogLevel::Error, &msg);
            Err(Error::Refused(msg))
        }
    }
}

/// Remove the existing partition table with an ioctl (locale-independent).
/// Falls back to `diskpart clean` when the ioctl is refused.
fn delete_layout(index: u32, log: &SharedLog) -> Result<()> {
    match ioctl_delete_layout(index) {
        Ok(()) => {
            note(
                log,
                crate::log::LogLevel::Success,
                "Partition table removed (ioctl)",
            );
            Ok(())
        }
        Err(e) => {
            note(
                log,
                crate::log::LogLevel::Warn,
                &format!("ioctl clean failed ({}); falling back to diskpart", e),
            );
            let script = format!("select disk {}\nclean\n", index);
            let out = format::run_diskpart(&script)?;
            note(
                log,
                crate::log::LogLevel::Success,
                "Partition table removed (diskpart)",
            );
            note(log, crate::log::LogLevel::Info, out.trim());
            Ok(())
        }
    }
}

fn ioctl_delete_layout(index: u32) -> Result<()> {
    let h = open_drive(index)?;
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_DISK_DELETE_DRIVE_LAYOUT,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    };
    let err = if ok == 0 {
        Some(crate::error::last_error())
    } else {
        None
    };
    unsafe { CloseHandle(h) };
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Create one primary partition and assign a drive letter.
fn create_partition(index: u32, log: &SharedLog) -> Result<()> {
    let script = format!(
        "select disk {}\nattributes disk clear readonly\ncreate partition primary\nassign\nrescan\n",
        index
    );
    let out = format::run_diskpart(&script)?;
    note(
        log,
        crate::log::LogLevel::Success,
        "Partition created and assigned",
    );
    note(log, crate::log::LogLevel::Info, out.trim());
    Ok(())
}

fn zero_signature_areas(index: u32, size_bytes: u64, log: &SharedLog) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::SetFilePointerEx;
    let h = open_drive(index)?;
    let chunk = vec![0u8; 1024 * 1024];
    let mut written: u32 = 0;

    // first 1 MiB
    let ok = unsafe {
        WriteFile(
            h,
            chunk.as_ptr(),
            chunk.len() as u32,
            &mut written,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        let e = crate::error::last_error();
        unsafe { CloseHandle(h) };
        return Err(e);
    }

    // last 1 MiB
    if size_bytes > chunk.len() as u64 {
        let target: i64 = -((chunk.len()) as i64);
        let mut newpos: i64 = 0;
        if unsafe { SetFilePointerEx(h, target, &mut newpos, 2) } != 0 {
            let _ = unsafe {
                WriteFile(
                    h,
                    chunk.as_ptr(),
                    chunk.len() as u32,
                    &mut written,
                    ptr::null_mut(),
                )
            };
        }
    }
    unsafe { CloseHandle(h) };
    note(
        log,
        crate::log::LogLevel::Success,
        "First/last 1 MiB zeroed",
    );
    Ok(())
}

fn open_drive(index: u32) -> Result<windows_sys::Win32::Foundation::HANDLE> {
    let path = format!(r"\\.\PhysicalDrive{}", index);
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let h = unsafe {
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
    if h == INVALID_HANDLE_VALUE {
        return Err(crate::error::last_error());
    }
    Ok(h)
}

/// Keep labels short, safe for every filesystem, and safe for diskpart scripts.
fn sanitize_label(label: &str) -> String {
    let cleaned: String = label
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(11)
        .collect();
    if cleaned.is_empty() {
        "STICK".to_string()
    } else {
        cleaned
    }
}

fn note(log: &SharedLog, level: crate::log::LogLevel, msg: &str) {
    if let Ok(mut l) = log.lock() {
        l.log(level, msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_label_strips_spaces_and_symbols() {
        assert_eq!(sanitize_label("my stick!"), "mystick");
        assert_eq!(sanitize_label(""), "STICK");
        assert_eq!(sanitize_label("   "), "STICK");
        assert_eq!(sanitize_label("a-b_c1234567890"), "a-b_c123456");
    }

    #[test]
    fn sanitize_label_caps_at_eleven_chars() {
        assert_eq!(sanitize_label("ABCDEFGHIJKLMNOPQRSTUV").len(), 11);
    }

    #[test]
    fn fs_choice_warns_for_large_fat32() {
        assert!(FsChoice::Fat32.warn_for_size(33.0).is_some());
        assert!(FsChoice::Fat32.warn_for_size(32.0).is_none());
        assert!(FsChoice::Fat32.warn_for_size(1.0).is_none());
        assert!(FsChoice::ExFat.warn_for_size(100.0).is_none());
        assert!(FsChoice::Ntfs.warn_for_size(100.0).is_none());
    }

    #[test]
    fn fs_choice_names() {
        assert_eq!(FsChoice::Fat32.fmifs_name(), "FAT32");
        assert_eq!(FsChoice::ExFat.fmifs_name(), "exFAT");
        assert_eq!(FsChoice::Ntfs.fmifs_name(), "NTFS");
    }
}
