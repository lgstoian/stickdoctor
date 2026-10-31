# StickDoctor agent breadcrumb map

Generated from `@sd` annotations (schema `1`).
Regenerate with `cargo run -p stickdoctor-cli -- agent-map --write`.

## Modules

| Module | Role | Entry | Exit |
|---|---|---|---|
| `crates/stickdoctor-agent/src/lib.rs` | parses and validates @sd breadcrumbs | scan | validate |
| `crates/stickdoctor-app/build.rs` | build script for resource embedding | main | main |
| `crates/stickdoctor-app/src/main.rs` | native Win32 GUI | main | build_ui |
| `crates/stickdoctor-cli/src/main.rs` | CLI front end | main | cmd_repair |
| `crates/stickdoctor-core/src/enumerate.rs` | physical drive enumeration | enumerate_disks | enumerate_disks |
| `crates/stickdoctor-core/src/error.rs` | error mapping | last_error | win32_error_message |
| `crates/stickdoctor-core/src/format.rs` | format and verification | format_volume | verify_boot_sector |
| `crates/stickdoctor-core/src/layout.rs` | partition table detection | inspect_sector_zero | analyze |
| `crates/stickdoctor-core/src/lib.rs` | public API and DiskCandidate | DiskCandidate::probe | details_text |
| `crates/stickdoctor-core/src/log.rs` | structured event log | Logger::log | Logger::render |
| `crates/stickdoctor-core/src/props.rs` | disk properties and media classification | inspect_properties | inspect_properties |
| `crates/stickdoctor-core/src/repair.rs` | repair orchestration and verification | execute_repair_with_progress | VerificationOutcome::is_success |
| `crates/stickdoctor-core/src/safety.rs` | safety gate | safety_check | safety_check |
| `crates/stickdoctor-core/src/tools.rs` | diagnostic and repair tools | scan_lost_partitions | full_overwrite |
| `crates/stickdoctor-core/src/volumes.rs` | volume mapping and unmount | disk_volumes | lock_dismount_all |
| `crates/stickdoctor-core/src/win32_const.rs` | Win32 IOCTL and FSCTL constants | FSCTL_LOCK_VOLUME | PROPERTY_STANDARD_QUERY |

## Flow: `core`

| Step | Stage | Verifies |
|---|---|---|
| 1 | enumerate physical drives (`core.enumerate`) |  |
| 2 | probe one disk (`core.probe`) |  |
| 3 | diagnose layout and volumes (`core.diagnose`) |  |
| 4 | repair (`core.repair`) | repair-success |

## Flow: `format`

| Step | Stage | Verifies |
|---|---|---|
| 1 | fmifs FormatEx (`format.fmifs`) |  |
| 2 | diskpart format (`format.diskpart`) |  |
| 3 | format.com (`format.formatcom`) |  |

## Flow: `repair`

| Step | Stage | Verifies |
|---|---|---|
| 1 | Safety gate (`repair.safety`) |  |
| 2 | Unmount volumes (`repair.unmount`) |  |
| 3 | Optional raw wipe (`repair.zero`) |  |
| 4 | Delete layout (`repair.delete`) |  |
| 5 | Create partition and format (`repair.create`) |  |
| 6 | Wait for volume (`repair.wait`) |  |
| 7 | Verify filesystem (`repair.verify`) | repair-success |
| 8 | Report (`repair.report`) |  |

## Invariants

| ID | Invariant | Guard test |
|---|---|---|
| INV-1 | Never touch the disk that hosts Windows | `classify_safety_refuses_system_disk` |
| INV-2 | Only removable media may be repaired | `classify_safety_refuses_non_usb` |
| INV-3 | Disk-modifying tools run only on removable media | `require_safe_refuses_non_safe_statuses` |

## Verification

| ID | Function | Proves | Negative test |
|---|---|---|---|
| boot-sector-signature | `boot_sector_matches` | the partition boot sector holds the requested filesystem signature | `boot_sector_rejects_unknown_fs_name` |
| repair-success | `is_success` | a repair succeeded only if the filesystem was read back and matched | `verification_rejects_boot_sector_failure` |
| partition-usable | `layout_is_usable_mbr` | a created partition is a usable MBR partition | `layout_is_usable_mbr_rejects_gpt` |
| capacity-test | `capacity_offsets` | counterfeit-capacity offsets are distinct, aligned and in bounds | `capacity_offsets_are_distinct_and_in_bounds` |

## Hazards

| Hazard | Guard |
|---|---|
| performs irreversible writes | self-elevation, can_repair and a confirmation dialog |
| performs irreversible writes to a raw disk | safety_check, self-elevation and user confirmation |
| writes test patterns to the media | safety gate (removable media only) and explicit user confirmation |

Counts: 17 modules, 66 annotations, 62 tests.
