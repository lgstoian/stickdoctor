//! Additional repair / diagnostic tools for removable media.
//!
//! These cover the cases the plain "repair and format" path does not:
//! lost partition tables (TestDisk-style search), write protection at the
//! *volume* level, filesystem repair with `chkdsk`, counterfeit-capacity
//! detection, and full-overwrite formatting (SD Association formatter style).
//!
//! @sd file module="crates/stickdoctor-core/src/tools.rs" role="diagnostic and repair tools" entry="scan_lost_partitions" exit="full_overwrite"
//! @sd anchor name=core.tools text="extra tools"
//! @sd verify id=capacity-test fn="sample_offsets" proves="capacity-test offsets are aligned, distinct and in bounds" negative_test=sample_offsets_are_empty_for_tiny_media
//! @sd invariant id=INV-3 text="Disk-modifying tools run only on removable media" test=require_safe_refuses_non_safe_statuses
//! @sd hazard text="writes test patterns to the media" guard="safety gate (removable media only) and explicit user confirmation"

use crate::error::{Error, Result};
use std::ptr;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_INVALID_FUNCTION,
    ERROR_INVALID_PARAMETER, ERROR_IO_DEVICE, ERROR_LOCK_VIOLATION, ERROR_NOT_READY,
    ERROR_SHARING_VIOLATION, ERROR_WRITE_PROTECT, GENERIC_READ, GENERIC_WRITE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, SetFilePointerEx, WriteFile, FILE_BEGIN,
    FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};

// ------------------------------------------------------------ lost partitions

#[derive(Debug, Clone)]
pub struct LostPartition {
    /// LBA of the boot sector that was found.
    pub lba: u64,
    /// Human-readable kind, e.g. "FAT32", "exFAT", "NTFS".
    pub kind: &'static str,
    /// The volume boot sector signature that was matched.
    pub signature: String,
}

/// Scan the disk for filesystem boot sectors that are not referenced by the
/// current partition table. This is the read-only equivalent of TestDisk's
/// "Quick Search".
///
/// Probing every 64 MiB keeps this fast on large cards while still finding
/// partitions whose table was wiped.
pub fn scan_lost_partitions(index: u32) -> Vec<LostPartition> {
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
        return Vec::new();
    }

    let mut found = Vec::new();
    let total_sectors = match crate::props::inspect_properties(index) {
        Ok(p) => p.size_bytes / 512,
        Err(_) => 0,
    };
    if total_sectors == 0 {
        unsafe { CloseHandle(h) };
        return found;
    }

    let step: u64 = 64 * 1024 * 1024 / 512; // 64 MiB
    let mut lba: u64 = 2048; // skip the partition table area
    while lba < total_sectors {
        if let Some(lp) = probe_sector(h, lba) {
            found.push(lp);
            // Skip past the partition we just found to avoid duplicate hits.
            lba += 2048;
        } else {
            lba += step;
        }
    }

    unsafe { CloseHandle(h) };
    found
}

fn probe_sector(h: windows_sys::Win32::Foundation::HANDLE, lba: u64) -> Option<LostPartition> {
    let mut buf = [0u8; 2048];
    let mut pos: i64 = 0;
    let offset = (lba as i64) * 512;
    let ok = unsafe { SetFilePointerEx(h, offset, &mut pos, 0) };
    if ok == 0 {
        return None;
    }
    let mut read: u32 = 0;
    let ok = unsafe { ReadFile(h, buf.as_mut_ptr(), 2048, &mut read, ptr::null_mut()) };
    if ok == 0 || read < 512 {
        return None;
    }

    let kind = classify_sector(&buf);
    kind.map(|(k, sig)| LostPartition {
        lba,
        kind: k,
        signature: sig.to_string(),
    })
}

fn classify_sector(buf: &[u8; 2048]) -> Option<(&'static str, &'static str)> {
    // exFAT / NTFS: "EXFAT   " / "NTFS    " at offset 3
    if &buf[3..11] == b"EXFAT   " {
        return Some(("exFAT", "EXFAT"));
    }
    if &buf[3..11] == b"NTFS    " {
        return Some(("NTFS", "NTFS"));
    }
    // FAT32: "FAT32   " at offset 82
    if &buf[82..90] == b"FAT32   " {
        return Some(("FAT32", "FAT32"));
    }
    // FAT16: "FAT16   " at offset 54
    if &buf[54..62] == b"FAT16   " {
        return Some(("FAT16", "FAT16"));
    }
    // FAT12: "FAT12   " at offset 54
    if &buf[54..62] == b"FAT12   " {
        return Some(("FAT12", "FAT12"));
    }
    // ext2/3/4: 0x53 0xEF at offset 0x438
    if buf[0x438] == 0x53 && buf[0x439] == 0xEF {
        return Some(("ext", "ext"));
    }
    // ISO9660 (superfloppy): "CD001" at offset 1
    if &buf[1..6] == b"CD001" {
        return Some(("ISO9660", "CD001"));
    }
    None
}

