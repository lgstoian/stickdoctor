# Architecture

```
crates/
  stickdoctor-core/     no UI dependencies, all disk logic
    enumerate.rs        probe \\.\PhysicalDrive0..63
    props.rs            bus type, size, model, serial, writable
    layout.rs           MBR/GPT/isohybrid detection (pure parser + reader)
    volumes.rs          volume <-> disk mapping, lock/dismount, wait_for_volume
    safety.rs           system-disk detection, USB-only policy
    format.rs           fmifs FormatEx, diskpart and format.com fallbacks
    repair.rs           the repair pipeline (orchestration + verification)
    log.rs              structured event log
    error.rs            error type + Win32 message mapping
  stickdoctor-cli/      thin CLI over the core
  stickdoctor-app/      native Win32 GUI (native-windows-gui)
```

## Repair pipeline

`execute_repair_with_progress()` performs these stages, and each one is verified
before the next runs:

1. **Safety gate** — read disk properties, refuse anything that is not USB or
   that hosts Windows.
2. **Unmount** — enumerate volumes on the disk (via
   `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`), lock and dismount each.
3. **Optional raw wipe** — zero the first and last 1 MiB (kills stale GPT
   backup headers and isohybrid residues).
4. **Delete layout** — `IOCTL_DISK_DELETE_DRIVE_LAYOUT` (locale-independent),
   with `diskpart clean` as a fallback.
5. **Create partition** — `diskpart`: `create partition primary`, `assign`,
   `rescan`.
6. **Wait for the volume** — poll until Windows exposes a volume on the disk.
7. **Format** — try in order:
   * `fmifs.dll!FormatEx` on the volume root, with the volume locked,
   * `diskpart format` on the specific volume,
   * `format.com E: /FS:... /Q`.
8. **Verify** — `GetVolumeInformationW` must report the requested filesystem.
   If it does not, the repair fails with a clear message.

Step 8 is what makes the tool trustworthy: it never reports success based only
on a tool's exit code.

## Why diskpart for layout but fmifs for format

`diskpart` is the most reliable way to rebuild a partition table that Windows
accepts. But it is a separate process whose failures are reported as localized
text. `fmifs` is in-process, returns a Win32 error code, and gives a progress
callback. Using each for what it does best, then verifying independently, is the
most robust combination.

## FFI notes

* `VOLUME_DISK_EXTENTS` layout on x64: `NumberOfDiskExtents` at offset 0,
  padding at 4, `DISK_EXTENT[0].DiskNumber` at offset 8. Reading offset 0 was a
  real bug (see `CHANGELOG.md`); `volumes::tests` guards it.
* All raw pointers are confined to small `unsafe` blocks with the struct layout
  documented next to them.