//! The repair pipeline: safety -> unmount -> wipe -> delete layout ->
//! create + format -> wait -> verify -> report.
//!
//! Success is decided by reading the result back, never by a tool's exit code.
//!
//! @sd file module="crates/stickdoctor-core/src/repair.rs" role="repair orchestration and verification" entry="execute_repair_with_progress" exit="VerificationOutcome::is_success"
//! @sd anchor name=core.repair text="repair pipeline"
//! @sd ref name=core.safety
//! @sd ref name=core.format
//! @sd flow id=repair.safety step=1 pipeline=repair label="Safety gate" next=repair.unmount
//! @sd flow id=repair.unmount step=2 pipeline=repair label="Unmount volumes" next=repair.zero
//! @sd flow id=repair.zero step=3 pipeline=repair label="Optional raw wipe" next=repair.delete
//! @sd flow id=repair.delete step=4 pipeline=repair label="Delete layout" next=repair.create
//! @sd flow id=repair.create step=5 pipeline=repair label="Create partition and format" next=repair.wait
//! @sd flow id=repair.wait step=6 pipeline=repair label="Wait for volume" next=repair.verify
//! @sd flow id=repair.verify step=7 pipeline=repair label="Verify filesystem" next=repair.report verify=repair-success
//! @sd flow id=repair.report step=8 pipeline=repair label="Report"
//! @sd verify id=repair-success fn="is_success" proves="a repair succeeded only if the filesystem was read back and matched" negative_test=verification_rejects_boot_sector_failure
//! @sd verify id=partition-usable fn="layout_is_usable_mbr" proves="a created partition is a usable MBR partition" negative_test=layout_is_usable_mbr_rejects_gpt
//! @sd hazard text="performs irreversible writes to a raw disk" guard="safety_check, self-elevation and user confirmation"

use crate::error::{Error, Result};
use crate::format;
use crate::props::DiskProperties;
use crate::SharedLog;
use std::ptr;
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, WriteFile, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
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

    /// MBR partition-type byte that matches the filesystem, so an exFAT/NTFS
    /// repair does not leave a FAT32 type byte on the partition.
    pub fn partition_type_byte(&self) -> u8 {
        match self {
            FsChoice::Fat32 => 0x0C,                  // FAT32 LBA
            FsChoice::ExFat | FsChoice::Ntfs => 0x07, // NTFS / exFAT
        }
    }
}

#[derive(Debug, Clone)]
pub struct RepairOptions {
    pub fs: FsChoice,
    pub label: String,
    pub quick: bool,
    pub zero_sectors: bool,
    /// Only rebuild the partition table; skip formatting.
    pub rebuild_only: bool,
}

impl Default for RepairOptions {
    fn default() -> Self {
        Self {
            fs: FsChoice::Fat32,
            label: String::new(),
            quick: true,
            zero_sectors: false,
            rebuild_only: false,
        }
    }
}

/// The result of the independent verification steps that decide whether a
/// repair really succeeded.
///
/// `None` means "this check did not apply" (for example, no volume was
/// registered so the read/write test could not run). Every check that *did* run
/// must have passed, and at least one must have run: otherwise success would be
/// reported for a repair that was never verified (a false green).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct VerificationOutcome {
    pub boot_sector: Option<bool>,
    pub volume_fs: Option<bool>,
    pub read_write: Option<bool>,
}

impl VerificationOutcome {
    fn checks(&self) -> [Option<bool>; 3] {
        [self.boot_sector, self.volume_fs, self.read_write]
    }

    /// How many checks actually ran.
    pub fn ran(&self) -> usize {
        self.checks().iter().flatten().count()
    }

    /// True only when at least one check ran and every check passed.
    pub fn is_success(&self) -> bool {
        let checks = self.checks();
        self.ran() > 0 && checks.iter().flatten().all(|ok| *ok)
    }

    /// The first failed check, as a message for the log and the returned error.
    pub fn failure_reason(&self) -> String {
        if let Some(false) = self.boot_sector {
            return "the partition boot sector does not contain the requested filesystem signature"
                .to_string();
        }
        if let Some(false) = self.volume_fs {
            return "GetVolumeInformationW did not report the requested filesystem".to_string();
        }
        if let Some(false) = self.read_write {
            return "the post-format read/write test failed".to_string();
        }
        "no verification check was run".to_string()
    }
}

