use crate::error::Result;
use std::ffi::OsString;
use std::ptr;
use windows_sys::Win32::Foundation::{GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

#[derive(Debug, Clone)]
pub struct DiskDevice {
    pub device_path: OsString,
    pub instance_id: OsString,
    pub drive_index: Option<u32>,
    pub friendly_name: Option<OsString>,
}

/// Enumerate physical drives by probing \\.\PhysicalDrive0..=63.
pub fn enumerate_disks() -> Result<Vec<DiskDevice>> {
    let mut devices = Vec::new();

    for index in 0..64u32 {
        let path = format!(r"\\.\PhysicalDrive{index}");
        let path_w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let h = unsafe {
            CreateFileW(
                path_w.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if h != INVALID_HANDLE_VALUE {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(h) };
            devices.push(DiskDevice {
                device_path: OsString::from(&path),
                instance_id: OsString::from(format!("PhysicalDrive{index}")),
                drive_index: Some(index),
                friendly_name: None,
            });
        }
    }

    Ok(devices)
}

#[allow(dead_code)]
pub fn open_physical_drive(index: u32) -> Result<HANDLE> {
    let path = format!(r"\\.\PhysicalDrive{index}");
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
    Ok(h)
}
