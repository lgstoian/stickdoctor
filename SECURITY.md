# Security Policy

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.2.x   | Yes       |
| 0.1.x   | No        |

## Reporting a vulnerability

Please **do not open a public issue** for security problems.

Use GitHub's private vulnerability reporting:

<https://github.com/lgstoian/stickdoctor/security/advisories/new>

Include:

- a description of the issue and its impact,
- the version and how it was built,
- reproduction steps,
- the Windows version,
- any relevant log output.

You can expect an initial response within 7 days. Please allow time for a fix
before public disclosure.

## Scope

StickDoctor runs with Administrator rights and performs raw disk writes. The
following are in scope:

- incorrect disk selection or a safety-gate bypass,
- writing to a disk other than the one the user selected,
- privilege escalation (binary/DLL planting, insecure temp files),
- command or script injection through the label or other input,
- memory-safety issues in the FFI layer,
- supply-chain issues in the dependency tree.

Out of scope:

- data loss caused by the user confirming a repair on the wrong disk
  (the confirmation dialog shows the model, size and bus),
- sticks that are physically failing or write-protected at the controller
  level,
- the absence of an Authenticode signature (documented in `docs/SECURITY.md`).