// ---------------------------------------------------------- write protection

/// Clear the read-only attribute at both the disk and volume level.
/// Returns a list of messages describing what was done / what failed.
pub fn clear_read_only(index: u32) -> Result<Vec<String>> {
    crate::safety::ensure_safe_for_tool(index)?;
    let mut messages = Vec::new();

    // 1. Disk-level attribute via diskpart (handles the VDS "read-only" bit).
    let script = format!("select disk {}\nattributes disk clear readonly\n", index);
    match crate::format::run_diskpart(&script) {
        Ok(out) if out.to_lowercase().contains("attributes cleared") => {
            messages.push("Cleared the disk read-only attribute.".to_string());
        }
        Ok(out) => messages.push(format!("Disk attribute clear: {}", out.trim())),
        Err(e) => messages.push(format!("Disk attribute clear failed: {}", e)),
    }

    // 2. Volume-level attribute for any volume on the disk.
    let vols = crate::volumes::disk_volumes(index)?;
    if vols.is_empty() {
        messages.push("No volume found to clear.".to_string());
    }
    for v in &vols {
        let target = match v.drive_letter.clone() {
            Some(l) => l,
            None => continue,
        };
        let script = format!(
            "select volume {}\nattributes volume clear readonly\n",
            target
        );
        match crate::format::run_diskpart(&script) {
            Ok(out) if out.to_lowercase().contains("attributes cleared") => {
                messages.push(format!("Cleared the read-only attribute on {}.", target));
            }
            Ok(out) => messages.push(format!("{}: {}", target, out.trim())),
            Err(e) => messages.push(format!("{}: {}", target, e)),
        }
    }

    // 3. Registry policy (read-only check).
    let policy = crate::props::read_only_sources();
    if policy.is_empty() {
        messages.push("No registry write-protection policy is set.".to_string());
    } else {
        messages.push(format!(
            "Registry write-protection is set: {}. Set WriteProtect to 0 or delete the key.",
            policy.join("; ")
        ));
    }

    Ok(messages)
}

// ------------------------------------------------------------------- chkdsk

/// Run `chkdsk <letter>: /f` and return its output.
pub fn run_chkdsk(letter: &str) -> Result<String> {
    use std::process::Command;
    let drive = format!("{}:", letter);
    // Resolve chkdsk from System32 like diskpart/format.com: a bare
    // `Command::new("chkdsk")` would search PATH and the current directory.
    let out = Command::new(crate::format::system32("chkdsk.exe"))
        .arg(&drive)
        .arg("/f")
        .output();
    match out {
        Ok(o) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            // chkdsk exits 1 when it fixed errors, so a non-zero status is not
            // always a failure — but it must not be reported silently.
            if o.status.success() {
                Ok(text)
            } else {
                Ok(format!("chkdsk exited with {}:\n{}", o.status, text))
            }
        }
        Err(e) => Err(Error::Io(e)),
    }
}

// ------------------------------------------------------- counterfeit capacity

/// How thoroughly [`verify_capacity_with_mode`] probes the media.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityTestMode {
    /// Write and verify one block at many offsets spread across the whole
    /// advertised capacity. Fast (seconds) and catches most fake cards, but it
    /// samples rather than proving every byte.
    Quick,
    /// Write every byte of the advertised capacity and read it all back. Slow
    /// (tens of minutes on a large card) but definitive.
    Full,
}

impl std::fmt::Display for CapacityTestMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapacityTestMode::Quick => write!(f, "quick"),
            CapacityTestMode::Full => write!(f, "full"),
        }
    }
}

/// What the capacity test concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityVerdict {
    /// Every tested block read back exactly as written.
    Genuine,
    /// Data written at one offset was found at another, or vanished entirely:
    /// the advertised capacity does not exist.
    Counterfeit,
    /// The pattern came back from the right offset but with changed bytes, so
    /// the flash is failing rather than lying about its size.
    Corrupt,
    /// The test could not complete (write-protected, access denied, I/O error).
    /// This is **not** a counterfeit verdict.
    Inconclusive,
}

impl CapacityVerdict {
    pub fn is_genuine(self) -> bool {
        self == CapacityVerdict::Genuine
    }

