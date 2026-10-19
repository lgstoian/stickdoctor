use crate::error::Result;
use crate::win32_const::{
    FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::ptr;
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

#[derive(Debug, Clone)]
pub struct VolumeInfo {
    /// Volume device path, e.g. `\\?\Volume{guid}\`
    pub device_path: String,
    /// Drive letter if present, e.g. "E:"
    pub drive_letter: Option<String>,
    /// Filesystem reported, if readable (None for RAW/unformatted volumes)
    pub filesystem: Option<String>,
}

/// Enumerate volumes and map them back to a physical drive index.
pub fn disk_volumes(target_disk_index: u32) -> Result<Vec<VolumeInfo>> {
    let mut result = Vec::new();
    use windows_sys::Win32::Storage::FileSystem::{
        FindFirstVolumeW, FindNextVolumeW, FindVolumeClose,
    };

    let mut buf = vec![0u16; 512];
    let h = unsafe { FindFirstVolumeW(buf.as_mut_ptr(), buf.len() as u32) };
    if h == INVALID_HANDLE_VALUE {
        return Err(crate::error::last_error());
    }

    loop {
        let name = OsString::from_wide(&buf)
            .to_string_lossy()
            .trim_end_matches('\0')
            .to_string();
        if !name.is_empty() && volume_on_disk(&name, target_disk_index) {
            result.push(VolumeInfo {
                drive_letter: first_drive_letter(&name),
                filesystem: probe_filesystem(&name),
                device_path: name,
            });
        }

        let ok = unsafe { FindNextVolumeW(h, buf.as_mut_ptr(), buf.len() as u32) };
        if ok == 0 {
            break;
        }
    }
    unsafe { FindVolumeClose(h) };
    Ok(result)
}

/// Wait until a volume appears on the disk (used after creating a partition).
/// Enumerate every volume on the system, without filtering by disk.
pub fn all_volumes() -> Result<Vec<VolumeInfo>> {
    let mut result = Vec::new();
    use windows_sys::Win32::Storage::FileSystem::{
        FindFirstVolumeW, FindNextVolumeW, FindVolumeClose,
    };
    let mut buf = vec![0u16; 512];
    let h = unsafe { FindFirstVolumeW(buf.as_mut_ptr(), buf.len() as u32) };
    if h == INVALID_HANDLE_VALUE {
        return Err(crate::error::last_error());
    }
    loop {
        let name = OsString::from_wide(&buf)
            .to_string_lossy()
            .trim_end_matches('\0')
            .to_string();
        if !name.is_empty() {
            result.push(VolumeInfo {
                drive_letter: first_drive_letter(&name),
                filesystem: probe_filesystem(&name),
                device_path: name,
            });
        }
        let ok = unsafe { FindNextVolumeW(h, buf.as_mut_ptr(), buf.len() as u32) };
        if ok == 0 {
            break;
        }
    }
    unsafe { FindVolumeClose(h) };
    Ok(result)
}
pub fn wait_for_volume(target_disk_index: u32, attempts: u32) -> Option<VolumeInfo> {
    for _ in 0..attempts {
        if let Ok(vols) = disk_volumes(target_disk_index) {
            if let Some(v) = vols.into_iter().next() {
                return Some(v);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    None
}

fn first_drive_letter(volume_name: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumePathNamesForVolumeNameW;
    let wide: Vec<u16> = volume_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut len: u32 = 0;
    unsafe { GetVolumePathNamesForVolumeNameW(wide.as_ptr(), ptr::null_mut(), 0, &mut len) };
    if len == 0 {
        return None;
    }
    let mut paths = vec![0u16; len as usize];
    let ok = unsafe {
        GetVolumePathNamesForVolumeNameW(wide.as_ptr(), paths.as_mut_ptr(), len, &mut len)
    };
    if ok == 0 {
        return None;
    }
    let s = OsString::from_wide(&paths[..len as usize])
        .to_string_lossy()
        .split('\0')
        .next()
        .unwrap_or("")
        .trim_end_matches('\\')
        .to_string();
    if s.len() == 2 && s.ends_with(':') {
        Some(s)
    } else {
        None
    }
}

/// Volumes that span the disk.
///
/// `VOLUME_DISK_EXTENTS` layout (Win32, x64):
///   offset 0 : DWORD  NumberOfDiskExtents
///   offset 4 : padding
///   offset 8 : DISK_EXTENT[0]
/// `DISK_EXTENT`:
///   offset 0 : DWORD         DiskNumber
///   offset 4 : padding
///   offset 8 : LARGE_INTEGER StartingOffset
///   offset 16: LARGE_INTEGER ExtentLength
///   sizeof   : 24
fn volume_on_disk(volume_name: &str, disk_index: u32) -> bool {
    let wide: Vec<u16> = volume_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return false;
    }

    let mut buf = vec![0u8; 512];
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
            ptr::null_mut(),
            0,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            buf.len() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    unsafe { CloseHandle(h) };
    if ok == 0 || returned < 32 {
        return false;
    }

    parse_extent_disks(&buf[..returned as usize]).contains(&disk_index)
}

/// Decode the disk numbers from a raw `VOLUME_DISK_EXTENTS` buffer.
///
/// Layout on x64:
///   offset 0 : DWORD NumberOfDiskExtents
///   offset 4 : padding
///   offset 8 : DISK_EXTENT[0] { DWORD DiskNumber; pad; LARGE_INTEGER offset; LARGE_INTEGER length; }
pub(crate) fn parse_extent_disks(buf: &[u8]) -> Vec<u32> {
    const EXTENT_SIZE: usize = 24;
    const EXTENTS_OFFSET: usize = 8;
    if buf.len() < EXTENTS_OFFSET + 4 {
        return Vec::new();
    }
    let count = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let mut disks = Vec::new();
    for i in 0..count.min(16) {
        let base = EXTENTS_OFFSET + i * EXTENT_SIZE;
        if base + 4 > buf.len() {
            break;
        }
        disks.push(u32::from_le_bytes([
            buf[base],
            buf[base + 1],
            buf[base + 2],
            buf[base + 3],
        ]));
    }
    disks
}

fn probe_filesystem(volume_name: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
    let wide: Vec<u16> = volume_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut fs_buf = vec![0u16; 64];
    let ok = unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            fs_buf.as_mut_ptr(),
            fs_buf.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    let fs = OsString::from_wide(&fs_buf)
        .to_string_lossy()
        .trim_end_matches('\0')
        .to_string();
    if fs.is_empty() {
        None
    } else {
        Some(fs)
    }
}

/// Lock and dismount every volume on a disk. Handles are closed before
/// returning so that an external tool (diskpart) can operate on the disk.
pub fn lock_dismount_all(target_disk_index: u32) -> Result<(usize, usize)> {
    let volumes = disk_volumes(target_disk_index)?;
    let mut locked = 0;
    let mut dismounted = 0;
    for v in &volumes {
        let wide: Vec<u16> = v
            .device_path
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
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
            continue;
        }
        let mut returned: u32 = 0;
        // Locks can transiently fail; retry briefly.
        let mut ok_lock = 0;
        for _ in 0..10 {
            ok_lock = unsafe {
                DeviceIoControl(
                    h,
                    FSCTL_LOCK_VOLUME,
                    ptr::null_mut(),
                    0,
                    ptr::null_mut(),
                    0,
                    &mut returned,
                    ptr::null_mut(),
                )
            };
            if ok_lock != 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        if ok_lock != 0 {
            locked += 1;
        }
        // Dismount even if the exclusive lock was refused: releasing the
        // volume is what lets DiskPart open the disk.
        let ok_dis = unsafe {
            DeviceIoControl(
                h,
                FSCTL_DISMOUNT_VOLUME,
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok_dis != 0 {
            dismounted += 1;
        }
        unsafe { CloseHandle(h) };
    }
    Ok((locked, dismounted))
}

#[cfg(test)]
mod tests {
    use super::parse_extent_disks;

    fn extent_buffer(disks: &[u32]) -> Vec<u8> {
        let mut buf = vec![0u8; 8 + 24 * disks.len()];
        buf[0..4].copy_from_slice(&(disks.len() as u32).to_le_bytes());
        for (i, d) in disks.iter().enumerate() {
            let base = 8 + i * 24;
            buf[base..base + 4].copy_from_slice(&d.to_le_bytes());
        }
        buf
    }

    #[test]
    fn reads_disk_number_from_offset_eight() {
        // The old implementation read offset 0 (NumberOfDiskExtents) and only
        // ever matched disk 1. This is the regression test for that bug.
        let buf = extent_buffer(&[3]);
        assert_eq!(parse_extent_disks(&buf), vec![3]);
        assert!(!parse_extent_disks(&buf).contains(&1));
    }

    #[test]
    fn reads_multiple_extents() {
        let buf = extent_buffer(&[0, 2, 5]);
        assert_eq!(parse_extent_disks(&buf), vec![0, 2, 5]);
        assert!(parse_extent_disks(&buf).contains(&5));
    }

    #[test]
    fn ignores_truncated_buffers() {
        assert!(parse_extent_disks(&[]).is_empty());
        assert!(parse_extent_disks(&[1, 0, 0, 0]).is_empty());
    }
}
