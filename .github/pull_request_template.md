## What changed

<!-- One or two sentences. -->

## Why

<!-- The problem this solves. -->

## Testing

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test`

## Safety

- [ ] This does not weaken the safety gate
- [ ] Any success path still verifies the result
- [ ] No unvalidated input reaches diskpart or a shell