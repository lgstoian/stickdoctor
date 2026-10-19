# StickDoctor

**Recover a USB stick that Windows can no longer format or even see after you
`dd`'d a raw OS image onto it.**

![CI](https://github.com/lgstoian/stickdoctor/actions/workflows/ci.yml/badge.svg)
![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)
![Version](https://img.shields.io/badge/version-0.2.0-blue)

---

## The problem

You write an ISO or raw image onto a USB stick with `dd` or balenaEtcher, boot
it once, and then Windows refuses to take the stick back:

* it is missing from **Explorer**,
* it shows in **Disk Management** as *Removable* but **Format is greyed out**,
* or Windows says *"The volume is too big"* / *"Access denied"*.

The image replaced the partition table and filesystem with something Windows
does not mount (Linux `0x83`, ISO9660, a hybrid MBR, GPT, or a table that points
past the end of the device). `diskpart clean` usually fixes it, but it is easy
to get wrong — and if you pick the wrong disk you lose your data.

StickDoctor identifies the stick, explains what is on it, rebuilds it into a
normal FAT32/exFAT/NTFS volume, and then **proves the result works**.

## What it does

1. **Lists every physical disk** with bus type, size, model and safety status,
   so the stick is easy to tell apart from your system disk.
2. **Diagnoses the layout**: MBR, GPT, isohybrid, blank, or Linux-only
   partitions, and says why Windows refuses to format it.
3. **Repairs it**: unmounts the volumes, removes the old partition table,
   creates a new primary partition, formats it, verifies the filesystem, and
   finally writes and reads back a test file.

### Features

- FAT32, exFAT and NTFS; quick or full format; optional zeroing of the first
  and last 1 MiB for stubborn sticks.
- **Post-format read/write test** so "formatted" actually means "usable".
- **Automatic hotplug refresh** when you plug or unplug a stick.
- **Live progress bar** driven by the formatter's own percentage callback.
- **Rebuild partition table only** mode for advanced users.
- **Drive-letter fallback** if Windows does not assign one.
- **Settings are remembered** in `%APPDATA%\StickDoctor\config.json`.
- **Copy log** and **Save log**.
- CLI with `list`, `repair`, `--json`, `--dry-run`, `--rebuild-only`.

## Safety

* Requires Administrator (UAC prompt).
* **Refuses to touch** non-USB disks and the disk that hosts Windows — checked
  in the UI *and* again inside the repair engine.
* Shows a confirmation dialog with the disk number, model, size and target
  filesystem before doing anything.
* **Never reports success unless the filesystem was verified and a write/read
  test passed.**

There is no undo. Read [docs/SAFETY.md](docs/SAFETY.md) before using it on a disk
you care about.

## Security

StickDoctor runs elevated and writes to raw disks, so it is built defensively:

- system tools are resolved from `%SystemRoot%\System32` (no PATH hijacking),
- `fmifs.dll` is loaded with `LOAD_LIBRARY_SEARCH_SYSTEM32`,
- no temporary files are used for `diskpart` scripts,
- labels are filtered before they reach any script,
- FFI struct sizes are asserted in tests.

See [SECURITY.md](SECURITY.md) for the disclosure policy and
[docs/SECURITY.md](docs/SECURITY.md) for the threat model and audit results.

## Download and run

1. Download `StickDoctor.exe` from the
   [latest release](https://github.com/lgstoian/stickdoctor/releases/latest).
2. Verify it against `SHA256SUMS.txt`.
3. Double-click it and accept the UAC prompt.
4. Select your stick, choose a filesystem, click **REPAIR STICK**.

The binary is unsigned, so SmartScreen may warn on first run
(*More info* -> *Run anyway*). Some antivirus engines flag tools that write to
raw disks; verify the source and checksum before trusting a binary.

### CLI

```
StickDoctorCLI.exe                              # list disks
StickDoctorCLI.exe list --json                  # machine-readable
StickDoctorCLI.exe repair 3 fat32 MYSTICK       # repair disk 3
StickDoctorCLI.exe repair 3 exfat MYSTICK --yes --zero
StickDoctorCLI.exe repair 3 --rebuild-only      # no format
StickDoctorCLI.exe repair 3 --dry-run           # show what would happen
```

## Troubleshooting

See [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md). The short version: the
log contains the real error — press **Copy log** or **Save log**.

| Symptom | Meaning |
|---|---|
| `Repair refused (safety)` | Wrong disk selected, or it is not USB |
| `all formatting methods failed` | Stick is write-protected or failing |
| `format did not take effect ... reports RAW` | Format ran but wrote no filesystem |
| `read/write test mismatch` | Formatting worked but the media is failing |
| FAT32 rejected on a large stick | Windows limit: use exFAT |

## How it works

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). In short:

```
detect -> diagnose -> unmount -> clean -> create partition
       -> format -> verify filesystem -> verify read/write
```

* Partition table rebuilt with `diskpart` (most compatible).
* Formatting via `fmifs.dll!FormatEx`, with `diskpart` and `format.com` as
  fallbacks.
* Success is defined by `GetVolumeInformationW` plus a real write/read test,
  not by a tool's exit code.

## Build from source

Requirements:

* Rust stable (1.77 or newer)
* **Visual Studio Build Tools** with *Desktop development with C++* for the MSVC
  linker and `rc.exe` (used to embed the icon, version info and manifest)

```powershell
cargo build --release
# target/release/stickdoctor-app.exe
# target/release/stickdoctor-cli.exe
```

The GNU toolchain also builds, but cannot embed the Windows resources.

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
cargo audit
cargo deny check
```

## Project layout

```
crates/stickdoctor-core   disk logic, no UI
crates/stickdoctor-cli    command line front end
crates/stickdoctor-app    native Win32 GUI (native-windows-gui)
docs/                     architecture, security, safety, troubleshooting
```

## Why not just use Rufus?

Rufus is excellent and can also restore a stick. StickDoctor is narrower on
purpose: it diagnoses *why* Windows is refusing, shows the partition table it
found, verifies the result, and gives you a log you can read. If you already
know the `diskpart` incantation, this is a safer, repeatable wrapper around it.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Safety- and security-related changes
need extra care.

## License

GPL-3.0-or-later, Copyright (C) 2026 Alexandru Stoian. See [LICENSE](LICENSE).