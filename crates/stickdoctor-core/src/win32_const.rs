#![allow(dead_code)]
// Constants not exposed (or not consistently exposed) by windows-sys.

// ---- FSCTL codes (from winioctl.h) ----
pub const FSCTL_LOCK_VOLUME: u32 = 0x00090018;
pub const FSCTL_DISMOUNT_VOLUME: u32 = 0x00090020;
pub const FSCTL_UNLOCK_VOLUME: u32 = 0x0009001C;
pub const FSCTL_IS_VOLUME_MOUNTED: u32 = 0x00090028;

// ---- IOCTL codes ----
pub const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x00056000;

// ---- Win32 error codes ----
pub const ERROR_WRITE_PROTECT: u32 = 19;
pub const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
pub const ERROR_NO_MORE_ITEMS: u32 = 259;

// ---- STORAGE_PROPERTY query IDs ----
pub const STORAGE_DEVICE_PROPERTY: u32 = 0;
pub const PROPERTY_STANDARD_QUERY: u32 = 0;
