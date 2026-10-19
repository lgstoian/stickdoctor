# Safety

StickDoctor performs irreversible writes to raw disks. Read this before using it.

## What it will never do

* It will not touch a disk that is not on the USB bus.
* It will not touch the disk that hosts Windows (checked by comparing storage
  device numbers, both in the UI and again inside the repair engine).
* It will not run without Administrator rights.
* It will not report success unless it has read the filesystem back and
  confirmed it matches what you asked for.

## What it will do

* Erase **everything** on the selected disk: partition table, filesystem and
  all data.
* Optionally overwrite the first and last 1 MiB with zeros.

## How to stay safe

1. Unplug every USB disk you do not intend to erase.
2. Check the **model**, **size** and **bus** in the confirmation dialog before
   clicking Yes.
3. Your system disk will be marked `[SYSTEM - DO NOT WIPE]` and cannot be
   selected for repair.
4. If you are unsure, use the CLI `--dry-run` first.

## Verifying the binary

Releases are built from the tagged source by GitHub Actions. To build it
yourself:

```powershell
git clone https://github.com/alexandrustoian/stickdoctor
cd stickdoctor
cargo build --release
```

Compare the SHA-256 of the downloaded binary with the `SHA256SUMS.txt` attached
to the release.

## Antivirus

A tool that writes to raw disks can trigger heuristic detection. If Defender
flags it, verify the source and the checksum before deciding. Do not run an
unsigned binary you did not build or verify.