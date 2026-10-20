# AGENTS.md

Guidance for AI coding agents working on **StickDoctor**.

> Read this before changing disk I/O, the repair pipeline, or the GUI layout.

---

## What this is

A native Windows GUI + CLI that recovers removable media (USB sticks, SD cards,
CF cards) that Windows can no longer format or see after a raw image has been
written with `dd`, balenaEtcher, Rufus, etc.

The symptom set it targets:

- Explorer shows nothing for the stick.
- Disk Management lists it as *Removable* but **Format** is greyed out.
- Windows keeps prompting *"You need to format the disk before you can use it."*
- `diskpart clean` works manually but is easy to run on the wrong disk.

## Repository layout

```
Cargo.toml                  workspace root
crates/
  stickdoctor-core/         all disk logic — NO UI dependencies
    src/lib.rs              public API + DiskCandidate
    src/enumerate.rs        \\.\PhysicalDrive0..63 probing
    src/props.rs            bus type, size, model, serial, writable, MediaType
    src/layout.rs           MBR / GPT / isohybrid / blank detection
    src/volumes.rs          volume <-> disk mapping, lock/dismount, drive letters
    src/safety.rs           system-disk detection, removable-only policy
    src/format.rs           fmifs FormatEx, diskpart, format.com fallbacks
    src/repair.rs           the repair pipeline (orchestration + verification)
    src/tools.rs            lost-partition scan, write-protect clear, chkdsk, capacity test
    src/log.rs              structured event log
    src/error.rs            Error type + Win32 message mapping
  stickdoctor-cli/          command line front end
  stickdoctor-app/          native Win32 GUI (native-windows-gui)
    assets/                 icon, manifest, .rc
    build.rs                embeds resources when rc.exe / windres exist
docs/                       architecture, security, safety, troubleshooting
.github/                    CI + release workflows, issue templates
```

## Build & test

```powershell
cargo build                 # debug
cargo build --release       # release (embeds icon + manifest)
cargo test                  # unit tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
cargo audit                 # RustSec advisories (needs `cargo install cargo-audit`)
cargo deny check            # license / ban / source policy
```

Requirements: Rust stable, and **Visual Studio Build Tools** with *Desktop
development with C++* for the MSVC linker and `rc.exe`. Without the resource
compiler the app still builds — it just has no embedded icon/version/manifest
(`build.rs` degrades gracefully).

## Architecture

The GUI never touches a disk directly. Everything goes through
`stickdoctor-core`, which is UI-agnostic and unit-testable.

```
enumerate -> probe (props + layout + volumes + safety) -> diagnose -> repair
```

`DiskCandidate` bundles everything the UI needs about one disk:
properties, sector-zero layout, volumes, and a `SafetyStatus`.

## The repair pipeline

`execute_repair_with_progress()` in `repair.rs` runs these stages. **Every
stage after the safety gate is verified before the next one runs.**

1. **Safety gate** — read properties, refuse non-removable media and the disk
   that hosts Windows.
2. **Unmount** — `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` to find volumes, then
   `FSCTL_LOCK_VOLUME` + `FSCTL_DISMOUNT_VOLUME`.
3. **Optional raw wipe** — zero first/last 1 MiB (kills stale GPT backup
   headers and isohybrid residues). Non-fatal on failure.
4. **Delete layout** — `IOCTL_DISK_DELETE_DRIVE_LAYOUT`, best-effort.
5. **Create partition + format** — `diskpart` in ONE session:
   `select disk N` → `online disk` → `attributes disk clear readonly` →
   `clean` → `create partition primary` → `format fs=... label=... quick` →
   `assign` → `rescan`.
6. **Wait for volume** — extent-based, then GUID-diff, then drive-letter-diff.
7. **Verify** — `verify_boot_sector()` reads LBA 2048 and checks the FS
   signature (`FAT32   `, `EXFAT   `, `NTFS    `). If a volume appears, also
   `GetVolumeInformationW` and a write/read-back test.
8. **Report** — success is defined by the **boot-sector check**, not by a tool's
   exit code.

### Why `clean` and `create partition primary` must be in the same diskpart session

`IOCTL_DISK_DELETE_DRIVE_LAYOUT` invalidates the on-disk table but does **not**
update DiskPart/VDS's cached view. Running `create partition primary` in a
*separate* `diskpart` process then fails with
*"No usable free extent could be found."* Always `clean` and `create` in one
session.

