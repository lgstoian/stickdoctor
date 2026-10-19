use crate::error::{last_error, Result};
use std::mem;
use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Ioctl::{
    IOCTL_DISK_GET_LENGTH_INFO, IOCTL_DISK_IS_WRITABLE, IOCTL_STORAGE_GET_DEVICE_NUMBER,
    IOCTL_STORAGE_QUERY_PROPERTY,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusType {
    Unknown,
    Usb,
    Sata,
    Nvme,
    Scsi,
    Atapi,
    Raid,
    Sd,
    Mmc,
}

impl std::fmt::Display for BusType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let s = match self {
            BusType::Usb => "USB",
            BusType::Sata => "SATA",
            BusType::Nvme => "NVMe",
            BusType::Scsi => "SCSI",
            BusType::Atapi => "ATAPI",
            BusType::Raid => "RAID",
            BusType::Sd => "SD",
            BusType::Mmc => "MMC",
            _ => "unknown",
        };
        write!(f, "{}", s)
    }
}

fn bus_type_from_raw(raw: u32) -> BusType {
    match raw {
        7 => BusType::Usb,
        11 => BusType::Sata,
        17 => BusType::Nvme,
        1 => BusType::Scsi,
        2 => BusType::Atapi,
        8 => BusType::Raid,
        15 => BusType::Sd,
        14 => BusType::Mmc,
        _ => BusType::Unknown,
    }
}

#[repr(C)]
struct StoragePropertyQuery {
    property_id: u32,
    query_type: u32,
    additional: [u8; 1],
}

#[repr(C)]
#[derive(Default)]
struct StorageDeviceDescriptor {
    version: u32,
    size: u32,
    device_type: u8,
    device_type_modifier: u8,
    removable_media: u8,
    command_queueing: u8,
    vendor_id_offset: u32,
    product_id_offset: u32,
    product_revision_offset: u32,
    serial_number_offset: u32,
    bus_type: u32,
    raw_properties_length: u32,
    // raw_properties follows
}

#[repr(C)]
#[derive(Default)]
struct DiskLengthInfo {
    length: i64,
}

#[repr(C)]
#[derive(Default)]
struct StorageDeviceNumber {
    device_number: u32,
    partition_number: u32,
}

#[derive(Debug, Clone)]
pub struct DiskProperties {
    pub bus_type: BusType,
    pub removable: bool,
    pub writable: bool,
    pub size_bytes: u64,
    pub device_number: Option<u32>,
    pub model: Option<String>,
    pub serial: Option<String>,
}

impl DiskProperties {
    pub fn size_gb(&self) -> f64 {
        self.size_bytes as f64 / 1_000_000_000.0
    }
    pub fn size_gib(&self) -> f64 {
        self.size_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
    }

    /// One-line summary for the device list.
    pub fn summary(&self) -> String {
        let model = self.model.clone().unwrap_or_else(|| "Unknown".to_string());
        format!("{}  [{:.1} GiB]", model, self.size_gib())
    }

    pub fn is_usb(&self) -> bool {
        self.bus_type == BusType::Usb
    }
}

impl std::fmt::Display for DiskProperties {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{} {:.2} GiB {} {}",
            self.bus_type,
            self.size_gib(),
            if self.writable {
                "writable"
            } else {
                "READ-ONLY"
            },
            if self.removable { "removable" } else { "fixed" }
        )
    }
}

fn read_cstr(buf: &[u8], offset: u32) -> Option<String> {
    if offset == 0 || offset as usize >= buf.len() {
        return None;
    }
    let start = offset as usize;
    let end = buf[start..]
        .iter()
        .position(|&b| b == 0)
        .map(|p| start + p)
        .unwrap_or(buf.len());
    let s = String::from_utf8_lossy(&buf[start..end]).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

pub fn inspect_properties(index: u32) -> Result<DiskProperties> {
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
        return Err(last_error());
    }

    let query = StoragePropertyQuery {
        property_id: 0,
        query_type: 0,
        additional: [0],
    };
    let mut buf = vec![0u8; 1024];
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &query as *const _ as *const std::ffi::c_void,
            mem::size_of::<StoragePropertyQuery>() as u32,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            buf.len() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };

    let (bus, removable, model, serial) =
        if ok != 0 && returned as usize >= mem::size_of::<StorageDeviceDescriptor>() {
            let d = unsafe { &*(buf.as_ptr() as *const StorageDeviceDescriptor) };
            let model = read_cstr(&buf, d.product_id_offset)
                .or_else(|| read_cstr(&buf, d.vendor_id_offset));
            let serial = read_cstr(&buf, d.serial_number_offset);
            (
                bus_type_from_raw(d.bus_type),
                d.removable_media != 0,
                model,
                serial,
            )
        } else {
            (BusType::Unknown, false, None, None)
        };

    let writable = query_writable(h);
    let size = query_length(h).unwrap_or_default();
    let device_number = query_device_number(h);
    unsafe { CloseHandle(h) };

    Ok(DiskProperties {
        bus_type: bus,
        removable,
        writable,
        size_bytes: size,
        device_number,
        model,
        serial,
    })
}

fn query_writable(h: HANDLE) -> bool {
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_DISK_IS_WRITABLE,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    };
    ok != 0
}

fn query_length(h: HANDLE) -> Option<u64> {
    let mut info = DiskLengthInfo::default();
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_DISK_GET_LENGTH_INFO,
            ptr::null_mut(),
            0,
            &mut info as *mut _ as *mut _,
            mem::size_of::<DiskLengthInfo>() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok != 0 && returned >= mem::size_of::<DiskLengthInfo>() as u32 {
        Some(info.length as u64)
    } else {
        None
    }
}

fn query_device_number(h: HANDLE) -> Option<u32> {
    let mut d = StorageDeviceNumber::default();
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            ptr::null_mut(),
            0,
            &mut d as *mut _ as *mut _,
            mem::size_of::<StorageDeviceNumber>() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok != 0 && returned >= mem::size_of::<StorageDeviceNumber>() as u32 {
        Some(d.device_number)
    } else {
        None
    }
}

#[allow(dead_code)]
pub fn read_only_sources() -> Vec<String> {
    let mut sources = Vec::new();
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD,
    };
    let key_path: Vec<u16> = r"SYSTEM\CurrentControlSet\Control\StorageDevicePolicies"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let wp_name: Vec<u16> = "WriteProtect"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut hkey: HKEY = ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            key_path.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        )
    };
    if status == 0 {
        let mut value: u32 = 0;
        let mut size: u32 = mem::size_of::<u32>() as u32;
        let mut kind: u32 = 0;
        let v = unsafe {
            RegQueryValueExW(
                hkey,
                wp_name.as_ptr(),
                ptr::null_mut(),
                &mut kind,
                &mut value as *mut _ as *mut u8,
                &mut size,
            )
        };
        if v == 0 && kind == REG_DWORD && value != 0 {
            sources.push("HKLM\\...\\StorageDevicePolicies\\WriteProtect = 1".into());
        }
        unsafe { RegCloseKey(hkey) };
    }
    sources
}
