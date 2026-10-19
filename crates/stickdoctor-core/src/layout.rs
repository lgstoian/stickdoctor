use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

/// Bytes read from the start of the disk: sectors 0-16 plus the ISO9660 magic.
const SCAN_BYTES: usize = 0x8800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionStyle {
    None,
    Mbr,
    Gpt,
    IsoHybrid,
    TruncatedGpt,
    TruncatedMbr,
}

impl std::fmt::Display for PartitionStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartitionStyle::None => write!(f, "none / superfloppy"),
            PartitionStyle::Mbr => write!(f, "MBR"),
            PartitionStyle::Gpt => write!(f, "GPT"),
            PartitionStyle::IsoHybrid => write!(f, "isohybrid (MBR+ISO9660)"),
            PartitionStyle::TruncatedGpt => write!(f, "GPT (truncated / corrupt)"),
            PartitionStyle::TruncatedMbr => write!(f, "MBR (truncated / corrupt)"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionType {
    Windows(u8),
    Linux(u8),
    EfiSystem,
    Iso9660,
    Empty(u8),
    Unknown(u8),
}

impl PartitionType {
    pub fn describe(&self) -> &'static str {
        match self {
            PartitionType::Windows(t) => match *t {
                0x07 => "0x07 NTFS/FAT/exFAT",
                0x0B | 0x0C => "0x0B/0x0C FAT32",
                0x01 => "0x01 FAT12",
                0x04 | 0x06 => "0x04/0x06 FAT16",
                _ => "Windows-type",
            },
            PartitionType::Linux(t) => match *t {
                0x83 => "0x83 Linux native",
                0x82 => "0x82 Linux swap",
                0x85 => "0x85 Linux extended",
                0x8E => "0x8E LVM",
                _ => "Linux-type",
            },
            PartitionType::EfiSystem => "EFI System (0xEF)",
            PartitionType::Iso9660 => "ISO 9660",
            PartitionType::Empty(_) => "empty",
            PartitionType::Unknown(_) => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PartitionEntry {
    pub index: u32,
    pub bootable: bool,
    pub type_byte: u8,
    pub kind: PartitionType,
    pub start_lba: u64,
    pub size_lba: u64,
}

#[derive(Debug, Clone)]
pub struct SectorZeroInfo {
    pub style: PartitionStyle,
    pub entries: Vec<PartitionEntry>,
    pub has_mbr_signature: bool,
    pub has_gpt: bool,
    pub has_iso9660: bool,
}

/// Human-readable verdict: why Windows can't format this stick.
pub fn verdict(style: &PartitionStyle, entries: &[PartitionEntry]) -> Option<String> {
    match style {
        PartitionStyle::None => Some(
            "No partition table found. The stick is blank or uses a superfloppy layout. Repair will build a fresh MBR."
                .into(),
        ),
        PartitionStyle::TruncatedGpt | PartitionStyle::TruncatedMbr => Some(
            "The partition table points past the end of the disk (likely a truncated image write). Windows refuses to use it. Repair will wipe and rebuild."
                .into(),
        ),
        PartitionStyle::IsoHybrid => Some(
            "isohybrid layout (MBR partition plus ISO9660). Windows mounts nothing because there is no Windows filesystem. Repair will replace it with MBR + FAT."
                .into(),
        ),
        _ => {
            let has_linux = entries.iter().any(|e| matches!(e.kind, PartitionType::Linux(_) | PartitionType::EfiSystem));
            let has_windows = entries.iter().any(|e| matches!(e.kind, PartitionType::Windows(_)));
            if has_linux && !has_windows {
                Some(
                    "The partition table contains only Linux/EFI partitions with no Windows filesystem, so Windows cannot mount or format them. Repair will wipe and rebuild."
                        .into(),
                )
            } else {
                None
            }
        }
    }
}

/// Read the first sectors and classify the layout. Read-only.
pub fn inspect_sector_zero(index: u32) -> Option<SectorZeroInfo> {
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
        return None;
    }

    let mut buf = vec![0u8; SCAN_BYTES];
    let mut read: u32 = 0;
    let ok = unsafe {
        ReadFile(
            h,
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut read,
            ptr::null_mut(),
        )
    };
    unsafe { CloseHandle(h) };
    if ok == 0 || read < 512 {
        return None;
    }
    let read = read as usize;

    Some(analyze(&buf[..read]))
}

/// Pure analysis of a disk prefix. Separated out so it can be unit tested.
pub(crate) fn analyze(buf: &[u8]) -> SectorZeroInfo {
    let has_mbr_signature = buf.len() >= 512 && buf[510] == 0x55 && buf[511] == 0xAA;
    let has_gpt = buf.len() >= 520 && &buf[512..520] == b"EFI PART";
    let has_iso9660 = buf.len() >= 0x8006 && &buf[0x8001..0x8006] == b"CD001";

    let entries = if has_mbr_signature {
        parse_mbr_entries(buf)
    } else {
        Vec::new()
    };

    let style = if has_gpt {
        PartitionStyle::Gpt
    } else if has_mbr_signature && has_iso9660 {
        PartitionStyle::IsoHybrid
    } else if has_mbr_signature {
        PartitionStyle::Mbr
    } else {
        PartitionStyle::None
    };

    SectorZeroInfo {
        style,
        entries,
        has_mbr_signature,
        has_gpt,
        has_iso9660,
    }
}

/// Decode the four 16-byte MBR partition entries starting at offset 446.
pub(crate) fn parse_mbr_entries(buf: &[u8]) -> Vec<PartitionEntry> {
    let mut entries = Vec::new();
    if buf.len() < 512 {
        return entries;
    }
    for i in 0..4usize {
        let off = 446 + i * 16;
        let boot = buf[off] == 0x80;
        let type_byte = buf[off + 4];
        let start =
            u32::from_le_bytes([buf[off + 8], buf[off + 9], buf[off + 10], buf[off + 11]]) as u64;
        let size =
            u32::from_le_bytes([buf[off + 12], buf[off + 13], buf[off + 14], buf[off + 15]]) as u64;
        entries.push(PartitionEntry {
            index: i as u32,
            bootable: boot,
            type_byte,
            kind: classify(type_byte),
            start_lba: start,
            size_lba: size,
        });
    }
    entries
}

fn classify(type_byte: u8) -> PartitionType {
    match type_byte {
        0x00 => PartitionType::Empty(type_byte),
        0x07 | 0x0B | 0x0C | 0x01 | 0x04 | 0x06 | 0x0E | 0x0F => PartitionType::Windows(type_byte),
        0xEF => PartitionType::EfiSystem,
        0x83 | 0x82 | 0x85 | 0x8E | 0xDA => PartitionType::Linux(type_byte),
        _ => PartitionType::Unknown(type_byte),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mbr_with(entries: &[(u8, u32, u32)]) -> Vec<u8> {
        let mut buf = vec![0u8; SCAN_BYTES];
        buf[510] = 0x55;
        buf[511] = 0xAA;
        for (i, (kind, start, size)) in entries.iter().enumerate() {
            let off = 446 + i * 16;
            buf[off + 4] = *kind;
            buf[off + 8..off + 12].copy_from_slice(&start.to_le_bytes());
            buf[off + 12..off + 16].copy_from_slice(&size.to_le_bytes());
        }
        buf
    }

    #[test]
    fn detects_gpt_from_protective_header() {
        let mut buf = mbr_with(&[]);
        buf[512..520].copy_from_slice(b"EFI PART");
        let info = analyze(&buf);
        assert!(info.has_gpt);
        assert_eq!(info.style, PartitionStyle::Gpt);
    }

    #[test]
    fn parses_mbr_linux_partition() {
        let buf = mbr_with(&[(0x83, 2048, 100_000)]);
        let info = analyze(&buf);
        assert_eq!(info.style, PartitionStyle::Mbr);
        assert_eq!(info.entries.len(), 4);
        assert_eq!(info.entries[0].start_lba, 2048);
        assert_eq!(info.entries[0].size_lba, 100_000);
        assert!(matches!(info.entries[0].kind, PartitionType::Linux(0x83)));
    }

    #[test]
    fn detects_isohybrid() {
        let mut buf = mbr_with(&[(0x00, 0, 100_000)]);
        buf[0x8001..0x8006].copy_from_slice(b"CD001");
        let info = analyze(&buf);
        assert!(info.has_iso9660);
        assert_eq!(info.style, PartitionStyle::IsoHybrid);
    }

    #[test]
    fn blank_disk_is_none() {
        let info = analyze(&vec![0u8; SCAN_BYTES]);
        assert_eq!(info.style, PartitionStyle::None);
        assert!(info.entries.is_empty());
    }
}