### Why success is the boot-sector check

`diskpart` exits 0 even when a command fails, and on many USB bridges Windows
**never registers a volume** for a freshly created partition. Waiting for a
volume to appear makes the tool fail on media that is actually fine. Reading
the boot sector proves the format landed.

## Safety invariants — do not break these

1. **Never touch the Windows system disk.** `safety.rs` compares
   `IOCTL_STORAGE_GET_DEVICE_NUMBER` for the candidate against the disk that
   holds `%SystemRoot%`. The check runs **twice** — in the UI and again inside
   the repair engine.
2. **Only removable media.** `safety_check` refuses anything that is not USB /
   removable unless it is explicitly removable media.
3. **Always require elevation.** The app self-elevates; the manifest also says
   `requireAdministrator` when embedded.
4. **Success requires independent verification.** Never report success from a
   tool's exit code alone.

## Design decisions worth knowing

| Decision | Rationale |
|---|---|
| `diskpart` for layout | Most reliable way to rebuild a partition table Windows accepts. |
| `fmifs!FormatEx` for format | In-process, precise Win32 error code, progress callback. Falls back to `diskpart format` then `format.com`. |
| Boot-sector verification | Works without a mounted volume — the case that actually bites. |
| No temp files | `diskpart` is fed on **stdin**; removes the symlink/TOCTOU risk. |
| System tools resolved from `%SystemRoot%\System32` | Prevents binary planting via PATH / current directory. |
| `fmifs.dll` loaded with `LOAD_LIBRARY_SEARCH_SYSTEM32` | Prevents DLL search-order hijacking. |
| FFI struct sizes asserted in tests | `VOLUME_DISK_EXTENTS` and friends are ABI; a wrong layout corrupts memory. |

## Tools beyond "repair and format"

`tools.rs` covers the cases a plain format does not:

| Tool | Use |
|---|---|
| `scan_lost_partitions` | TestDisk-style search for orphaned boot sectors. |
| `clear_read_only` | Disk **and** volume level, plus registry policy detection. |
| `run_chkdsk` | Filesystem repair when a volume is mounted. |
| `verify_capacity` | Counterfeit / fake-capacity test (writes at 25/50/75/95 %). |
| `full_overwrite` | SD Association formatter style — zero the whole card. |
| `verify_disk` | Non-destructive "is this formatted?" check. |

## Known issues / gotchas

- **Startup stutter** — the initial disk probe now runs on a background thread
  and pushes candidates to the UI via a `Notice`. Do not call `refresh_ui`
  directly from the UI thread.
- **`State` is `Arc<State>`** (Send + Sync) because it is shared with worker
  threads. The NWG controls stay `Rc`. Mixing `Rc<State>` and threads does not
  compile.
- **`table.set_redraw(false/true)`** around list updates avoids flicker.
- **`diskpart` output is localized.** Never parse it for success; scan for the
  presence of specific English strings only as a *hint*, and verify separately.
- **`VOLUME_DISK_EXTENTS`** layout on x64: `NumberOfDiskExtents` at offset 0,
  padding at 4, `Extents[0].DiskNumber` at offset **8**. Reading offset 0 was a
  real bug (`volumes::tests` guards it).

## Testing notes

- Unit tests cover MBR / GPT / isohybrid parsing, the extent decoder, label
  sanitisation, `FsChoice`, `safety_check`, and MBR generation.
- `repair::tests::mbr_has_one_fat32_partition_at_1mib` is the regression test
  for the fallback layout writer.
- There is no automated end-to-end test: it needs Administrator rights and a
  real stick. Manual matrix: a dd'd stick, a healthy stick, a >32 GiB stick
  (exFAT path), and a write-protected SD card.

## Manual verification checklist

```powershell
# 1. List disks (shows MEDIA column)
.\StickDoctorCLI.exe

# 2. Non-destructive check
.\StickDoctorCLI.exe verify 3

# 3. Scan for lost partitions (read-only)
.\StickDoctorCLI.exe scan 3

# 4. Repair
.\StickDoctorCLI.exe repair 3 fat32 MYSTICK

# 5. After repair, the stick should be FAT32 and visible in Explorer.
#    If it is not visible, unplug and replug it.
```

## Commit conventions

Short imperative subject, body explains **why**. One logical change per
commit. Tag releases as `vX.Y.Z` — the release workflow attaches binaries and a
`SHA256SUMS.txt`.
