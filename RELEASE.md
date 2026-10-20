# StickDoctor — Release Guide

The gate to clear **before** a public release, end to end: version freeze,
builds, verification ladder, packaging (binaries + SHA256SUMS), legal/hygiene,
docs, and a sign-off checklist.

This is a checklist, not a second test manual. The mechanics of each step live
in their owning docs and are linked, not repeated:

| Topic | Owning document |
|-------|-----------------|
| Architecture | [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) |
| Safety model | [`docs/SAFETY.md`](docs/SAFETY.md) |
| Security review | [`docs/SECURITY.md`](docs/SECURITY.md) |
| Troubleshooting | [`docs/TROUBLESHOOTING.md`](docs/TROUBLESHOOTING.md) |
| Agent notes | [`AGENTS.md`](AGENTS.md) |

Current target version: **v0.2.2**. Substitute your release version throughout.

---

## 0. Release blockers (clear these first)

- **No open build warnings** — `cargo clippy --all-targets -D warnings` and
  `cargo fmt --all -- --check` must both pass clean.
- **No test failures** — `cargo test` fully green.
- **Confirmed on real hardware** — the repair flow verified on at least one
  actual USB stick that Windows had refused to format, plus the one-click
  `REPAIR STICK` path in the GUI.
- **Resources embedded** — built with the MSVC toolchain so the icon, version
  info and `requireAdministrator` manifest are embedded (Subsystem 2 in the PE
  headers; `requireAdministrator` present in the manifest resource).

---

## 1. Preflight

1. **Working tree** — everything intended is committed and the tree is clean
   (`git status`). Generated files (`target/`, `*.exe`, `*.o`) are git-ignored
   and must not be staged.
2. **Version freeze** — update, in one change:
   - `[workspace].version` in the root `Cargo.toml` (drives all three crates
     via `version.workspace = true`);
   - the `FILEVERSION` / `PRODUCTVERSION` / `VALUE "FileVersion"` /
     `VALUE "ProductVersion"` strings in
     `crates/stickdoctor-app/assets/app.rc`;
   - the `assemblyIdentity version` in
     `crates/stickdoctor-app/assets/stickdoctor.manifest`;
   - `Unreleased` → `## [vX.Y.Z] — YYYY-MM-DD` in [`CHANGELOG.md`](CHANGELOG.md)
     listing what changed;
   - version strings in [`README.md`](README.md) and
     [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) if they mention it.
3. **AI-comment sweep** — no `// AI:` comments anywhere:
   ```
   rg -n "// AI:" crates || echo "clean"
   ```

---

## 2. Build

One build directory; always the MSVC target (embeds Windows resources).

```powershell
cargo build --release
# target/release/stickdoctor-app.exe   (GUI)
# target/release/stickdoctor-cli.exe   (CLI)
```

**Expected:** zero compiler warnings/errors; both binaries produced.

**Verify resources are embedded:**
- File properties → Details shows `File description` ("StickDoctor …"),
  `File version` ("0.2.2"), and `Company name` ("Alexandru Stoian");
- the manifest contains `requireAdministrator` (UAC elevation);
- PE subsystem is 2 (Windows GUI, no console window);
- embedded icon present (4617-byte ICO in `assets/`).

CI mirrors this: [`.github/workflows/ci.yml`](.github/workflows/ci.yml).

---

## 3. Verify (the ladder)

Run in order.

| # | Step | Command | Pass condition |
|---|------|---------|----------------|
| 1 | Format check | `cargo fmt --all -- --check` | no diff |
| 2 | Clippy | `cargo clippy --all-targets -- -D warnings` | no output |
| 3 | Unit tests | `cargo test` | all green |
| 4 | Release build | `cargo build --release` | `Finished` |
| 5 | Elevation test | launch `StickDoctor.exe` as a normal user | UAC prompt appears; accepts |
| 6 | GUI smoke | launch, select a disk, confirm UI is responsive and the list populates | list shows disks; details pane updates; REPAIR STICK clickable |
| 7 | CLI repair | `StickDoctorCLI.exe repair 3 fat32 TEST` (elevated) | log ends with "Repair complete"; stick FAT32 and visible in Explorer |
| 8 | Read-only / SD matrix | run on a write-protected SD card (lock switch on) | detects read-only, refuses, explains why |
| 9 | >32 GiB exFAT | select FAT32 on a >32 GiB stick | offers exFAT instead |