    pub fn label(self) -> &'static str {
        match self {
            CapacityVerdict::Genuine => "GENUINE",
            CapacityVerdict::Counterfeit => "COUNTERFEIT",
            CapacityVerdict::Corrupt => "CORRUPT",
            CapacityVerdict::Inconclusive => "INCONCLUSIVE",
        }
    }
}

/// A write that landed at a different offset than the one requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrapEvidence {
    /// The offset that was read back.
    pub read_offset: u64,
    /// The offset whose test pattern was found there.
    pub written_at: u64,
    /// `written_at - read_offset`. The real capacity divides this value; it is
    /// the exact capacity when the write wrapped once and `read_offset` is 0.
    pub capacity_multiple: u64,
}

#[derive(Debug, Clone)]
pub struct CapacityReport {
    pub verdict: CapacityVerdict,
    /// Convenience flag: `verdict == CapacityVerdict::Genuine`.
    pub genuine: bool,
    /// The offset where verification first failed.
    pub first_failure: Option<u64>,
    /// The offsets that were written.
    pub tested_offsets: Vec<u64>,
    /// How many bytes were read back and matched.
    pub bytes_verified: u64,
    pub mode: CapacityTestMode,
    /// Explanation for `Inconclusive` / `Corrupt` / `Counterfeit`.
    pub error: Option<String>,
    /// Set when a write was found at a different offset.
    pub wrap_evidence: Option<WrapEvidence>,
}

/// A quick-mode sample block. Large enough to be a real transfer, small enough
/// that 100+ samples are still fast.
pub(crate) const CAPACITY_QUICK_BLOCK: u64 = 64 * 1024;

/// A full-mode block.
pub(crate) const CAPACITY_FULL_BLOCK: u64 = 1024 * 1024;

/// Raw-device I/O must be aligned to the device sector size. 4 KiB covers both
/// 512-byte and 4Kn media.
pub(crate) const CAPACITY_ALIGN: u64 = 4096;

/// Number of evenly spaced samples in quick mode.
const CAPACITY_SAMPLES: u64 = 128;

/// Identifies a StickDoctor capacity-test block.
const PATTERN_MAGIC: u64 = 0x5344_5041_5454_4552; // "SDPATTER"

/// The offsets quick mode writes and verifies.
///
/// Pure and separate so the offsets can be checked for alignment, distinctness
/// and bounds: an unaligned offset makes `WriteFile` fail with
/// `ERROR_INVALID_PARAMETER`, and a duplicate offset would make the test
/// vacuous.
pub(crate) fn sample_offsets(size: u64) -> Vec<u64> {
    let block = CAPACITY_QUICK_BLOCK;
    if size < 2 * block {
        return Vec::new();
    }
    // Keep the MBR/GPT intact on normal media, but test tiny media from 0.
    let start = if size >= 2 * 1024 * 1024 {
        1024 * 1024
    } else {
        0
    };
    let last = ((size - block) / CAPACITY_ALIGN) * CAPACITY_ALIGN;
    if last < start + block {
        return Vec::new();
    }

    let mut offsets = Vec::new();

    // Power-of-two boundaries. A controller that really has 8/16/32/64 GiB and
    // wraps at that boundary writes both R and 2R onto the same physical block,
    // so one of the two reads back with the other's pattern.
    let mut boundary = 1u64 << 20;
    while boundary <= last {
        if boundary + block <= size {
            offsets.push(boundary);
        }
        boundary = match boundary.checked_mul(2) {
            Some(v) => v,
            None => break,
        };
    }

    // Evenly spaced samples across the entire advertised capacity.
    let step = (last - start) / (CAPACITY_SAMPLES - 1);
    for i in 0..CAPACITY_SAMPLES {
        let raw = start + step * i;
        let aligned = (raw / CAPACITY_ALIGN) * CAPACITY_ALIGN;
        if aligned >= start && aligned + block <= size {
            offsets.push(aligned);
        }
    }
    if last + block <= size {
        offsets.push(last);
    }

    offsets.sort_unstable();
    offsets.dedup();
    offsets
}

