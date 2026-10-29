# Proposal

## Why

The daemon already owns admission, durable execution and reconciliation, but exchanges much of their meaning through hand-built JSON and string vocabularies. Rust consumers recover that meaning from serialized responses, multiplying state representations and making uncertainty, replay and privacy rules harder to maintain.

## What Changes

- Decode closed request vocabularies once at ingress; direct internal callers use the same typed service entry points without self-RPC or JSON round trips.
- Carry typed admission decisions, operation results, public record projections and errors through handlers and Rust consumers. Reuse existing domain enums and retain genuinely open data.
- Sweep all operation families, then registry, communication, observation, task and event read contracts; remove replaced JSON probing and string-state interpretation in each complete path.
- Preserve current wire shapes, optional-field semantics, same-ID replay, public redaction, execution certainty and shutdown guarantees. Do not support legacy record forms: claims require subject identity and categories are a closed vocabulary. Other contract changes require explicit approval.
- Scope later subscription/transport adoption in the design without bundling new codecs, endpoints or delivery policies into this refactor.

## Capabilities

### New Capabilities

None. This is an internal refactor with existing observable behaviour preserved.

### Modified Capabilities

None. Existing contracts remain authoritative; no new behavioural requirement or transport migration is introduced. This change declares `skip_specs: true`.

## Impact

`src/control_plane/{protocol,ops,store,lifecycle,containers,registry,communication,public_contracts,client,server}` and their consumers, including `src/ui_session`. Schema/SDK compatibility checks and existing lifecycle tests are the boundary evidence. No new dependency is assumed. Work is isolated from merged TUI acceptance, tmux observation/exit redesign and workspace recovery; no bookmark advances or deployment occur as part of planning.
