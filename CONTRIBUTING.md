# Contributing to StickDoctor

Thanks for helping. This project deals with raw disk access, so correctness and
safety matter more than speed.

## Building

Requirements on Windows:

* Rust stable (see `rust-toolchain.toml`)
* [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
  with the **Desktop development with C++** workload (for the MSVC linker and
  `rc.exe`). The GNU toolchain also works but cannot embed the icon/version
  resources.

```powershell
cargo build            # debug
cargo build --release  # release, produces target/release/*.exe
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Before opening a PR

* `cargo fmt --all`
* `cargo clippy --all-targets -- -D warnings`
* `cargo test`
* If you touched disk I/O, describe the hardware you tested on.

## Safety rules for changes

* Never weaken the safety gate in `safety.rs`. It must refuse the Windows system
  disk and any non-USB disk.
* Never report success without reading the result back (see `verify_volume`).
* Never build shell/diskpart scripts from unvalidated user input.
* Any new `#[repr(C)]` FFI struct must have a matching size assertion where
  practical.

## Commit style

Short imperative subject, then a body explaining **why** if it is not obvious.
One logical change per commit.