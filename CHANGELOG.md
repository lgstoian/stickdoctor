# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [v0.3.0] - 2026-10-31

### Added
- **Machine-readable breadcrumbs (`@sd`)** — every source file now carries
  `@sd file` / `flow` / `invariant` / `verify` / `hazard` / `anchor` / `ref`
  annotations, parsed by the new `stickdoctor-agent` crate (no dependencies).
  `docs/AGENT_MAP.md` is a generated, CI-checked map of the codebase;
  `docs/AGENT_ANNOTATIONS.md` documents the schema.
- **False-green guard** — `cargo test` fails when a breadcrumb is missing or
  malformed, an `invariant`/`verify` entry has no matching test, a `flow` is out
  of order, a `#[test]` contains no assertion, or a function named by `@sd
  verify` can return success for an unknown input (`=> true`). The System32
  tool-resolution and stdin-script spellings are checked as well.
- **`agent-map` CLI command** — `stickdoctor-cli agent-map [--json] [--write]`
  prints or refreshes the breadcrumb map.

### Fixed
- **A repair could be reported successful without verification.** The pipeline
  logged boot-sector, filesystem and read/write failures as warnings and still
  returned `Ok(())`. `execute_repair_with_progress()` now collects the checks
  into a `VerificationOutcome` and returns `Err` unless every applicable check
  passed (and at least one ran).
- **`verify_boot_sector` accepted an unknown filesystem name** (`_ => true`).
  The new pure `boot_sector_matches` returns `false` for anything it does not
  recognise, with tests for the wrong signature, an unknown name and a truncated
  buffer.
- **`partition_exists` accepted a GPT entry.** The new pure
  `layout_is_usable_mbr` requires MBR style, a non-zero partition count and a
  non-zero first offset/length.
- **The counterfeit-capacity test's offsets are now guarded for distinctness**
  via the pure `capacity_offsets` / `capacity_marker`, so the test cannot become
  vacuous if two offsets collide.
- The safety decision was extracted into the pure `classify_safety`, so the
  system-disk and removable-only invariants are guarded without a real disk.

### Fixed (audit follow-up)
- **Destructive tools bypassed the safety gate.** The GUI tool buttons (full
  overwrite, capacity test, clear-readonly, `chkdsk`) and the CLI `capacity` /
  `clear-readonly` commands ran on whichever disk was selected, including the
  Windows system disk. The core tools now call `ensure_safe_for_tool`, and the
  GUI's `run_tool` gates every modifying tool on `can_repair()`.
- **`--rebuild-only` still formatted the disk.** `create_partition` always
  emitted a `format` command, so the "rebuild partition table only" mode wrote a
  filesystem. The script builder is now pure (`create_partition_script`) and
  skips `format` in rebuild-only mode.
- **The "Quick format" checkbox was ignored in the diskpart path.** The script
  always said `quick`; it now honours `RepairOptions::quick`.
- **`classify_safety` refused SD/MMC cards.** A card in a native SD/MMC slot
  reports `BusType::Sd`/`Mmc` and was refused as `NotUsb`. Removable SD/MMC is
  now accepted (with the system-disk check), matching the tool's stated target.
- **The fallback writers hardcoded a FAT32 partition type.** `create_partition_ioctl`
  and `build_mbr` always wrote `0x0C`; they now use `FsChoice::partition_type_byte()`
  (`0x07` for exFAT/NTFS).
- **`format.com` fallback built a nonsense drive argument** for a volume with
  only a GUID path (`\:`). It now refuses clearly.
- **`full_overwrite` could loop forever** if `WriteFile` reported success with
  zero bytes written.
- The CLI `repair` command now refuses non-removable/system disks before the
  confirmation prompt.

### Security
- `run_chkdsk` resolved `chkdsk` from PATH/current directory (binary planting,
  S9); it now uses `%SystemRoot%\System32\chkdsk.exe`. The `@sd` guard rejects a
  bare `Command::new("chkdsk")`.

### Fixed (independent audit)

- **The "never touch the Windows system disk" check never worked.**
  `STORAGE_DEVICE_NUMBER` is `{ DeviceType; DeviceNumber; PartitionNumber }` —
  12 bytes, with `DeviceNumber` at offset 4 — but `safety.rs` and `props.rs`
  declared an 8-byte struct and read offset 0 (`DeviceType`), so
  `IOCTL_STORAGE_GET_DEVICE_NUMBER` failed and `is_system_disk` always returned
  false. Both now use the `windows-sys` type, and `is_system_disk` **fails
  closed** when the numbers cannot be determined. The 12-byte ABI is asserted in
  `props::ffi_layout`, and the `@sd` guard rejects a hand-rolled struct.
- **The GUI did not exit after relaunching elevated.** A non-elevated parent
  kept building the UI next to the elevated child; it now returns after the
  relaunch.
- **Tools could run concurrently with a repair.** `run_tool` now refuses while
  a repair holds the disk (`busy`).
- **The fmifs callback could read past a packet** and used packet constants from
  an undocumented convention. It now bounds the wide-string read by
  `PacketLength` and handles both `PROGRESS`/`OUTPUT` and `1`/`2`.
- **`capacity_offsets` collapsed to duplicate offsets on tiny media**, making
  the counterfeit test vacuous; `verify_capacity` refuses such media.
- **`full_overwrite` reported success for a zero-size disk.**
- **`run_chkdsk` hid its exit status**; the status is now included in the output.
- **FFI hygiene:** `partition_exists` reads `DRIVE_LAYOUT_INFORMATION_EX` with
  `read_unaligned` (the buffer is only byte-aligned), and `create_partition_ioctl`
  zero-initialises the layout so the kernel never sees uninitialised union bytes.