/// Progress callback used by the GUI so it can show a live status line.
pub type StageCallback = dyn Fn(&str, u8) + Send + Sync;

pub fn execute_repair(index: u32, opts: &RepairOptions, log: &SharedLog) -> Result<()> {
    execute_repair_with_progress(index, opts, log, &|_, _| {})
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
    // Rich disk summary — invaluable when diagnosing a stubborn stick.
    note(
        log,
        crate::log::LogLevel::Info,
        &format!(
            "Disk summary: bus={} media={} size={:.2} GiB ({:.0} bytes) removable={} writable={}",
            props.bus_type,
            props.media_type(),
            props.size_gib(),
            props.size_bytes,
            props.removable,
            props.writable,
        ),
    );
    if let Some(ref m) = props.model {
        note(log, crate::log::LogLevel::Info, &format!("Model: {}", m));
    }
    if let Some(ref s) = props.serial {
        note(log, crate::log::LogLevel::Info, &format!("Serial: {}", s));
    }
    if let Some(n) = props.device_number {
        note(
            log,
            crate::log::LogLevel::Info,
            &format!("Storage device number: {}", n),
        );
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
    on_stage("Unmounting volumes...", 10);
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
        on_stage("Zeroing first/last 1 MiB...", 18);
        if let Err(e) = zero_signature_areas(index, props.size_bytes, log) {
            note(
                log,
                crate::log::LogLevel::Warn,
                &format!(
                    "Skipping the optional zero pass: {}. Continuing with the rebuild.",
                    e
                ),
            );
        }
    }

    // ---- 3. rebuild the partition table ----
    on_stage("Removing old partition table...", 26);
    delete_layout(index, log)?;
    on_stage("Creating partition...", 34);
    let diskpart_formatted = create_partition(index, props.size_bytes, opts, log)?;

    // ---- 4. wait for the new volume ----
    let pre_volumes: std::collections::HashSet<String> =
        crate::volumes::all_volume_paths().into_iter().collect();
    let pre_letters = crate::volumes::drive_letters();
    on_stage("Waiting for the new volume...", 42);
    let mut volume: Option<crate::volumes::VolumeInfo> = None;
    for attempt in 0..40 {
        let vols = crate::volumes::disk_volumes(index).unwrap_or_default();
        let all_paths = crate::volumes::all_volume_paths();
        note(
            log,
            crate::log::LogLevel::Info,
            &format!(
                "wait attempt {}: {} volume(s) on disk, {} total volumes: [{}]",
                attempt + 1,
                vols.len(),
                all_paths.len(),
                all_paths.join(" | ")
            ),
        );
        if let Some(v) = vols.into_iter().next() {
            volume = Some(v);
            break;
        }
        // GUID-diff fallback: a volume that did not exist before repair.
        let now_paths = crate::volumes::all_volume_paths();
        if let Some(path) = now_paths.iter().find(|p| !pre_volumes.contains(*p)) {
            note(
                log,
                crate::log::LogLevel::Warn,
                &format!(
                    "wait attempt {}: new volume detected: {}",
                    attempt + 1,
                    path
                ),
            );
            let letter = crate::volumes::first_drive_letter(path);
            let fs = crate::volumes::probe_filesystem(path);
            volume = Some(crate::volumes::VolumeInfo {
                device_path: path.clone(),
                drive_letter: letter,
                filesystem: fs,
            });
            break;
        }
        // Diff-based fallback: a drive letter that did not exist before.
        let now = crate::volumes::drive_letters();
        if let Some(letter) = now.iter().find(|l| !pre_letters.contains(l)) {
            note(
                log,
                crate::log::LogLevel::Warn,
                &format!(
                    "wait attempt {}: new drive letter {} detected",
                    attempt + 1,
                    letter
                ),
            );
            let root = format!("{}\\", letter);
            let fs = crate::volumes::probe_filesystem(&root);
            volume = Some(crate::volumes::VolumeInfo {
                device_path: format!(r"\\.\{}", letter),
                drive_letter: Some(letter.clone()),
                filesystem: fs,
            });
            break;
        }
        if attempt % 5 == 4 {
            let _ = format::run_diskpart(&format!("select disk {}\nrescan\n", index));
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    let volume = match volume {
        Some(v) => Some(v),
        None => {
            note(
                log,
                crate::log::LogLevel::Warn,
                "Windows has not registered a drive letter for the new volume yet. \
                 The partition is formatted; unplug and replug the stick to mount it, \
                 or format it in Disk Management.",
            );
            None
        }
    };

    // If Windows created the volume without a letter, assign one ourselves.
    let volume = if let Some(ref v) = volume {
        if v.drive_letter.is_none() {
            on_stage("Assigning a drive letter...", 46);
            if let Err(e) = assign_free_letter(index, log) {
                note(
                    log,
                    crate::log::LogLevel::Warn,
                    &format!("Could not assign a drive letter: {}", e),
                );
            }
            if let Some(v2) = crate::volumes::wait_for_volume(index, 10) {
                Some(v2)
            } else {
                Some(v.clone())
            }
        } else {
            Some(v.clone())
        }
    } else {
        None
    };

    if let Some(ref v) = volume {
        note(
            log,
            crate::log::LogLevel::Success,
            &format!(
                "New volume: {} (filesystem before format: {})",
                v.drive_letter.as_deref().unwrap_or("?"),
                v.filesystem.as_deref().unwrap_or("RAW")
            ),
        );
    }

    if opts.rebuild_only {
        note(
            log,
            crate::log::LogLevel::Success,
            "Partition table rebuilt. Formatting skipped (rebuild-only mode).",
        );
        on_stage("Done.", 100);
        return Ok(());
    }

    let label = sanitize_label(&opts.label);

    // ---- 5. format ----
    if !diskpart_formatted {
        if let Some(ref v) = volume {
            on_stage(&format!("Formatting as {}...", opts.fs), 52);
            format::reset_format_progress();
            format::format_volume(v, opts.fs, &label, opts.quick, log)?;
        } else {
            // No volume yet (common on USB bridges): format the partition
            // directly so the repair still produces a filesystem.
            on_stage(&format!("Formatting partition as {}...", opts.fs), 52);
            let script = format!(
                "select disk {}\nselect partition 1\nformat fs={} label={} quick\n",
                index,
                opts.fs.fmifs_name(),
                label
            );
            if let Err(e) = format::run_diskpart(&script) {
                note(
                    log,
                    crate::log::LogLevel::Warn,
                    &format!("partition format failed: {}", e),
                );
            }
        }
    }

    // ---- 6. verify: every applicable check must pass ----
    on_stage("Verifying...", 88);
    let mut checks = VerificationOutcome::default();

    // The boot sector is the authoritative check: it needs no mounted volume.
    match format::verify_boot_sector(index, opts.fs.fmifs_name(), log) {
        Ok(()) => checks.boot_sector = Some(true),
        Err(e) => {
            checks.boot_sector = Some(false);
            note(
                log,
                crate::log::LogLevel::Warn,
                &format!("boot sector check failed: {}", e),
            );
        }
    }

    let root = volume.as_ref().and_then(|v| format::volume_root(v).ok());
    if let Some(ref r) = root {
        match format::verify_volume(r, opts.fs.fmifs_name(), log) {
            Ok(()) => checks.volume_fs = Some(true),
            Err(e) => {
                checks.volume_fs = Some(false);
                note(
                    log,
                    crate::log::LogLevel::Warn,
                    &format!("volume check failed: {}", e),
                );
            }
        }
        on_stage("Testing read/write...", 94);
        match format::verify_read_write(r, log) {
            Ok(()) => checks.read_write = Some(true),
            Err(e) => {
                checks.read_write = Some(false);
                note(
                    log,
                    crate::log::LogLevel::Warn,
                    &format!("read/write test failed: {}", e),
                );
            }
        }
    }

    if !checks.is_success() {
        let msg = format!("Repair failed verification: {}", checks.failure_reason());
        note(log, crate::log::LogLevel::Error, &msg);
        on_stage("Verification failed.", 96);
        return Err(Error::Internal(msg));
    }

    note(
        log,
        crate::log::LogLevel::Success,
        &format!(
            "Repair complete. {} is ready ({}).",
            root.as_deref().unwrap_or("the stick"),
            opts.fs
        ),
    );
    on_stage("Done.", 100);
    Ok(())
}

/// Assign the first free drive letter (E..Z) to partition 1 of the disk.
fn assign_free_letter(index: u32, log: &SharedLog) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;
    let mask = unsafe { GetLogicalDrives() };
    let mut letter = None;
    for i in 4..26u32 {
        if mask & (1 << i) == 0 {
            letter = Some((b'A' + i as u8) as char);
            break;
        }
    }
    let letter = match letter {
        Some(l) => l,
        None => return Err(Error::Internal("no free drive letter available".into())),
    };
    let script = format!(
        "select disk {}\nselect partition 1\nassign letter={}\n",
        index, letter
    );
    let out = format::run_diskpart(&script)?;
    note(
        log,
        crate::log::LogLevel::Info,
        &format!("Assigned drive letter {}: {}", letter, out.trim()),
    );
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

/// Invalidate the existing layout with an ioctl. This is only a best-effort
/// pre-step: `DELETE_DRIVE_LAYOUT` does not update DiskPart/VDS's own view of
/// the disk, so the authoritative reset is `diskpart clean` in `create_partition`.
fn delete_layout(index: u32, log: &SharedLog) -> Result<()> {
    match ioctl_delete_layout(index) {
        Ok(()) => note(
            log,
            crate::log::LogLevel::Success,
            "Partition table invalidated (ioctl)",
        ),
        Err(e) => note(
            log,
            crate::log::LogLevel::Warn,
            &format!(
                "ioctl invalidation failed ({}); DiskPart clean will handle it",
                e
            ),
        ),
    }
    Ok(())
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

/// Build the diskpart script that resets and rebuilds the partition table.
///
/// `rebuild_only` skips the `format` command entirely; otherwise `quick`
/// selects a quick format. Pure so the exact script can be asserted in tests.
fn create_partition_script(index: u32, opts: &RepairOptions) -> String {
    let label = sanitize_label(&opts.label);
    let head = format!(
        "select disk {index}\nonline disk\nattributes disk clear readonly\nclean\ncreate partition primary\n"
    );
    if opts.rebuild_only {
        format!("{head}assign\nrescan\n")
    } else if opts.quick {
        format!(
            "{head}format fs={} label={label} quick\nassign\nrescan\n",
            opts.fs.fmifs_name()
        )
    } else {
        format!(
            "{head}format fs={} label={label}\nassign\nrescan\n",
            opts.fs.fmifs_name()
        )
    }
}

/// Create one primary partition and assign a drive letter.
///
/// `diskpart clean` must run in the same DiskPart session as
/// `create partition primary`: the ioctl used by `delete_layout` leaves VDS with
/// a stale layout, which makes `create partition primary` fail with
/// "No usable free extent could be found".
///
/// DiskPart exits 0 even when a command fails, so the result is verified by
/// asking the disk for its actual partition layout. If that fails, two
/// progressively lower-level fallbacks are tried.
fn create_partition(
    index: u32,
    disk_size: u64,
    opts: &RepairOptions,
    log: &SharedLog,
) -> Result<bool> {
    let script = create_partition_script(index, opts);
    let formatted = match format::run_diskpart(&script) {
        Ok(out) => {
            let formatted = out.to_lowercase().contains("successfully formatted");
            note(log, crate::log::LogLevel::Info, out.trim());
            if formatted {
                note(
                    log,
                    crate::log::LogLevel::Success,
                    "DiskPart formatted the partition.",
                );
            } else {
                note(
                    log,
                    crate::log::LogLevel::Warn,
                    "DiskPart did not report a successful format.",
                );
            }
            formatted
        }
        Err(e) => {
            note(
                log,
                crate::log::LogLevel::Warn,
                &format!("DiskPart reported an error: {}. Trying fallbacks.", e),
            );
            false
        }
    };

    // Force the mount manager to re-read the partition table. DiskPart's
    // `rescan` is not always enough for a freshly created partition.
    if let Ok(h) = open_drive(index) {
        let mut returned = 0u32;
        let _ = unsafe {
            DeviceIoControl(
                h,
                windows_sys::Win32::System::Ioctl::IOCTL_DISK_UPDATE_PROPERTIES,
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null_mut(),
            )
        };
        unsafe { CloseHandle(h) };
    }

    if partition_exists(index) {
        note(
            log,
            crate::log::LogLevel::Success,
            "Partition created and assigned (DiskPart)",
        );
        return Ok(formatted);
    }

    note(
        log,
        crate::log::LogLevel::Warn,
        "DiskPart did not create a partition; trying the ioctl layout writer.",
    );
    match create_partition_ioctl(index, disk_size, opts.fs.partition_type_byte(), log) {
        Ok(()) if partition_exists(index) => {
            note(
                log,
                crate::log::LogLevel::Success,
                "Partition created (ioctl layout writer)",
            );
            let _ = assign_free_letter(index, log);
            return Ok(false);
        }
        Ok(()) => note(
            log,
            crate::log::LogLevel::Warn,
            "ioctl layout writer produced no visible partition",
        ),
        Err(e) => note(
            log,
            crate::log::LogLevel::Warn,
            &format!("ioctl layout writer failed: {}", e),
        ),
    }

    note(
        log,
        crate::log::LogLevel::Warn,
        "Falling back to a raw MBR write.",
    );
    write_raw_mbr(index, disk_size, opts.fs.partition_type_byte(), log)?;
    let _ = format::run_diskpart(&format!("select disk {}\nrescan\n", index));
    if !partition_exists(index) {
        return Err(Error::Internal(
            "could not create a partition table (DiskPart, ioctl and raw MBR all failed). "
                .to_string()
                + "The disk is probably still in use: close Explorer, Disk Management and any "
                + "format dialogs, or unplug and replug the stick, then try again.",
        ));
    }
    let _ = assign_free_letter(index, log);
    note(
        log,
        crate::log::LogLevel::Success,
        "Partition created (raw MBR)",
    );
    Ok(false)
}

/// Decide whether a decoded drive layout is a usable single-partition MBR.
///
/// Pure so it can be tested. `partition_exists` used to accept any entry with a
/// non-zero length, including GPT entries, so the partition-creation fallback
/// could look successful when it was not.
pub(crate) fn layout_is_usable_mbr(
    partition_style: u32,
    partition_count: u32,
    first_offset: i64,
    first_length: i64,
) -> bool {
    const STYLE_MBR: u32 = 0;
    partition_style == STYLE_MBR && partition_count > 0 && first_offset > 0 && first_length > 0
}

/// Read the real partition layout from the disk.
fn partition_exists(index: u32) -> bool {
    use windows_sys::Win32::System::Ioctl::{
        DRIVE_LAYOUT_INFORMATION_EX, IOCTL_DISK_GET_DRIVE_LAYOUT_EX,
    };
    let h = match open_drive(index) {
        Ok(h) => h,
        Err(_) => return false,
    };
    let mut buf = vec![0u8; 4096];
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_DISK_GET_DRIVE_LAYOUT_EX,
            ptr::null_mut(),
            0,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            buf.len() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    unsafe { CloseHandle(h) };
    if ok == 0 || (returned as usize) < std::mem::size_of::<DRIVE_LAYOUT_INFORMATION_EX>() {
        return false;
    }
    let layout =
        unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const DRIVE_LAYOUT_INFORMATION_EX) };
    let (offset, length) = if layout.PartitionCount > 0 {
        (
            layout.PartitionEntry[0].StartingOffset,
            layout.PartitionEntry[0].PartitionLength,
        )
    } else {
        (0, 0)
    };
    layout_is_usable_mbr(layout.PartitionStyle, layout.PartitionCount, offset, length)
}

/// Build the partition table with `IOCTL_DISK_CREATE_DISK` +
/// `IOCTL_DISK_SET_DRIVE_LAYOUT_EX`. Locale-independent and fully verifiable.
fn create_partition_ioctl(
    index: u32,
    disk_size: u64,
    partition_type: u8,
    log: &SharedLog,
) -> Result<()> {
    use windows_sys::Win32::System::Ioctl::{
        DRIVE_LAYOUT_INFORMATION_EX, DRIVE_LAYOUT_INFORMATION_EX_0, DRIVE_LAYOUT_INFORMATION_MBR,
        IOCTL_DISK_CREATE_DISK, IOCTL_DISK_SET_DRIVE_LAYOUT_EX, IOCTL_DISK_UPDATE_PROPERTIES,
        PARTITION_INFORMATION_EX, PARTITION_INFORMATION_EX_0, PARTITION_INFORMATION_MBR,
        PARTITION_STYLE_MBR,
    };

    let h = open_drive(index)?;

    #[repr(C)]
    struct CreateDisk {
        style: u32,
        mbr_signature: u32,
    }
    let create = CreateDisk {
        style: 0,
        mbr_signature: 0xDEAD_BEEF,
    };
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_DISK_CREATE_DISK,
            &create as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<CreateDisk>() as u32,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        let e = crate::error::last_error();
        unsafe { CloseHandle(h) };
        return Err(e);
    }

    let start_lba: i64 = 2048; // 1 MiB alignment
    let total_sectors = (disk_size / 512) as i64;
    let length_sectors = (total_sectors - start_lba).max(1);

    // Zero-initialise so the union bytes the kernel does not read (the GPT
    // half of the MBR layout) are defined rather than stack garbage.
    let mut layout: DRIVE_LAYOUT_INFORMATION_EX = unsafe { std::mem::zeroed() };
    layout.PartitionStyle = PARTITION_STYLE_MBR as u32;
    layout.PartitionCount = 1;
    layout.Anonymous = DRIVE_LAYOUT_INFORMATION_EX_0 {
        Mbr: DRIVE_LAYOUT_INFORMATION_MBR {
            Signature: 0xDEAD_BEEF,
            CheckSum: 0,
        },
    };
    layout.PartitionEntry = [PARTITION_INFORMATION_EX {
        PartitionStyle: PARTITION_STYLE_MBR,
        StartingOffset: start_lba * 512,
        PartitionLength: length_sectors * 512,
        PartitionNumber: 1,
        RewritePartition: 1,
        IsServicePartition: 0,
        Anonymous: PARTITION_INFORMATION_EX_0 {
            Mbr: PARTITION_INFORMATION_MBR {
                PartitionType: partition_type,
                BootIndicator: 0,
                RecognizedPartition: 1,
                HiddenSectors: start_lba as u32,
                PartitionId: windows_sys::core::GUID {
                    data1: 0,
                    data2: 0,
                    data3: 0,
                    data4: [0; 8],
                },
            },
        },
    }];

    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_DISK_SET_DRIVE_LAYOUT_EX,
            &layout as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<DRIVE_LAYOUT_INFORMATION_EX>() as u32,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        let e = crate::error::last_error();
        unsafe { CloseHandle(h) };
        return Err(e);
    }

    let _ = unsafe {
        DeviceIoControl(
            h,
            IOCTL_DISK_UPDATE_PROPERTIES,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    };
    unsafe { CloseHandle(h) };
    note(
        log,
        crate::log::LogLevel::Info,
        "ioctl layout written (MBR, one partition)",
    );
    Ok(())
}

