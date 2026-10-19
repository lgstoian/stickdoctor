mod enumerate;
mod error;
mod format;
pub mod layout;
mod log;
mod props;
mod repair;
mod safety;
mod volumes;
mod win32_const;

pub use enumerate::{enumerate_disks, DiskDevice};
pub use error::{Error, Result};
pub use format::{format_volume, verify_volume, volume_root};
pub use layout::{inspect_sector_zero, PartitionEntry, PartitionStyle, SectorZeroInfo};
pub use log::{LogEvent, LogLevel};
pub use props::{inspect_properties, BusType, DiskProperties};
pub use repair::{
    execute_repair, execute_repair_with_progress, FsChoice, RepairOptions, StageCallback,
};
pub use safety::{safety_check, SafetyStatus};
pub use volumes::{disk_volumes, lock_dismount_all, wait_for_volume, VolumeInfo};

use std::sync::{Arc, Mutex};

/// Shared logger so CLI and GUI both consume the same structured events.
pub type SharedLog = Arc<Mutex<log::Logger>>;

pub fn new_logger() -> SharedLog {
    Arc::new(Mutex::new(log::Logger::new()))
}

/// One candidate device presented in the diagnostics panel.
#[derive(Debug, Clone)]
pub struct DiskCandidate {
    pub device: DiskDevice,
    pub properties: DiskProperties,
    pub sector_zero: Option<SectorZeroInfo>,
    pub volumes: Vec<VolumeInfo>,
    pub safety: SafetyStatus,
}

impl DiskCandidate {
    /// Build a candidate by querying the system for one enumerated device.
    pub fn probe(device: &DiskDevice) -> Self {
        let index = device.drive_index;
        let properties = index
            .and_then(|i| inspect_properties(i).ok())
            .unwrap_or(DiskProperties {
                bus_type: BusType::Unknown,
                removable: false,
                writable: false,
                size_bytes: 0,
                device_number: None,
                model: None,
                serial: None,
            });
        let sector_zero = index.and_then(inspect_sector_zero);
        let volumes = index
            .map(|i| disk_volumes(i).unwrap_or_default())
            .unwrap_or_default();
        let safety = index
            .map(|i| safety_check(&properties, i).unwrap_or(SafetyStatus::Unknown))
            .unwrap_or(SafetyStatus::Unknown);
        Self {
            device: device.clone(),
            properties,
            sector_zero,
            volumes,
            safety,
        }
    }

    /// Human-readable one-line description for the device list.
    pub fn display_line(&self) -> String {
        let index = self
            .device
            .drive_index
            .map(|n| n.to_string())
            .unwrap_or_else(|| "?".into());
        let model = self
            .properties
            .model
            .clone()
            .unwrap_or_else(|| "Unknown disk".into());
        let bus = self.properties.bus_type;
        let size = self.properties.size_gib();
        let flag = match self.safety {
            SafetyStatus::Safe => "",
            SafetyStatus::SystemDisk => "  [SYSTEM - DO NOT WIPE]",
            SafetyStatus::NotUsb => "  [not USB]",
            SafetyStatus::Unknown => "  [unknown]",
        };
        format!(
            "Disk {} | {} | {:.1} GiB | {}{}",
            index, bus, size, model, flag
        )
    }

    /// Multi-line diagnostic summary for the details pane.
    pub fn details_text(&self) -> String {
        let mut out = String::new();
        let p = &self.properties;
        out.push_str(&format!("Bus type: {}\n", p.bus_type));
        out.push_str(&format!("Size: {:.2} GiB\n", p.size_gib()));
        out.push_str(&format!(
            "Removable: {}\n",
            if p.removable { "yes" } else { "no" }
        ));
        out.push_str(&format!(
            "Writable: {}\n",
            if p.writable {
                "yes"
            } else {
                "NO (write-protected)"
            }
        ));
        if let Some(ref m) = p.model {
            out.push_str(&format!("Model: {}\n", m));
        }
        if let Some(ref s) = p.serial {
            out.push_str(&format!("Serial: {}\n", s));
        }

        if let Some(ref sz) = self.sector_zero {
            out.push_str(&format!("Partition style: {}\n", sz.style));
            let parts: Vec<String> = sz
                .entries
                .iter()
                .filter(|e| !matches!(e.kind, layout::PartitionType::Empty(_)))
                .map(|e| {
                    format!(
                        "#{} {} start={} size={} MiB",
                        e.index,
                        e.kind.describe(),
                        e.start_lba,
                        e.size_lba * 512 / (1024 * 1024)
                    )
                })
                .collect();
            if !parts.is_empty() {
                out.push_str("Partitions:\n");
                for part in parts {
                    out.push_str(&format!("  {}\n", part));
                }
            }
            if let Some(v) = layout::verdict(&sz.style, &sz.entries) {
                out.push_str(&format!("\nDiagnosis: {}\n", v));
            }
        } else {
            out.push_str("Partition style: (could not read - need Administrator)\n");
        }

        if self.volumes.is_empty() {
            out.push_str("\nVolumes: none visible to Windows\n");
        } else {
            out.push_str("\nVolumes:\n");
            for v in &self.volumes {
                let fs = v.filesystem.as_deref().unwrap_or("RAW");
                let letter = v.drive_letter.as_deref().unwrap_or("(no letter)");
                out.push_str(&format!("  {}  fs={}\n", letter, fs));
            }
        }
        out
    }

    /// The UI must only allow what `execute_repair` will actually perform.
    pub fn can_repair(&self) -> bool {
        matches!(self.safety, SafetyStatus::Safe)
    }
}