/// Fill `buf` with data that depends on the absolute byte offset.
///
/// The first 16 bytes carry a magic word and the offset, so a block found at
/// the wrong place proves that the controller wrapped the address instead of
/// storing it. The rest is a deterministic SplitMix64 stream, so it does not
/// compress and cannot be replayed from a small cache.
pub(crate) fn fill_pattern(buf: &mut [u8], offset: u64) {
    let mut state = offset ^ PATTERN_MAGIC ^ 0xA5A5_5A5A_1234_5678;
    for chunk in buf.chunks_mut(8) {
        let word = splitmix64(&mut state).to_le_bytes();
        chunk.copy_from_slice(&word[..chunk.len()]);
    }
    if buf.len() >= 16 {
        buf[0..8].copy_from_slice(&PATTERN_MAGIC.to_le_bytes());
        buf[8..16].copy_from_slice(&offset.to_le_bytes());
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The offset encoded in a test block, if the block still carries our header.
pub(crate) fn pattern_origin(buf: &[u8]) -> Option<u64> {
    if buf.len() < 16 {
        return None;
    }
    let magic = u64::from_le_bytes(buf[0..8].try_into().ok()?);
    if magic != PATTERN_MAGIC {
        return None;
    }
    Some(u64::from_le_bytes(buf[8..16].try_into().ok()?))
}

/// `written_at - read_offset`, the value the real capacity divides when a write
/// wraps. Returns `None` when the evidence is not a forward wrap.
pub(crate) fn wrap_capacity_multiple(read_offset: u64, written_at: u64) -> Option<u64> {
    let delta = written_at.checked_sub(read_offset)?;
    if delta == 0 || read_offset >= delta {
        return None;
    }
    Some(delta)
}

/// A block-sized buffer whose pointer satisfies the raw-device alignment rule.
struct AlignedBlock {
    backing: Vec<u8>,
    offset: usize,
    len: usize,
}

impl AlignedBlock {
    fn new(len: usize) -> Option<Self> {
        let extra = CAPACITY_ALIGN as usize;
        let backing = vec![0u8; len + extra];
        let offset = backing.as_ptr().align_offset(CAPACITY_ALIGN as usize);
        if offset == usize::MAX {
            return None;
        }
        Some(Self {
            backing,
            offset,
            len,
        })
    }

    fn as_ptr(&self) -> *const u8 {
        unsafe { self.backing.as_ptr().add(self.offset) }
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        unsafe { self.backing.as_mut_ptr().add(self.offset) }
    }

    fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.as_ptr(), self.len) }
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.as_mut_ptr(), self.len) }
    }
}

/// Open the raw disk with cache-defeating flags.
///
/// `FILE_FLAG_NO_BUFFERING` is what makes the read-back test the media rather
/// than the Windows cache. A few USB bridges reject it, so fall back to
/// write-through and flush explicitly.
fn open_capacity_handle(
    index: u32,
    no_buffering: bool,
) -> Result<(windows_sys::Win32::Foundation::HANDLE, bool)> {
    use windows_sys::Win32::Foundation::HANDLE;
    let path = format!(r"\\.\PhysicalDrive{}", index);
    let path_w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let open = |flags: u32| -> HANDLE {
        unsafe {
            CreateFileW(
                path_w.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                flags,
                ptr::null_mut(),
            )
        }
    };
    if no_buffering {
        let h = open(FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH);
        if h != INVALID_HANDLE_VALUE {
            return Ok((h, true));
        }
    }
    let h = open(FILE_FLAG_WRITE_THROUGH);
    if h != INVALID_HANDLE_VALUE {
        return Ok((h, false));
    }
    Err(crate::error::last_error())
}

fn seek_capacity(
    h: windows_sys::Win32::Foundation::HANDLE,
    offset: u64,
) -> std::result::Result<(), u32> {
    let mut pos: i64 = 0;
    let ok = unsafe { SetFilePointerEx(h, offset as i64, &mut pos, FILE_BEGIN) };
    if ok == 0 {
        Err(unsafe { GetLastError() })
    } else {
        Ok(())
    }
}

fn write_capacity_block(
    h: windows_sys::Win32::Foundation::HANDLE,
    ptr: *const u8,
    len: u32,
    offset: u64,
) -> std::result::Result<(), u32> {
    seek_capacity(h, offset)?;
    let mut written: u32 = 0;
    let ok = unsafe { WriteFile(h, ptr, len, &mut written, ptr::null_mut()) };
    if ok == 0 {
        return Err(unsafe { GetLastError() });
    }
    if written != len {
        return Err(ERROR_IO_DEVICE);
    }
    Ok(())
}

fn read_capacity_block(
    h: windows_sys::Win32::Foundation::HANDLE,
    ptr: *mut u8,
    len: u32,
    offset: u64,
) -> std::result::Result<(), u32> {
    seek_capacity(h, offset)?;
    let mut read: u32 = 0;
    let ok = unsafe { ReadFile(h, ptr, len, &mut read, ptr::null_mut()) };
    if ok == 0 {
        return Err(unsafe { GetLastError() });
    }
    if read != len {
        return Err(ERROR_IO_DEVICE);
    }
    Ok(())
}

