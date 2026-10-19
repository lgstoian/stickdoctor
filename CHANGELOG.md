# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Nothing yet.

## [0.1.0] - 2026-10-19

### Added
- Native Windows GUI (`StickDoctor.exe`) and a CLI (`StickDoctorCLI.exe`).
- Physical disk enumeration with bus type, size, model, partition style and
  safety status.
- Diagnosis of MBR / GPT / isohybrid / blank layouts, including a plain-English
  explanation of why Windows refuses to format the stick.
- Repair pipeline: unmount volumes, remove the old partition table, create a
  primary partition, format, then **verify the filesystem really exists**.
- Filesystem choice: FAT32, exFAT, NTFS; quick or full format; optional raw
  zeroing of the first and last 1 MiB.
- Formatting tries `fmifs.dll!FormatEx` first, then `diskpart`, then
  `format.com`, and reports the error from each attempt.
- Safety gate that refuses the Windows system disk and any non-USB disk, checked
  both in the UI and inside the repair engine.
- CLI `--dry-run`, `--yes`, `--zero`, `--help`, `--version`.
- Unit tests for the MBR/GPT/isohybrid parsers and the volume-extent decoder.

### Fixed
- Volume-to-disk mapping read `NumberOfDiskExtents` instead of `DiskNumber`,
  so volumes were never matched on disks other than disk 1. This meant volumes
  were not unmounted before repair.
- Verification reported success as soon as a drive letter appeared, even when no
  filesystem had been written. Success is now defined by reading the filesystem
  back and comparing it to the requested one.
- `diskpart` error detection relied on English output strings. Decisions no
  longer depend on locale.
- The "zero first/last 1 MiB" option was accepted by the UI but never executed.