- **`IOCTL_STORAGE_QUERY_PROPERTY` used a fixed 1 KiB buffer**; it now retries
  with 64 KiB on `ERROR_INSUFFICIENT_BUFFER`/`ERROR_MORE_DATA`.

## [0.2.2] - 2026-10-20

### Fixed
- **The stick was never actually formatted.** Two independent bugs:
  1. `IOCTL_DISK_DELETE_DRIVE_LAYOUT` and `create partition primary` were run in
     separate processes. DiskPart/VDS kept a stale layout, so
     `create partition primary` failed with *"No usable free extent could be
     found"* — and because DiskPart exits 0 on failure, the tool reported
     success. `clean` and `create partition primary` now run in the **same**
     DiskPart session.
  2. Success was defined as *"a drive letter appeared"*. On many USB bridges
     Windows never registers a volume for a freshly created partition, so the
     tool aborted even though the filesystem had been written. Success is now
     defined by reading the partition **boot sector** and checking the
     filesystem signature (`FAT32   ` / `EXFAT   ` / `NTFS    `), which needs
     no mounted volume. Volume detection remains best-effort.
- **Startup stutter / "Not responding"** — the initial disk probe ran on the UI
  thread. It now runs on a background thread and pushes the results to the UI
  via a `Notice`.
- **UI flicker** — the hotplug timer fired during startup and refreshed the list
  repeatedly. Removed; use Refresh or replug the media.
- Volume locking now also tries `FSCTL_DISMOUNT_VOLUME` when the exclusive lock
  is refused, so a held volume no longer blocks DiskPart.

### Added
- **Format now runs inside the same DiskPart session** as `clean` + `create`,
  writing the filesystem to the partition directly.
- **`verify <N>`** command — non-destructive check whether a disk has a mounted,
  formatted volume.
- **Scan for lost partitions** (TestDisk-style) — probes the media for orphaned
  FAT12/16/32, exFAT, NTFS, ext and ISO9660 boot sectors and reports their LBA.
  Read-only.
- **Clear write protection** — clears the read-only attribute at both the disk
  and volume level, and reports any `StorageDevicePolicies\WriteProtect`
  registry policy.
- **Repair filesystem (chkdsk)** — runs `chkdsk <drive>: /f` when a volume is
  mounted.
- **Test capacity** — counterfeit / fake-capacity test: writes a marker at
  25/50/75/95 % of the reported size and reads it back. **Writes to the media.**
- **Full overwrite** — SD Association formatter style: zero the entire card
  before formatting. Fixes cards that quick formatting cannot.
- **Media type detection** — the list shows USB stick / SD card / CF card / SSD /
  HDD, and the CLI gained a `MEDIA` column.
- **`AGENTS.md`** — architecture, safety invariants, design decisions and the
  testing matrix for future agents.

## [0.2.1] - 2026-10-19

### Fixed
- **The GUI failed at startup** with *"The procedure entry point
  GetWindowSubclass could not be located"*. The embedded manifest did not
  declare the Common Controls v6 dependency, so Windows loaded `comctl32` v5,
  which does not export `GetWindowSubclass`. The manifest now binds to
  `Microsoft.Windows.Common-Controls` 6.0.0.0. Verified by launching the built
  binary with the manifest applied.
## [0.2.0] - 2026-10-19

### Added
- **Post-format read/write test**: after formatting, a 64 KiB file is written,
  flushed, read back and compared. A failure is reported as a failed repair.
- **Hotplug detection**: the disk list refreshes automatically when a stick is
  plugged in or removed.
- **Live progress bar** driven by the `fmifs` percentage callback plus weighted
  repair stages.
- **Rebuild partition table only** option (skips formatting).
- **Drive-letter fallback**: assigns the first free letter if Windows does not.
- **CLI `--json`** output for scripting, plus `--rebuild-only`.
- **Settings persistence**: filesystem, label and options are remembered in
  `%APPDATA%\StickDoctor\config.json`.
- **Copy log** button.
- Embedded Windows icon, version information and a `requireAdministrator`
  manifest when built with the MSVC toolchain.
- `SECURITY.md` and `docs/SECURITY.md` with the threat model and audit results.
- `cargo-audit` and `cargo-deny` in CI with a reviewed policy.

### Security
- `diskpart.exe` and `format.com` are now resolved from `%SystemRoot%\System32`,
  preventing binary planting in the current directory or PATH (S1).
- `fmifs.dll` is loaded with `LoadLibraryExW` and
  `LOAD_LIBRARY_SEARCH_SYSTEM32`, preventing DLL search-order hijacking (S2).
- The diskpart script is passed on stdin instead of a predictable temporary
  file, removing a symlink/TOCTOU vector (S3).
- The log is saved through a user-chosen path instead of a predictable temp
  file (S4).
- Added compile-time assertions for the Win32 ABI struct sizes.

### Changed
- The GUI was rebuilt around a table view with clear steps, a details pane, a
  progress bar and result feedback.
- `enumerate_usb_disks` was renamed to `enumerate_disks` (it never filtered USB).

### Fixed
- Volume-to-disk mapping read `NumberOfDiskExtents` instead of `DiskNumber`, so
  volumes were never matched on disks other than disk 1 and nothing was
  unmounted before repair.
- Verification reported success as soon as a drive letter appeared, even when
  no filesystem had been written. Success now requires reading the filesystem
  back and comparing it.
- Formatting failures are no longer inferred from English `diskpart` strings;
  success is defined by independent verification.
- The "zero first/last 1 MiB" option is now actually executed.
- Volume locking uses the `\\.\E:` device path required by `FSCTL_LOCK_VOLUME`.