/// Build a single-partition MBR starting at LBA 2048 (1 MiB aligned).
/// Returns `None` if the disk is too small to partition.
pub(crate) fn build_mbr(disk_size: u64, partition_type: u8) -> Option<([u8; 512], u32, u32)> {
    if disk_size < 1024 * 1024 {
        return None;
    }
    let total_sectors = disk_size / 512;
    let start_lba: u64 = 2048;
    let sectors = total_sectors.saturating_sub(start_lba).min(u32::MAX as u64) as u32;
    if sectors == 0 {
        return None;
    }

    let mut mbr = [0u8; 512];
    let e = 446;
    mbr[e] = 0x00; // not bootable
    mbr[e + 1] = 0xFE; // CHS start (dummy, LBA is authoritative)
    mbr[e + 2] = 0xFF;
    mbr[e + 3] = 0xFF;
    mbr[e + 4] = partition_type; // filesystem-matching partition type
    mbr[e + 5] = 0xFE; // CHS end (dummy)
    mbr[e + 6] = 0xFF;
    mbr[e + 7] = 0xFF;
    mbr[e + 8..e + 12].copy_from_slice(&(start_lba as u32).to_le_bytes());
    mbr[e + 12..e + 16].copy_from_slice(&sectors.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;

    Some((mbr, start_lba as u32, sectors))
}

/// Last resort: write a valid MBR by hand. Independent of DiskPart and VDS.
fn write_raw_mbr(index: u32, disk_size: u64, partition_type: u8, log: &SharedLog) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::FlushFileBuffers;

    let (mbr, start_lba, sectors) = build_mbr(disk_size, partition_type)
        .ok_or_else(|| Error::Internal("disk size is unknown or too small to partition".into()))?;

    let h = open_drive(index)?;
    let mut written: u32 = 0;
    let ok = unsafe { WriteFile(h, mbr.as_ptr(), 512, &mut written, ptr::null_mut()) };
    if ok == 0 {
        let e = crate::error::last_error();
        unsafe { CloseHandle(h) };
        return Err(e);
    }
    let _ = unsafe { FlushFileBuffers(h) };
    unsafe { CloseHandle(h) };
    note(
        log,
        crate::log::LogLevel::Info,
        &format!(
            "raw MBR written (start LBA {}, {} sectors)",
            start_lba, sectors
        ),
    );
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
    let _ = unsafe { FlushFileBuffers(h) };
    unsafe { CloseHandle(h) };
    note(
        log,
        crate::log::LogLevel::Success,
        "First/last 1 MiB zeroed and flushed",
    );
    Ok(())
}

