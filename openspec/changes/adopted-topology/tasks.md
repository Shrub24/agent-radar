# Tasks

## 1. Focus

- [x] 1.1 Refuse a tmux focus with no attached client before anything changes, naming that case; check it only after the handle validates. Real-tmux test: refused and unchanged without a client, completed with one.
- [x] 1.2 Skip `mux.focus` in the store's unresolved search. Test an unknown focus not blocking a later mutation while an unknown close still does.

## 2. Scope

- [x] 2.1 Show a registration only when its pane is in this inventory or the daemon holds a spawn edge for it; never one of another backend or server. Tests for each case and the removal of the unplaced group.

## 3. Adoption

- [x] 3.1 Add `containers::adopt`: observed containers for the server's own places with parent links, under a verified incarnation, appending nothing for a recorded place.
- [x] 3.2 Run adoption and reconciliation from the sweeper, edge-triggered on the incarnation and place handles. Tests: first sight adopts, a repeat appends nothing, a vanished place is marked, an unverified incarnation adopts nothing.
- [x] 3.3 Confirm an adopted container is deletable under the existing confirmation and refused for reload. Socket test through the daemon.

## 4. Documentation and gate

- [x] 4.1 Update `docs/daemon.md` and `docs/control-plane.md` for adoption, the no-client refusal and the visibility rule.
- [ ] 4.2 `managed-topology` is not archived, so its provenance requirement does not yet exist as a main spec; archive `managed-topology` before this change so its modification applies.
- [ ] 4.3 Run fmt, strict Clippy, the locked tests, strict spec validation and the checked Nix build; record real exit codes.