fn capacity_io_message(action: &str, offset: u64, code: u32) -> String {
    format!(
        "{} failed at offset {}: {}",
        action,
        offset,
        crate::error::win32_error_message(code)
    )
}

fn is_environment_error(code: u32) -> bool {
    matches!(
        code,
        ERROR_ACCESS_DENIED
            | ERROR_WRITE_PROTECT
            | ERROR_SHARING_VIOLATION
            | ERROR_LOCK_VIOLATION
            | ERROR_NOT_READY
            | ERROR_INVALID_FUNCTION
    )
}

/// Close the raw handle, release the volume locks, and ask Windows to remount.
/// Used on every exit path so an error never leaves the card without a letter.
fn release_raw(
    handle: windows_sys::Win32::Foundation::HANDLE,
    locks: crate::volumes::VolumeLocks,
    index: u32,
) -> usize {
    unsafe { CloseHandle(handle) };
    drop(locks);
    crate::volumes::remount_volumes(index)
}

/// Write a recognizable pattern at many offsets and read it back.
///
/// Counterfeit cards report a larger capacity than they physically have. Writes
/// past the real capacity either vanish or wrap onto earlier blocks, so writing
/// the entire sample set *before* reading any of it back exposes the fraud.
/// (Reading each block immediately after writing it proves nothing: a wrapping
/// controller happily returns the data it just stored.)
///
/// This WRITES to the media, so it needs the raw device dismounted. Only run it
/// on a card whose data you do not need.
pub fn verify_capacity(index: u32, log: &crate::SharedLog) -> Result<CapacityReport> {
    verify_capacity_with_mode(index, CapacityTestMode::Quick, log)
}

/// Exhaustive variant: write the whole advertised capacity, then read it all
/// back. Definitive, but slow on large media.
pub fn verify_capacity_full(index: u32, log: &crate::SharedLog) -> Result<CapacityReport> {
    verify_capacity_with_mode(index, CapacityTestMode::Full, log)
}

/// Run the counterfeit-capacity test in the requested mode.
pub fn verify_capacity_with_mode(
    index: u32,
    mode: CapacityTestMode,
    log: &crate::SharedLog,
) -> Result<CapacityReport> {
    match verify_capacity_once(index, mode, log, true) {
        Err(Error::Internal(msg)) if msg.contains("ERROR_INVALID_PARAMETER") => {
            // Some USB bridges accept FILE_FLAG_NO_BUFFERING on open but reject
            // the aligned I/O. Retry with write-through and explicit flushes
            // rather than reporting an internal error.
            if let Ok(mut l) = log.lock() {
                l.warn(
                    "The reader rejected no-buffering I/O; retrying with write-through and explicit flushes.",
                );
            }
            verify_capacity_once(index, mode, log, false)
        }
        other => other,
    }
}

