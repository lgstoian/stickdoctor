# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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