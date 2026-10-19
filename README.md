# StickDoctor

**Recover a USB stick that Windows can no longer format or even see after you
`dd`'d a raw OS image onto it.**

![CI](https://github.com/OWNER/stickdoctor/actions/workflows/ci.yml/badge.svg)
![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)

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

StickDoctor identifies the stick, explains what is on it, and rebuilds it into a
normal FAT32/exFAT/NTFS volume in one step.

## What it does

1. **Lists every physical disk** with bus type, size, model and safety status,
   so the stick is easy to tell apart from your system disk.
2. **Diagnoses the layout**: MBR, GPT, isohybrid, blank, or Linux-only
   partitions, and says why Windows refuses to format it.
3. **Repairs it**: unmounts the volumes, removes the old partition table,
   creates a new primary partition, formats it, and then **reads the filesystem
   back to verify the result**.

Supported filesystems: **FAT32**, **exFAT**, **NTFS**. Quick format by default;
optional full format and optional zeroing of the first/last 1 MiB for stubborn
sticks.

## Safety

* Requires Administrator (UAC prompt).
* **Refuses to touch** non-USB disks and the disk that hosts Windows — checked
  in the UI *and* again inside the repair engine.
* Shows a confirmation dialog with the disk number, model, size and target
  filesystem before doing anything.
* **Never reports success unless the filesystem was verified after formatting.**

There is no undo. Read [docs/SAFETY.md](docs/SAFETY.md) before using it on a disk
you care about.

## Download and run

1. Download `StickDoctor.exe` from the
   [latest release](https://github.com/OWNER/stickdoctor/releases/latest).
2. Verify it against `SHA256SUMS.txt` if you like.
3. Double-click it and accept the UAC prompt.
4. Select your stick, choose a filesystem, click **REPAIR STICK**.

The binary is unsigned, so SmartScreen may warn on first run
(*More info* -> *Run anyway*). Some antivirus engines flag tools that write to
raw disks; verify the source and checksum before trusting a binary.

### CLI

```
StickDoctorCLI.exe                          # list disks
StickDoctorCLI.exe repair 3 fat32 MYSTICK   # repair disk 3
StickDoctorCLI.exe repair 3 exfat MYSTICK --yes --zero
StickDoctorCLI.exe repair 3 --dry-run       # show what would happen
```

## Troubleshooting

See [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md). The short version: the
log pane contains the real error — press **Save log** and read it.

Common cases:

| Symptom | Meaning |
|---|---|
| `Repair refused (safety)` | Wrong disk selected, or it is not USB |
| `all formatting methods failed` | Stick is write-protected or failing |
| `format did not take effect ... reports RAW` | Format ran but wrote no filesystem |
| FAT32 rejected on a large stick | Windows limit: use exFAT |

## How it works

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). In short:

```
detect -> diagnose -> unmount -> clean -> create partition -> format -> verify
```

* Partition table rebuilt with `diskpart` (most compatible).
* Formatting via `fmifs.dll!FormatEx`, with `diskpart` and `format.com` as
  fallbacks.
* Success is defined by `GetVolumeInformationW`, not by a tool's exit code.

## Build from source

Requirements:

* Rust stable (pinned by `rust-toolchain.toml`)
* **Visual Studio Build Tools** with *Desktop development with C++* for the MSVC
  linker and `rc.exe` (used to embed the icon and version information)

```powershell
cargo build --release
# target/release/stickdoctor-app.exe
# target/release/stickdoctor-cli.exe
```

The GNU toolchain also works, but it cannot embed the Windows resources, so the
exe will have a generic icon.

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Project layout

```
crates/stickdoctor-core   disk logic, no UI
crates/stickdoctor-cli    command line front end
crates/stickdoctor-app    native Win32 GUI (native-windows-gui)
docs/                     architecture, safety, troubleshooting
```

## Why not just use Rufus?

Rufus is excellent and can also restore a stick. StickDoctor is narrower on
purpose: it diagnoses *why* Windows is refusing, shows the partition table it
found, and gives you a log you can read. If you already know the `diskpart`
incantation, this is a safer, repeatable wrapper around it.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Safety-related changes need extra care.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).