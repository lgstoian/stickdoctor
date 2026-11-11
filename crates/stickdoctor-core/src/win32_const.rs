//! Constants not exposed (or not consistently exposed) by windows-sys.
//!
//! @sd file module="crates/stickdoctor-core/src/win32_const.rs" role="Win32 IOCTL and FSCTL constants" entry="FSCTL_LOCK_VOLUME" exit="PROPERTY_STANDARD_QUERY"
//! @sd anchor name=core.win32_const text="Win32 constants"

#![allow(dead_code)]
// Constants not exposed (or not consistently exposed) by windows-sys.

// ---- FSCTL codes (from winioctl.h) ----
pub const FSCTL_LOCK_VOLUME: u32 = 0x00090018;
pub const FSCTL_DISMOUNT_VOLUME: u32 = 0x00090020;
pub const FSCTL_UNLOCK_VOLUME: u32 = 0x0009001C;
pub const FSCTL_IS_VOLUME_MOUNTED: u32 = 0x00090028;

// ---- IOCTL codes ----
// CTL_CODE(IOCTL_VOLUME_BASE (0x56), 0, METHOD_BUFFERED, FILE_ANY_ACCESS).
// The old value (0x00056000) had the device type in the wrong nibble, so
// DeviceIoControl returned ERROR_NOT_SUPPORTED and no volume was ever matched.
pub const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;

// ---- Win32 error codes ----
pub const ERROR_WRITE_PROTECT: u32 = 19;
pub const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
pub const ERROR_MORE_DATA: u32 = 234;
pub const ERROR_NO_MORE_ITEMS: u32 = 259;

// ---- STORAGE_PROPERTY query IDs ----
pub const STORAGE_DEVICE_PROPERTY: u32 = 0;
pub const PROPERTY_STANDARD_QUERY: u32 = 0;

#[cfg(test)]
mod tests {
    #[test]
    fn control_codes_match_winioctl() {
        // CTL_CODE(0x56, 0, METHOD_BUFFERED, FILE_ANY_ACCESS) = 0x00560000.
        // A wrong value silently disables volume discovery.
        assert_eq!(super::IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, 0x0056_0000);
        assert_eq!(super::FSCTL_LOCK_VOLUME, 0x0009_0018);
        assert_eq!(super::FSCTL_DISMOUNT_VOLUME, 0x0009_0020);
        assert_eq!(super::FSCTL_UNLOCK_VOLUME, 0x0009_001C);
        assert_eq!(super::FSCTL_IS_VOLUME_MOUNTED, 0x0009_0028);
    }
}
