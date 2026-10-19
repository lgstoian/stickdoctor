#![allow(dead_code)]
// Constants not exposed by windows-sys, defined manually from the SDK headers.

// ---- FSCTL codes (from winioctl.h) ----
pub const FSCTL_LOCK_VOLUME: u32 = 0x00090018;
pub const FSCTL_DISMOUNT_VOLUME: u32 = 0x00090020;
pub const FSCTL_UNLOCK_VOLUME: u32 = 0x0009001C;
pub const FSCTL_IS_VOLUME_MOUNTED: u32 = 0x00090028;

// ---- IOCTL codes not always exported ----
pub const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x00056000;

// ---- GUID_DEVINTERFACE_DISK ----
// {4D1E55B2-6F11-11D0-BABF-00A0C9062910}
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

pub const GUID_DEVINTERFACE_DISK: Guid = Guid {
    data1: 0x4D1E55B2,
    data2: 0x6F11,
    data3: 0x11D0,
    data4: [0xBA, 0xBF, 0x00, 0xA0, 0xC9, 0x06, 0x29, 0x10],
};

// PARTITION_STYLE for IOCTL_DISK_CREATE_DISK
pub const PARTITION_STYLE_MBR: u32 = 0;
pub const PARTITION_STYLE_GPT: u32 = 1;
pub const PARTITION_STYLE_RAW: u32 = 2;

// Win32 error codes
pub const ERROR_WRITE_PROTECT: u32 = 19;
pub const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
pub const ERROR_NO_MORE_ITEMS: u32 = 259;

// STORAGE_PROPERTY query IDs
pub const STORAGE_DEVICE_PROPERTY: u32 = 0;
pub const PROPERTY_STANDARD_QUERY: u32 = 0;
