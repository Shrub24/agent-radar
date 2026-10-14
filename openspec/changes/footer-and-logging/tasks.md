## 1. Implementation

- [x] 1.1 Elide footer lines, age out completed outcomes, keep failures; unit tests for the cut and the aging.
- [x] 1.2 Add `tracing`; the daemon logs to stderr, the dashboard to `tui.log`; replace the daemon's bare `eprintln!` diagnostics; log every footer message and outcome.
- [ ] 1.3 Live check: a long refusal in a narrow terminal, and `tui.log` holding its whole text.

## 2. Verification

- [x] 2.1 Run fmt, strict Clippy, the locked tests, strict spec validation and the checked Nix build; record real exit codes.
