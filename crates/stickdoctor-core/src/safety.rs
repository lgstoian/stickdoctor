//! System-disk detection and the removable-only policy.
//!
//! @sd file module="crates/stickdoctor-core/src/safety.rs" role="safety gate" entry="safety_check" exit="safety_check"
//! @sd anchor name=core.safety text="safety gate"
//! @sd invariant id=INV-1 text="Never touch the disk that hosts Windows" test=classify_safety_refuses_system_disk
//! @sd invariant id=INV-2 text="Only removable media may be repaired" test=classify_safety_refuses_non_usb

use crate::error::{Error, Result};
use crate::props::{BusType, DiskProperties};
use std::ptr;
use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Ioctl::{IOCTL_STORAGE_GET_DEVICE_NUMBER, STORAGE_DEVICE_NUMBER};
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
            SafetyStatus::Safe => "safe",
            SafetyStatus::SystemDisk => "SYSTEM",
            SafetyStatus::NotUsb => "not USB",
            SafetyStatus::Unknown => "unknown",
        }
    }

    /// Longer explanation, for the details pane.
    pub fn long_label(&self) -> &'static str {
        match self {
            SafetyStatus::Safe => "Safe to repair",
            SafetyStatus::SystemDisk => "SYSTEM DISK — repair refused",
            SafetyStatus::NotUsb => "Not removable media — repair refused",
            SafetyStatus::Unknown => "Cannot identify — run as Administrator",
        }
    }
}

/// Determine whether it is safe to wipe a disk. Never allow the system disk.
pub fn safety_check(props: &DiskProperties, index: u32) -> Result<SafetyStatus> {
    Ok(classify_safety(
        props.bus_type,
        props.removable,
        is_system_disk(index)?,
    ))
}

/// Refuse a disk-modifying tool unless the disk is removable media and is not
/// the disk that hosts Windows.
///
/// The GUI and CLI call this as well, but the core tools call it internally so
/// the invariant holds regardless of the entry point.
pub fn ensure_safe_for_tool(index: u32) -> Result<()> {
    let props = crate::props::inspect_properties(index)?;
    let status = classify_safety(props.bus_type, props.removable, is_system_disk(index)?);
    require_safe(status, index)
}

/// The refusal half of [`ensure_safe_for_tool`], split out so it can be tested
/// without a real disk.
pub(crate) fn require_safe(status: SafetyStatus, index: u32) -> Result<()> {
    match status {
        SafetyStatus::Safe => Ok(()),
        other => Err(Error::Refused(format!(
            "Refusing tool on PhysicalDrive{}: {}",
            index,
            other.long_label()
        ))),
    }
}

/// Pure safety decision given the bus, the removable bit and whether the target
/// is the disk that hosts Windows.
///
/// Extracted so the policy can be tested without a real disk: the invariant is
/// that the system disk is refused even when it reports a removable bit.
pub(crate) fn classify_safety(bus: BusType, removable: bool, is_system: bool) -> SafetyStatus {
    let candidate = |is_system: bool| {
        if is_system {
            SafetyStatus::SystemDisk
        } else {
            SafetyStatus::Safe
        }
    };
    match bus {
        BusType::Usb => candidate(is_system),
        // Some cheap USB bridges report an unknown bus, and a native SD/MMC slot
        // reports Sd/Mmc. If the device reports removable media, treat it as a
        // candidate but keep the system check.
        BusType::Unknown | BusType::Sd | BusType::Mmc if removable => candidate(is_system),
        BusType::Unknown => SafetyStatus::Unknown,
        BusType::Sd | BusType::Mmc => SafetyStatus::NotUsb,
        _ => SafetyStatus::NotUsb,
    }
}

/// The drive letter that hosts Windows, e.g. `C:`. Used to keep the `chkdsk`
/// tool off the boot volume.
pub fn system_drive_letter() -> Option<String> {
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buf = vec![0u16; 256];
    let len = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) };
    if len == 0 {
        return None;
    }
    let sys = String::from_utf16_lossy(&buf[..len as usize]);
    let drive = sys.chars().next()?;
    Some(format!("{}:", drive.to_ascii_uppercase()))
}

