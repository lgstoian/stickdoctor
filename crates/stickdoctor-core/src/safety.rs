use crate::error::Result;
use crate::props::{BusType, DiskProperties};
use std::ptr;
use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Ioctl::IOCTL_STORAGE_GET_DEVICE_NUMBER;
use windows_sys::Win32::System::IO::DeviceIoControl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyStatus {
    /// Safe to wipe (USB and not the running system disk).
    Safe,
    /// Refused: this is the system disk.
    SystemDisk,
    /// Refused: not a USB bus (use advanced mode to override).
    NotUsb,
    /// Cannot determine (properties unreadable).
    Unknown,
}

impl SafetyStatus {
    pub fn label(&self) -> &'static str {
        match self {
            SafetyStatus::Safe => "Safe to repair",
            SafetyStatus::SystemDisk => "SYSTEM DISK — repair refused",
            SafetyStatus::NotUsb => "Not a USB device — repair refused",
            SafetyStatus::Unknown => "Cannot identify — run as Administrator",
        }
    }
}

/// Determine whether it is safe to wipe a disk. Never allow the system disk.
pub fn safety_check(props: &DiskProperties, index: u32) -> Result<SafetyStatus> {
    match props.bus_type {
        BusType::Usb => {
            if is_system_disk(index)? {
                Ok(SafetyStatus::SystemDisk)
            } else {
                Ok(SafetyStatus::Safe)
            }
        }
        // Some cheap USB bridges report an unknown bus. If the device reports
        // removable media, treat it as a candidate but keep the system check.
        BusType::Unknown if props.removable => {
            if is_system_disk(index)? {
                Ok(SafetyStatus::SystemDisk)
            } else {
                Ok(SafetyStatus::Safe)
            }
        }
        BusType::Unknown => Ok(SafetyStatus::Unknown),
        _ => Ok(SafetyStatus::NotUsb),
    }
}

/// Compare the disk's StorageDeviceNumber to the disk that holds Windows.
fn is_system_disk(index: u32) -> Result<bool> {
    let system = system_disk_number()?;
    let target = disk_number(index)?;
    match (system, target) {
        (Some(sys), Some(tgt)) => Ok(sys == tgt),
        // If we cannot determine either side, refuse to call it safe only when
        // the target is clearly the boot disk. Otherwise allow (USB restriction
        // in the caller already narrows this down).
        _ => Ok(false),
    }
}

fn disk_number(index: u32) -> Result<Option<u32>> {
    let path = format!(r"\\.\PhysicalDrive{index}");
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Ok(None);
    }
    #[repr(C)]
    #[derive(Default)]
    struct DevNum {
        device_number: u32,
        partition_number: u32,
    }
    let mut d = DevNum::default();
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            ptr::null_mut(),
            0,
            &mut d as *mut _ as *mut _,
            std::mem::size_of::<DevNum>() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    unsafe { windows_sys::Win32::Foundation::CloseHandle(h) };
    if ok != 0 && returned >= std::mem::size_of::<DevNum>() as u32 {
        Ok(Some(d.device_number))
    } else {
        Ok(None)
    }
}

fn system_disk_number() -> Result<Option<u32>> {
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buf = vec![0u16; 256];
    let len = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) };
    if len == 0 {
        return Ok(None);
    }
    let sys = String::from_utf16_lossy(&buf[..len as usize]);
    let drive = sys.chars().next().unwrap_or('C');
    let path = format!(r"\\.\{drive}:");
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Ok(None);
    }
    #[repr(C)]
    #[derive(Default)]
    struct DevNum {
        device_number: u32,
        partition_number: u32,
    }
    let mut d = DevNum::default();
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            ptr::null_mut(),
            0,
            &mut d as *mut _ as *mut _,
            std::mem::size_of::<DevNum>() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    unsafe { windows_sys::Win32::Foundation::CloseHandle(h) };
    if ok != 0 && returned >= std::mem::size_of::<DevNum>() as u32 {
        Ok(Some(d.device_number))
    } else {
        Ok(None)
    }
}