fn verify_capacity_once(
    index: u32,
    mode: CapacityTestMode,
    log: &crate::SharedLog,
    no_buffering: bool,
) -> Result<CapacityReport> {
    crate::safety::ensure_safe_for_tool(index)?;
    let size = crate::props::inspect_properties(index)?.size_bytes;
    if size == 0 {
        return Err(Error::Internal(
            "disk size is unknown or zero; refusing to test".into(),
        ));
    }

    let block = match mode {
        CapacityTestMode::Quick => CAPACITY_QUICK_BLOCK,
        CapacityTestMode::Full => CAPACITY_FULL_BLOCK,
    };
    let offsets: Vec<u64> = match mode {
        CapacityTestMode::Quick => sample_offsets(size),
        CapacityTestMode::Full => {
            let start = if size >= 2 * 1024 * 1024 {
                1024 * 1024
            } else {
                0
            };
            if size < start + block {
                Vec::new()
            } else {
                let mut offsets: Vec<u64> = (start..=size - block)
                    .step_by(block as usize)
                    .map(|off| (off / CAPACITY_ALIGN) * CAPACITY_ALIGN)
                    .filter(|off| *off + block <= size)
                    .collect();
                // Include the final block even when the size is not a whole
                // number of 1 MiB blocks.
                let last = ((size - block) / CAPACITY_ALIGN) * CAPACITY_ALIGN;
                if last + block <= size && offsets.last().copied() != Some(last) {
                    offsets.push(last);
                }
                offsets
            }
        }
    };
    if offsets.is_empty() {
        return Err(Error::Internal(
            "media is too small for a meaningful capacity test".into(),
        ));
    }

    let (handle, unbuffered) = open_capacity_handle(index, no_buffering)?;
    let mut expect = match AlignedBlock::new(block as usize) {
        Some(b) => b,
        None => {
            unsafe { CloseHandle(handle) };
            return Err(Error::Internal(
                "could not allocate an aligned I/O buffer".into(),
            ));
        }
    };
    let mut read = match AlignedBlock::new(block as usize) {
        Some(b) => b,
        None => {
            unsafe { CloseHandle(handle) };
            return Err(Error::Internal(
                "could not allocate an aligned I/O buffer".into(),
            ));
        }
    };

    if let Ok(mut l) = log.lock() {
        l.info(format!(
            "Capacity test ({mode}): {} blocks of {} KiB across {:.1} GiB.",
            offsets.len(),
            block / 1024,
            size as f64 / (1024.0 * 1024.0 * 1024.0)
        ));
        if no_buffering && !unbuffered {
            l.warn(
                "The device rejected no-buffering I/O; using write-through and explicit flushes.",
            );
        }
    }

    // Windows rejects raw writes to sectors that belong to a mounted volume,
    // and a dismount alone is not enough: the lock must stay held for the whole
    // test. The guard closes the handles when it is dropped.
    let locks = crate::volumes::lock_dismount_all_held(index).unwrap_or_default();
    if locks.locked() == 0 {
        if let Ok(mut l) = log.lock() {
            l.warn(
                "Could not lock or dismount the volume; the capacity test will still try, but a busy volume can make raw writes fail.",
            );
        }
    }

    let mut tested = Vec::with_capacity(offsets.len());
    let mut bytes_verified = 0u64;
    let mut first_failure = None;
    let mut error = None;
    let mut wrap_evidence = None;
    let mut verdict = CapacityVerdict::Genuine;

    // ---- Write pass: every sample first, so a wrapped write has a chance to
    // clobber a block that was written earlier.
    let mut last_pct = 0u8;
    for (i, &off) in offsets.iter().enumerate() {
        fill_pattern(expect.as_mut_slice(), off);
        if let Err(code) = write_capacity_block(handle, expect.as_ptr(), block as u32, off) {
            if code == ERROR_INVALID_PARAMETER {
                let _ = release_raw(handle, locks, index);
                return Err(Error::Internal(format!(
                    "the raw device rejected an aligned {block}-byte write at offset {off} (ERROR_INVALID_PARAMETER)"
                )));
            }
            // A refused write is not proof of fraud: the volume may still be
            // mounted, the card may be write-protected, or the flash may be
            // failing. Say so instead of reporting "counterfeit".
            let hint = if is_environment_error(code) {
                " The drive is busy or write-protected; close programs using it and retry."
            } else {
                " The media reported an I/O error; the card may be failing."
            };
            error = Some(format!("{}{hint}", capacity_io_message("Write", off, code)));
            verdict = CapacityVerdict::Inconclusive;
            break;
        }
        tested.push(off);

        if mode == CapacityTestMode::Full {
            let pct = ((i + 1) as u64 * 100 / offsets.len() as u64) as u8;
            if pct >= last_pct + 2 {
                last_pct = pct;
                if let Ok(mut l) = log.lock() {
                    l.info(format!("Writing test data: {pct}%"));
                }
            }
        }
    }

    // Flush before reading: with a write-back cache the data may not be on the
    // media yet, and then the read would only prove that RAM works.
    if verdict == CapacityVerdict::Genuine && unsafe { FlushFileBuffers(handle) } == 0 {
        let code = unsafe { GetLastError() };
        error = Some(capacity_io_message(
            "Flush",
            *offsets.last().unwrap_or(&0),
            code,
        ));
        verdict = CapacityVerdict::Inconclusive;
    }

    // ---- Verify pass.
    if verdict == CapacityVerdict::Genuine {
        last_pct = 0;
        for (i, &off) in offsets.iter().enumerate() {
            if let Err(code) = read_capacity_block(handle, read.as_mut_ptr(), block as u32, off) {
                if code == ERROR_INVALID_PARAMETER {
                    let _ = release_raw(handle, locks, index);
                    return Err(Error::Internal(format!(
                        "the raw device rejected an aligned {block}-byte read at offset {off} (ERROR_INVALID_PARAMETER)"
                    )));
                }
                error = Some(capacity_io_message("Read", off, code));
                verdict = CapacityVerdict::Inconclusive;
                break;
            }
            fill_pattern(expect.as_mut_slice(), off);
            if read.as_slice() != expect.as_slice() {
                first_failure = Some(off);
                match pattern_origin(read.as_slice()) {
                    Some(written_at) if written_at != off => {
                        let multiple = wrap_capacity_multiple(off, written_at)
                            .unwrap_or_else(|| written_at.saturating_sub(off));
                        wrap_evidence = Some(WrapEvidence {
                            read_offset: off,
                            written_at,
                            capacity_multiple: multiple,
                        });
                        error = Some(format!(
                            "Data written at offset {} was found at offset {} — the controller wraps addresses, so the advertised capacity is not real.",
                            written_at, off
                        ));
                        verdict = CapacityVerdict::Counterfeit;
                    }
                    Some(_) => {
                        error = Some(format!(
                            "Data at offset {} changed after it was written — the media is failing.",
                            off
                        ));
                        verdict = CapacityVerdict::Corrupt;
                    }
                    None => {
                        error = Some(format!(
                            "The test pattern at offset {} vanished — the controller accepted the write but did not store it.",
                            off
                        ));
                        verdict = CapacityVerdict::Counterfeit;
                    }
                }
                break;
            }
            bytes_verified += block;

            if mode == CapacityTestMode::Full {
                let pct = ((i + 1) as u64 * 100 / offsets.len() as u64) as u8;
                if pct >= last_pct + 2 {
                    last_pct = pct;
                    if let Ok(mut l) = log.lock() {
                        l.info(format!("Verifying test data: {pct}%"));
                    }
                }
            }
        }
    }

    // Release the locks before reopening the volume, then trigger a remount so
    // the drive letter comes back without a replug.
    let remounted = release_raw(handle, locks, index);
    if remounted == 0 {
        if let Ok(mut l) = log.lock() {
            l.warn(
                "Windows did not remount the volume. Unplug and replug the card to get the drive letter back.",
            );
        }
    }

    if let Ok(mut l) = log.lock() {
        match verdict {
            CapacityVerdict::Genuine => l.success(format!(
                "Capacity test passed ({mode}): {} bytes verified across {} points.",
                bytes_verified,
                tested.len()
            )),
            CapacityVerdict::Counterfeit => l.error(format!(
                "Capacity test FAILED: the card is counterfeit. {}",
                error.as_deref().unwrap_or("")
            )),
            CapacityVerdict::Corrupt => l.error(format!(
                "Capacity test found data corruption: {}. The card is failing, not necessarily fake.",
                error.as_deref().unwrap_or("")
            )),
            CapacityVerdict::Inconclusive => l.warn(format!(
                "Capacity test could not complete: {} This is not a counterfeit verdict; close programs using the drive and retry.",
                error.as_deref().unwrap_or("unknown I/O error.")
            )),
        }
    }

    Ok(CapacityReport {
        verdict,
        genuine: verdict == CapacityVerdict::Genuine,
        first_failure,
        tested_offsets: tested,
        bytes_verified,
        mode,
        error,
        wrap_evidence,
    })
}

