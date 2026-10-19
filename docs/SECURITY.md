# Security Review

This document records the threat model and the security review performed for
0.2.0.

## Threat model

StickDoctor runs **elevated** and writes to raw block devices. The assets worth
protecting are:

1. **All disks** — a wrong write can destroy data on any attached drive.
2. **The Windows installation** — the system disk must never be touched.
3. **Integrity of the process** — an attacker must not be able to make the
   elevated process execute their code.

The realistic attacker is a **local, unprivileged user** who can write to
`%TEMP%`, the current directory, or directories on `PATH`, and who can plant
files that a later elevated run will touch. Remote attackers are out of scope
for a portable utility with no network code.

## Findings

### S1 — Binary planting in `diskpart` / `format.com` (High, fixed)

`Command::new("diskpart")` and `Command::new("format.com")` used the Windows
search order, which includes the current directory. A planted `diskpart.exe`
would have executed with Administrator rights.

**Fix:** both tools are resolved from `%SystemRoot%\System32\` by absolute path
(`format::system32`).

### S2 — DLL search-order hijacking of `fmifs.dll` (High, fixed)

`LoadLibraryW("fmifs.dll")` searched the application directory and `PATH` before
the system directory. A planted `fmifs.dll` beside the executable would load
into the elevated process.

**Fix:** `LoadLibraryExW` with `LOAD_LIBRARY_SEARCH_SYSTEM32` and an absolute
`System32` path.

### S3 — Insecure temporary file (Medium, fixed)

The diskpart script was written to `%TEMP%\stickdoctor_<pid>.txt`, a predictable
name created without `O_EXCL`. A local attacker could pre-create a symlink or
junction there and redirect an elevated write (CWE-377 / CWE-59).

**Fix:** the script is written to diskpart's **stdin**. No temporary file is
created at all, which removes the entire class of issue.

### S4 — Predictable log path (Medium, fixed)

"Save log" wrote to `%TEMP%\stickdoctor-<timestamp>.log` with `fs::write`.

**Fix:** the user chooses the destination through a Save-As dialog; the path is
no longer predictable or attacker-influenced.

### S5 — Command injection through the label (Low, mitigated)

The label is interpolated into a diskpart script. Mitigations:

- only `[A-Za-z0-9_-]` survive filtering,
- the label is capped at 11 characters,
- the disk index is a `u32`, not a string,
- no shell is involved (`Command` passes an argument vector),
- diskpart is fed on stdin, so a newline cannot appear in the label.

Regression tests cover the filter (`repair::tests::sanitize_label_*`).

### S6 — Unsigned binaries (Informational, accepted)

There is no Authenticode certificate. Consequences and mitigations:

- SmartScreen will warn on first run,
- every release ships `SHA256SUMS.txt`,
- the release is reproducible from the tagged source,
- `docs/SAFETY.md` explains how to verify.

Users should not run an unsigned binary they have not verified.

### S7 — FFI memory safety (Informational, reviewed)

There are 66 `unsafe` blocks, all confined to Win32 calls. Invariants:

- every output buffer is sized and its `bytes returned` is checked,
- `VOLUME_DISK_EXTENTS` is decoded with explicit offsets and a bounds check,
- `#[repr(C)]` struct sizes are asserted at test time
  (`props::ffi_layout::struct_sizes_match_the_windows_abi`),
- `FormatEx` is obtained with `GetProcAddress` and null-checked,
- handles are closed on every path, including error returns.

### S8 — Dependency supply chain (Informational, monitored)

CI runs `cargo-audit` (RustSec advisories) and `cargo-deny` (advisories,
licenses, banned crates, sources). `winapi 0.3` is pulled in by
`native-windows-gui` and is unmaintained; it has no known exploitable advisory
at the time of writing and is retained because it is the only practical backend
for the native controls. Any future advisory is tracked in `deny.toml`.

## Defences that are not security controls

The safety gate (refuse non-USB disks and the Windows system disk) is a
**safety** control, not a security boundary. It is checked twice — in the UI and
again inside the repair engine — so a UI bug cannot bypass it, but it is not
designed to resist a determined attacker who already controls the machine.

## Verification

```powershell
cargo audit
cargo deny check
cargo test
cargo clippy --all-targets -- -D warnings
```

## Review status

| Finding | Severity | Status |
|---------|----------|--------|
| S1 binary planting | High | Fixed in 0.2.0 |
| S2 DLL search order | High | Fixed in 0.2.0 |
| S3 insecure temp file | Medium | Fixed in 0.2.0 |
| S4 predictable log path | Medium | Fixed in 0.2.0 |
| S5 command injection | Low | Mitigated + tested |
| S6 unsigned binaries | Informational | Accepted, documented |
| S7 FFI memory safety | Informational | Reviewed |
| S8 supply chain | Informational | Monitored in CI |