fn open_drive(index: u32) -> Result<windows_sys::Win32::Foundation::HANDLE> {
    let path = format!(r"\\.\PhysicalDrive{}", index);
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut last_error = None;
    // DiskPart/VDS and antivirus can hold the disk for a moment after a
    // previous operation; retry briefly before giving up.
    for attempt in 0..12 {
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
        if h != INVALID_HANDLE_VALUE {
            return Ok(h);
        }
        last_error = Some(crate::error::last_error());
        if attempt < 11 {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    Err(last_error
        .unwrap_or_else(|| crate::error::Error::Internal("cannot open the physical drive".into())))
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

    #[test]
    fn mbr_has_one_fat32_partition_at_1mib() {
        let size = 32u64 * 1024 * 1024 * 1024;
        let (mbr, start, sectors) =
            build_mbr(size, FsChoice::Fat32.partition_type_byte()).expect("mbr");
        assert_eq!(mbr[510], 0x55);
        assert_eq!(mbr[511], 0xAA);
        assert_eq!(mbr[446], 0x00); // not bootable
        assert_eq!(mbr[446 + 4], 0x0C); // FAT32 LBA
        assert_eq!(start, 2048);
        assert_eq!(sectors as u64, size / 512 - 2048);
        assert_eq!(
            u32::from_le_bytes([mbr[454], mbr[455], mbr[456], mbr[457]]),
            2048
        );
    }

    #[test]
    fn mbr_uses_the_requested_partition_type() {
        // The fallback writers must not leave a FAT32 type byte on an exFAT/NTFS
        // partition.
        let size = 32u64 * 1024 * 1024 * 1024;
        let (fat, _, _) = build_mbr(size, FsChoice::Fat32.partition_type_byte()).unwrap();
        let (exfat, _, _) = build_mbr(size, FsChoice::ExFat.partition_type_byte()).unwrap();
        let (ntfs, _, _) = build_mbr(size, FsChoice::Ntfs.partition_type_byte()).unwrap();
        assert_eq!(fat[446 + 4], 0x0C);
        assert_eq!(exfat[446 + 4], 0x07);
        assert_eq!(ntfs[446 + 4], 0x07);
    }

    #[test]
    fn create_partition_script_formats_quick_by_default() {
        let opts = RepairOptions::default();
        let script = create_partition_script(3, &opts);
        assert!(script.contains("clean\ncreate partition primary\n"));
        assert!(script.contains("format fs=FAT32 label=STICK quick\n"));
        assert!(script.contains("assign\nrescan\n"));
    }

    #[test]
    fn create_partition_script_skips_format_for_rebuild_only() {
        let opts = RepairOptions {
            rebuild_only: true,
            ..Default::default()
        };
        let script = create_partition_script(3, &opts);
        assert!(script.contains("create partition primary\n"));
        assert!(!script.contains("format fs="));
    }

    #[test]
    fn create_partition_script_full_format_when_not_quick() {
        let opts = RepairOptions {
            quick: false,
            fs: FsChoice::ExFat,
            ..Default::default()
        };
        let script = create_partition_script(3, &opts);
        assert!(script.contains("format fs=exFAT label=STICK\n"));
        assert!(!script.contains(" quick\n"));
    }

    #[test]
    fn mbr_rejects_unusable_disks() {
        assert!(build_mbr(0, 0x0C).is_none());
        assert!(build_mbr(4096, 0x0C).is_none());
        // 1 MiB disk has no room after the 1 MiB alignment.
        assert!(build_mbr(1024 * 1024, 0x0C).is_none());
    }

    // ---- verification decision (the false-green guard) ----

    #[test]
    fn verification_requires_at_least_one_passing_check() {
        assert!(!VerificationOutcome::default().is_success());
        assert_eq!(VerificationOutcome::default().ran(), 0);
    }

    #[test]
    fn verification_accepts_all_passing_checks() {
        let v = VerificationOutcome {
            boot_sector: Some(true),
            volume_fs: Some(true),
            read_write: Some(true),
        };
        assert!(v.is_success());
        assert_eq!(v.ran(), 3);
    }

    #[test]
    fn verification_accepts_boot_sector_only_when_no_volume() {
        // The documented case: Windows never registered a volume, so the boot
        // sector is the only signal and it is authoritative.
        let v = VerificationOutcome {
            boot_sector: Some(true),
            volume_fs: None,
            read_write: None,
        };
        assert!(v.is_success());
    }

    #[test]
    fn verification_rejects_boot_sector_failure() {
        let v = VerificationOutcome {
            boot_sector: Some(false),
            volume_fs: Some(true),
            read_write: Some(true),
        };
        assert!(!v.is_success());
        assert!(v.failure_reason().contains("boot sector"));
    }

    #[test]
    fn verification_rejects_volume_fs_failure() {
        let v = VerificationOutcome {
            boot_sector: Some(true),
            volume_fs: Some(false),
            read_write: Some(true),
        };
        assert!(!v.is_success());
        assert!(v.failure_reason().contains("GetVolumeInformationW"));
    }

    #[test]
    fn verification_rejects_read_write_failure() {
        let v = VerificationOutcome {
            boot_sector: Some(true),
            volume_fs: Some(true),
            read_write: Some(false),
        };
        assert!(!v.is_success());
        assert!(v.failure_reason().contains("read/write"));
    }

    #[test]
    fn verification_rejects_boot_sector_only_failure() {
        let v = VerificationOutcome {
            boot_sector: Some(false),
            volume_fs: None,
            read_write: None,
        };
        assert!(!v.is_success());
    }

    // ---- partition usability ----

    #[test]
    fn layout_is_usable_mbr_accepts_a_real_partition() {
        assert!(layout_is_usable_mbr(0, 1, 2048 * 512, 100_000 * 512));
    }

    #[test]
    fn layout_is_usable_mbr_rejects_gpt() {
        // A GPT entry would have fooled the old `PartitionCount > 0` check.
        assert!(!layout_is_usable_mbr(1, 1, 1024 * 1024, 100_000 * 512));
    }

    #[test]
    fn layout_is_usable_mbr_rejects_empty_or_zero_sized() {
        assert!(!layout_is_usable_mbr(0, 0, 0, 0));
        assert!(!layout_is_usable_mbr(0, 1, 0, 100));
        assert!(!layout_is_usable_mbr(0, 1, 2048 * 512, 0));
    }
}