/// Compare the disk's StorageDeviceNumber to the disk that holds Windows.
fn is_system_disk(index: u32) -> Result<bool> {
    let system = system_disk_number()?;
    let target = disk_number(index)?;
    match (system, target) {
        (Some(sys), Some(tgt)) => Ok(sys == tgt),
        // Fail closed: if either side cannot be determined, refusing is safer
        // than assuming the disk is not the one hosting Windows.
        _ => Err(Error::Internal(
            "cannot determine the Windows system disk number; refusing".into(),
        )),
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
    // STORAGE_DEVICE_NUMBER is { DWORD DeviceType; DWORD DeviceNumber;
    // DWORD PartitionNumber; } — 12 bytes, with DeviceNumber at offset 4.
    let mut d: STORAGE_DEVICE_NUMBER = unsafe { std::mem::zeroed() };
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            ptr::null_mut(),
            0,
            &mut d as *mut _ as *mut _,
            std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    unsafe { windows_sys::Win32::Foundation::CloseHandle(h) };
    if ok != 0 && returned >= std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32 {
        Ok(Some(d.DeviceNumber))
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
    // STORAGE_DEVICE_NUMBER is { DWORD DeviceType; DWORD DeviceNumber;
    // DWORD PartitionNumber; } — 12 bytes, with DeviceNumber at offset 4.
    let mut d: STORAGE_DEVICE_NUMBER = unsafe { std::mem::zeroed() };
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            ptr::null_mut(),
            0,
            &mut d as *mut _ as *mut _,
            std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    unsafe { windows_sys::Win32::Foundation::CloseHandle(h) };
    if ok != 0 && returned >= std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32 {
        Ok(Some(d.DeviceNumber))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::props::BusType;

    #[test]
    fn non_usb_is_refused() {
        assert_eq!(
            classify_safety(BusType::Nvme, false, false),
            SafetyStatus::NotUsb
        );
    }

    #[test]
    fn unknown_fixed_is_unknown() {
        assert_eq!(
            classify_safety(BusType::Unknown, false, false),
            SafetyStatus::Unknown
        );
    }

    #[test]
    fn classify_safety_refuses_system_disk() {
        // INV-1: the system disk is refused even when it looks like removable USB.
        assert_eq!(
            classify_safety(BusType::Usb, true, true),
            SafetyStatus::SystemDisk
        );
        assert_eq!(
            classify_safety(BusType::Unknown, true, true),
            SafetyStatus::SystemDisk
        );
    }

    #[test]
    fn classify_safety_refuses_non_usb() {
        // INV-2: non-USB, non-removable media is refused.
        assert_eq!(
            classify_safety(BusType::Nvme, false, false),
            SafetyStatus::NotUsb
        );
        assert_eq!(
            classify_safety(BusType::Sata, false, false),
            SafetyStatus::NotUsb
        );
    }

    #[test]
    fn classify_safety_accepts_removable_usb() {
        assert_eq!(
            classify_safety(BusType::Usb, true, false),
            SafetyStatus::Safe
        );
        // Some cheap USB bridges report an unknown bus but set the removable bit.
        assert_eq!(
            classify_safety(BusType::Unknown, true, false),
            SafetyStatus::Safe
        );
        assert_eq!(
            classify_safety(BusType::Unknown, false, false),
            SafetyStatus::Unknown
        );
    }

    #[test]
    fn classify_safety_accepts_removable_sd_mmc() {
        // A card in a native SD/MMC slot reports Sd/Mmc and is removable.
        assert_eq!(
            classify_safety(BusType::Sd, true, false),
            SafetyStatus::Safe
        );
        assert_eq!(
            classify_safety(BusType::Mmc, true, false),
            SafetyStatus::Safe
        );
        // The system-disk check still applies.
        assert_eq!(
            classify_safety(BusType::Sd, true, true),
            SafetyStatus::SystemDisk
        );
    }

    #[test]
    fn classify_safety_refuses_fixed_sd_mmc() {
        assert_eq!(
            classify_safety(BusType::Sd, false, false),
            SafetyStatus::NotUsb
        );
        assert_eq!(
            classify_safety(BusType::Mmc, false, false),
            SafetyStatus::NotUsb
        );
    }

    #[test]
    fn require_safe_refuses_non_safe_statuses() {
        assert!(require_safe(SafetyStatus::Safe, 3).is_ok());
        assert!(require_safe(SafetyStatus::SystemDisk, 3).is_err());
        assert!(require_safe(SafetyStatus::NotUsb, 3).is_err());
        assert!(require_safe(SafetyStatus::Unknown, 3).is_err());
    }
}
