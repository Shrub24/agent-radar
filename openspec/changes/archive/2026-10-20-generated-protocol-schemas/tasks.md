# Tasks

## 1. Rust wire schemas

- [x] 1.1 Add pinned Schemars support and derive schemas only for the selected public facts, context-window, background-task, ping, task-status and spawn-exit DTOs.
- [x] 1.2 Introduce a typed ping result/task-status DTO and make the daemon serialize its ping reply through that type without changing existing fields or contract-name forward compatibility.
- [x] 1.3 Apply and document the JavaScript safe-integer maximum for context counts and the facts timestamp; keep cross-field and domain validation in Rust.
- [x] 1.4 Add a deterministic exporter for draft 2020-12 serialize and deserialize schemas; test Serde omission/null/tag behavior and the emitted integer bounds against actual DTO fixtures.

## 2. TypeScript SDK generation

- [x] 2.1 Pin `json-schema-to-typescript`, TypeScript and schema-test dependencies as development tools and add one generate/check command for schemas and SDK declarations.
- [x] 2.2 Generate TypeScript declarations for the SDK-consumed directions (ping output, facts input, spawn-exit output) and update those SDK types to consume them.
- [x] 2.3 Typecheck generated declarations and validate representative ping, facts and tagged-union fixtures against the generated schemas; inspect output for optional properties and number precision.

## 3. Documentation and gate

- [x] 3.1 Document generated artifact ownership, regeneration/check commands, selected DTO scope, and the deferred high-lifetime counter encoding decision.
- [x] 3.2 Verified: offline `npm ci`, contract drift/typecheck/tests (95/95), npm audit, `npm pack` plus offline tarball install/import/schema smoke, Rust fmt, strict Clippy, locked full suite (464 library tests), strict OpenSpec (34/34), and checked Nix build all exited 0. Nix output: `/nix/store/1chzn2q4b692n6xm5i5zihdvr6plqzdg-agent-radar-0.1.0`. Tmux acceptance and PTY/E2E smokes not run.