Step 7 must pass on a real stick that Windows refused to format. If the stick
mounts fine after repair, the release is good.

---

## 4. Package the artifacts

Copy the release binaries and emit checksums.

```powershell
New-Item -ItemType Directory -Force -Path release | Out-Null
Copy-Item target\release\stickdoctor-app.exe release\StickDoctor.exe
Copy-Item target\release\stickdoctor-cli.exe release\StickDoctorCLI.exe

Push-Location release
Get-FileHash -Algorithm SHA256 * | ForEach-Object {
  "$($_.Hash.ToLower())  $($_.Path | Split-Path -Leaf)"
} | Set-Content -Encoding ascii SHA256SUMS.txt
Pop-Location
```

Artifacts produced under `release/` (git-ignored):

| Artifact | Contents |
|----------|----------|
| `StickDoctor.exe` | GUI binary, 488 KB, resources embedded |
| `StickDoctorCLI.exe` | CLI binary, 350 KB |
| `SHA256SUMS.txt` | SHA-256 of both binaries |

Verify: `Get-FileHash StickDoctor.exe` matches `SHA256SUMS.txt`.

---

## 5. Legal and hygiene checklist

- **Binary clean** — both exe files start with `MZ`, PE subsystem 2, no debug
  info, no console subsystem (`/SUBSYSTEM:WINDOWS`).
- **No secrets** — no keys, tokens, or credentials in the tree or the binaries.
- **License present** — `LICENSE` (GPL-3.0) and `README.md` license section
  reference it; vendored component licenses (`native-windows-gui` MIT,
  `windows` MIT/Apache-2.0) preserved.
- **Icons** — `assets/stickdoctor.ico` is original work, included.

---

## 6. Docs gate

- [`README.md`](README.md) — version, feature tour, usage (GUI + CLI), safety,
  troubleshooting, license.
- [`CHANGELOG.md`](CHANGELOG.md) — `Unreleased` collapsed into this release's
  section.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — module layout, repair pipeline.
- [`docs/SAFETY.md`](docs/SAFETY.md) — threat model, invariants.
- [`docs/SECURITY.md`](docs/SECURITY.md) — disclosure policy.
- [`docs/TROUBLESHOOTING.md`](docs/TROUBLESHOOTING.md) — symptom table.
- `AGENTS.md` — up to date.
- Screenshots in `docs/` reflect the current UI (columns: Disk / Media / Bus /
  Size / Model / Partition table / Safety; details pane; Tools section).

---

## 7. Screenshots

Capture with the released binary (not a debug build). Place in `docs/` and
reference from the README.

Recommended shots:

| Shot | Content |
|------|---------|
| `docs/screenshot-main.png` | Main window with a disk selected, details pane populated, Tools section visible, progress bar |
| `docs/screenshot-repair.png` | Log after a successful repair, showing `Disk summary`, `DiskPart formatted`, `Boot sector confirms FAT32`, `Repair complete` |
| `docs/screenshot-refused.png` | A refused disk marked "SYSTEM" with the safety message |

---

## 8. Sign-off

Release is ready when **all** boxes below are true:

- [ ] Version in `Cargo.toml`, `app.rc`, `manifest`, docs and `changelog.md`
      updated
- [ ] `cargo fmt`, `cargo clippy -D warnings`, `cargo test` all clean
- [ ] Release build succeeds with resources embedded
- [ ] GUI smoke test passes (list populates, details update, REPAIR STICK works)
- [ ] CLI repair verified on a real unformattable stick
- [ ] `StickDoctor.exe` requires elevation (UAC)
- [ ] `release/` produced with `SHA256SUMS.txt` verifying
- [ ] Legal/hygiene checklist complete
- [ ] Docs gate complete; screenshots reflect the current UI
- [ ] Tag the release commit and publish the artifacts + `SHA256SUMS.txt`
