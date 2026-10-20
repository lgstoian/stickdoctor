//! Additional repair / diagnostic tools for removable media.
//!
//! These cover the cases the plain "repair and format" path does not:
//! lost partition tables (TestDisk-style search), write protection at the
//! *volume* level, filesystem repair with `chkdsk`, counterfeit-capacity
//! detection, and full-overwrite formatting (SD Association formatter style).

use crate::error::{Error, Result};
use std::ptr;
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, SetFilePointerEx, WriteFile, FILE_SHARE_READ, FILE_SHARE_WRITE,
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
    let out = Command::new("chkdsk").arg(&drive).arg("/f").output();
    match out {
        Ok(o) => Ok(format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )),
        Err(e) => Err(Error::Io(e)),
    }
}

// ------------------------------------------------------- counterfeit capacity

#[derive(Debug, Clone)]
pub struct CapacityReport {
    /// Whether the media appears to be genuine (all test offsets readable).
    pub genuine: bool,
    /// The offset (in bytes) where writing/reading first failed.
    pub first_failure: Option<u64>,
    /// The offsets that were tested.
    pub tested_offsets: Vec<u64>,
}

/// Write a recognizable pattern at several offsets and read it back.
/// Counterfeit cards report a larger capacity than they physically have and
/// silently overwrite earlier data, so a mismatch means the card is fake.
///
/// This WRITES to the media. Only run it on a card you do not need the data on.
pub fn verify_capacity(index: u32, log: &crate::SharedLog) -> Result<CapacityReport> {
    let size = crate::props::inspect_properties(index)?.size_bytes;
    let mut tested = Vec::new();
    let mut first_failure = None;

    if size == 0 {
        return Ok(CapacityReport {
            genuine: false,
            first_failure: None,
            tested_offsets: Vec::new(),
        });
    }

    // Test at 25%, 50%, 75%, and near the end.
    let fractions: [f64; 4] = [0.25, 0.5, 0.75, 0.95];
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

    for (i, frac) in fractions.iter().enumerate() {
        let offset = ((size as f64 * frac) / 512.0).floor() as i64 * 512;
        let marker: u64 = 0x5344_5443_4100_0000u64 + i as u64; // "SDTCA" + index
        let data = marker.to_le_bytes();
        let mut pos: i64 = 0;
        let ok = unsafe { SetFilePointerEx(h, offset, &mut pos, 0) };
        if ok == 0 {
            first_failure = first_failure.or(Some(offset as u64));
            continue;
        }
        let mut written: u32 = 0;
        let ok = unsafe { WriteFile(h, data.as_ptr(), 8, &mut written, ptr::null_mut()) };
        if ok == 0 || written != 8 {
            first_failure = first_failure.or(Some(offset as u64));
            continue;
        }
        // Read back.
        if unsafe { SetFilePointerEx(h, offset, &mut pos, 0) } == 0 {
            first_failure = first_failure.or(Some(offset as u64));
            continue;
        }
        let mut buf = [0u8; 8];
        let mut read: u32 = 0;
        let ok = unsafe { ReadFile(h, buf.as_mut_ptr(), 8, &mut read, ptr::null_mut()) };
        tested.push(offset as u64);
        if ok == 0 || read != 8 {
            first_failure = first_failure.or(Some(offset as u64));
            continue;
        }
        let value = u64::from_le_bytes(buf);
        if value != marker {
            first_failure = first_failure.or(Some(offset as u64));
            if let Ok(mut l) = log.lock() {
                l.warn(format!(
                    "capacity mismatch at offset {}: wrote {:016X}, read {:016X} — counterfeit media",
                    offset, marker, value
                ));
            }
        }
    }

    unsafe { CloseHandle(h) };

    if let Ok(mut l) = log.lock() {
        if first_failure.is_none() {
            l.success(format!("Capacity test passed at {} offsets.", tested.len()));
        } else {
            if let Some(fail) = first_failure {
                l.error(format!(
                    "Capacity test FAILED at offset {} — the card is likely counterfeit.",
                    fail
                ));
            }
        }
    }

    Ok(CapacityReport {
        genuine: first_failure.is_none(),
        first_failure,
        tested_offsets: tested,
    })
}

// ------------------------------------------------------------- full overwrite

/// Zero the entire card (SD Association formatter style). Much slower than a
/// quick format but fixes cards that quick formatting cannot.
pub fn full_overwrite(index: u32, log: &crate::SharedLog) -> Result<()> {
    let size = crate::props::inspect_properties(index)?.size_bytes;
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

    let chunk = vec![0u8; 8 * 1024 * 1024];
    let mut remaining = size;
    let mut written: u32 = 0;
    let mut last_pct = 0u8;

    while remaining > 0 {
        let to_write = remaining.min(chunk.len() as u64) as u32;
        let ok = unsafe { WriteFile(h, chunk.as_ptr(), to_write, &mut written, ptr::null_mut()) };
        if ok == 0 {
            let e = crate::error::last_error();
            unsafe { CloseHandle(h) };
            return Err(e);
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
    unsafe { CloseHandle(h) };

    if let Ok(mut l) = log.lock() {
        l.success("Full overwrite complete.");
    }
    Ok(())
}
