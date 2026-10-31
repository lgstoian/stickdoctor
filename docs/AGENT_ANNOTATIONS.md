# Agent annotations (`@sd`)

StickDoctor runs elevated and writes to raw disks, so an agent that edits it
needs more than prose comments: it needs a **machine-readable map** of what is
where, what depends on what, and — most importantly — what "success" means.

Every source file carries `@sd` annotations. They are ordinary comments, so they
cost nothing at runtime, but they follow a strict schema that
`stickdoctor-agent` parses and validates. The validation runs in CI as a test,
so a stale or malformed breadcrumb fails the build instead of quietly misleading
the next agent.

## Syntax

An annotation is a comment line whose text (after `//!`, `///` or `//`) begins
with `@sd`, followed by a kind and `key=value` pairs:

```rust
//! @sd file module="crates/stickdoctor-core/src/repair.rs" role="repair orchestration"
//! @sd flow id=repair.verify step=7 pipeline=repair label="Verify filesystem" next=repair.report
```

* Values that contain spaces are quoted: `role="format and verification"`.
* A `key=value` token that is not quoted may not contain spaces.
* `@sd` only counts when it is the first token on the line. Prose that merely
  mentions `@sd` mid-sentence is ignored.

## Kinds

| Kind | Required keys | Optional keys | Meaning |
|---|---|---|---|
| `file` | `module`, `role` | `entry`, `exit`, `depends` | Breadcrumb for a source file. `module` is the workspace-relative path. |
| `flow` | `pipeline`, `id`, `step`, `label` | `next`, `verify` | One stage of an ordered pipeline. |
| `invariant` | `id`, `text`, `test` | | A safety invariant and the test that guards it. |
| `verify` | `id`, `fn`, `proves`, `negative_test` | | An independent verification and the test that proves it can fail. |
| `hazard` | `text` | `guard` | A destructive or irreversible operation and its guard. |
| `anchor` | `name` | `text` | A named location other annotations can point at. |
| `ref` | `name` | | A cross-reference to an `anchor`. |

## Rules the guard enforces

`stickdoctor_agent::validate()` returns a list of problems; `cargo test` fails
when it is non-empty. It checks:

1. **Schema.** Every kind is known, every key is allowed for that kind, and every
   required key is present.
2. **File coverage.** Every non-test `.rs` file in the workspace has a `file`
   annotation whose `module` names the file it lives in. No orphan modules.
3. **Anchors and references.** Anchor names are unique; every `ref` resolves.
4. **Flow integrity.** Within a pipeline, `step` values are contiguous from 1,
   ids are unique, and each step's `next` points at the following step. The last
   step has no `next`.
5. **Guard references.** An `invariant`'s `test` and a `verify`'s
   `negative_test` must name a test that exists. A `flow`'s `verify` must name a
   `verify` annotation.
6. **No vacuous tests.** Every `#[test]` function contains at least one
   assertion (`assert*`, `.unwrap(`, `.expect(`, `panic!`, ...) and is not
   `#[ignore]`d. A test that cannot fail is a false green.
7. **Verification strength.** A function named by `@sd verify` must not contain
   `=> true`. A verification that can succeed on an unrecognised input is a
   false green.
8. **Security spellings.** No bare `Command::new("diskpart")` or
   `Command::new("format.com")`; `run_diskpart` resolves `diskpart.exe` from
   `System32` and feeds the script on stdin; `fmifs.dll` is loaded with
   `LOAD_LIBRARY_SEARCH_SYSTEM32`.

Rule 7 exists because the codebase previously had exactly that hole:
`verify_boot_sector` matched an unknown filesystem name with `_ => true`, so a
typo in the requested filesystem silently verified. The pure
`boot_sector_matches` now returns `false` for anything it does not recognise.

## Worked example: the repair pipeline

```rust
// @sd flow id=repair.safety step=1 pipeline=repair label="Safety gate" next=repair.unmount
// @sd flow id=repair.unmount step=2 pipeline=repair label="Unmount volumes" next=repair.zero
// ...
// @sd flow id=repair.verify step=7 pipeline=repair label="Verify filesystem" next=repair.report verify=repair-success
// @sd verify id=repair-success fn="is_success"
//     proves="a repair succeeded only if the filesystem was read back and matched"
//     negative_test=verification_rejects_boot_sector_failure
```

The `verify` entry is the load-bearing one: it names the pure decision function
that the pipeline calls before returning `Ok(())`, and the negative test that
proves the function fails when verification fails. Adding a new verification
stage means adding a `@sd verify` entry *and* a negative test — the guard will
not let you forget either.

## Commands

```powershell
# Print the breadcrumb map as Markdown
cargo run -p stickdoctor-cli -- agent-map

# Print it as JSON (machine-readable)
cargo run -p stickdoctor-cli -- agent-map --json

# Regenerate docs/AGENT_MAP.md after editing annotations
cargo run -p stickdoctor-cli -- agent-map --write

# Run the guard (also runs as part of `cargo test --all`)
cargo test -p stickdoctor-agent --test guard
```

`docs/AGENT_MAP.md` is committed and checked by the guard: if the code and the
map disagree, the test fails and tells you to regenerate it.

## Adding an annotation

1. Add the annotation to the file.
2. Run `cargo test -p stickdoctor-agent --test guard`.
3. Fix anything it reports.
4. Run `cargo run -p stickdoctor-cli -- agent-map --write`.

Do not weaken a rule to make the guard pass. If a rule genuinely does not apply
to your change, say so in the commit message instead.