// ------------------------------------------------------------- full overwrite

/// Zero the entire card (SD Association formatter style). Much slower than a
/// quick format but fixes cards that quick formatting cannot.
pub fn full_overwrite(index: u32, log: &crate::SharedLog) -> Result<()> {
    crate::safety::ensure_safe_for_tool(index)?;
    let size = crate::props::inspect_properties(index)?.size_bytes;
    if size == 0 {
        return Err(Error::Internal(
            "disk size is unknown or zero; refusing to overwrite".into(),
        ));
    }
    let path = format!(r"\\.\PhysicalDrive{}", index);
    let path_w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let h = unsafe {
        CreateFileW(
            path_w.as_ptr(),
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
    // A mounted volume blocks raw writes with ERROR_ACCESS_DENIED; keep it
    // locked for the whole overwrite.
    let locks = crate::volumes::lock_dismount_all_held(index).unwrap_or_default();

    let chunk = vec![0u8; 8 * 1024 * 1024];
    let mut remaining = size;
    let mut written: u32 = 0;
    let mut last_pct = 0u8;

    while remaining > 0 {
        let to_write = remaining.min(chunk.len() as u64) as u32;
        let ok = unsafe { WriteFile(h, chunk.as_ptr(), to_write, &mut written, ptr::null_mut()) };
        if ok == 0 {
            let e = crate::error::last_error();
            let _ = release_raw(h, locks, index);
            return Err(e);
        }
        if written == 0 {
            // A successful WriteFile that reports no bytes would otherwise loop
            // forever on the same offset.
            let _ = release_raw(h, locks, index);
            return Err(Error::Internal("full overwrite made no progress".into()));
        }
        remaining -= written as u64;
        if let Some(pct) = (size - remaining)
            .checked_mul(100)
            .and_then(|n| n.checked_div(size))
            .map(|n| n as u8)
        {
            if pct >= last_pct + 10 {
                last_pct = pct;
                if let Ok(mut l) = log.lock() {
                    l.info(format!("Overwriting: {}%", pct));
                }
            }
        }
    }

    unsafe { windows_sys::Win32::Storage::FileSystem::FlushFileBuffers(h) };
    let remounted = release_raw(h, locks, index);
    if remounted == 0 {
        if let Ok(mut l) = log.lock() {
            l.warn(
                "Windows did not remount the volume. Unplug and replug the card to get the drive letter back.",
            );
        }
    }

    if let Ok(mut l) = log.lock() {
        l.success("Full overwrite complete.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_offsets_are_aligned_distinct_and_in_bounds() {
        let size = 32u64 * 1024 * 1024 * 1024; // 32 GiB
        let offsets = sample_offsets(size);
        assert!(offsets.len() > 100, "too few samples: {}", offsets.len());

        let mut sorted = offsets.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), offsets.len(), "duplicate samples");

        for off in &offsets {
            assert_eq!(
                off % CAPACITY_ALIGN,
                0,
                "offset {off} is not raw-I/O aligned"
            );
            assert!(
                *off + CAPACITY_QUICK_BLOCK <= size,
                "block at {off} is out of bounds"
            );
        }
        // The low canary (where a wrapped write is most likely to land) and the
        // final block must both be present.
        assert!(offsets.contains(&(1024 * 1024)));
        assert!(offsets.contains(&(size - CAPACITY_QUICK_BLOCK)));
    }

    #[test]
    fn sample_offsets_are_empty_for_tiny_media() {
        assert!(sample_offsets(0).is_empty());
        assert!(sample_offsets(1024).is_empty());
        // Less than two sample blocks: there is no room for a meaningful test.
        assert!(sample_offsets(CAPACITY_QUICK_BLOCK).is_empty());
    }

    #[test]
    fn sample_offsets_include_power_of_two_boundaries() {
        // A controller that really holds 8 GiB but claims 32 GiB must have both
        // 8 GiB and 16 GiB sampled: those two writes alias to the same physical
        // block at the start of the real capacity.
        let offsets = sample_offsets(32 * 1024 * 1024 * 1024);
        assert!(offsets.contains(&(8 * 1024 * 1024 * 1024)));
        assert!(offsets.contains(&(16 * 1024 * 1024 * 1024)));
    }

    #[test]
    fn pattern_roundtrips_and_identifies_its_offset() {
        let mut a = vec![0u8; CAPACITY_QUICK_BLOCK as usize];
        fill_pattern(&mut a, 4 * 1024 * 1024);
        let mut b = vec![0u8; CAPACITY_QUICK_BLOCK as usize];
        fill_pattern(&mut b, 4 * 1024 * 1024);
        assert_eq!(a, b);
        assert_eq!(pattern_origin(&a), Some(4 * 1024 * 1024));

        let mut c = vec![0u8; CAPACITY_QUICK_BLOCK as usize];
        fill_pattern(&mut c, 8 * 1024 * 1024);
        assert_ne!(a, c);
        assert_eq!(pattern_origin(&c), Some(8 * 1024 * 1024));
        assert_ne!(pattern_origin(&c), Some(4 * 1024 * 1024));
    }

    #[test]
    fn short_blocks_do_not_claim_an_origin() {
        assert_eq!(pattern_origin(&[]), None);
        assert_eq!(pattern_origin(&[0u8; 15]), None);
        assert_eq!(pattern_origin(&[0u8; 64]), None);
    }

    #[test]
    fn wrap_hint_requires_a_forward_wrap() {
        let gb32 = 32u64 * 1024 * 1024 * 1024;
        assert_eq!(wrap_capacity_multiple(0, gb32), Some(gb32));
        assert_eq!(
            wrap_capacity_multiple(1024 * 1024, gb32),
            Some(gb32 - 1024 * 1024)
        );
        // Not a wrap: the "written" offset is below the one that was read.
        assert_eq!(wrap_capacity_multiple(gb32, 1024 * 1024), None);
        assert_eq!(wrap_capacity_multiple(5, 5), None);
    }

    #[test]
    fn verdict_labels_are_explicit() {
        assert_eq!(CapacityVerdict::Genuine.label(), "GENUINE");
        assert_eq!(CapacityVerdict::Counterfeit.label(), "COUNTERFEIT");
        assert_eq!(CapacityVerdict::Corrupt.label(), "CORRUPT");
        assert_eq!(CapacityVerdict::Inconclusive.label(), "INCONCLUSIVE");
        assert!(CapacityVerdict::Genuine.is_genuine());
        assert!(!CapacityVerdict::Counterfeit.is_genuine());
    }
}
