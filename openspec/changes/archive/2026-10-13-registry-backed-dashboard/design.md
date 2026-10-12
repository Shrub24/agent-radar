# Design

## Data path

`DaemonRuntime` already serves `mux.observe` for inventory and foreground evidence. It gains registry reads (`agent.list` paged, `agent.get` as needed) and spawn topology. The collector builds one `FleetObservation` per refresh: inventory first, then registry agents joined to panes, so every frame comes from one pass. Fact projection happens in the daemon client behind the existing provider seam, which keeps `ui.rs` unchanged except for the new freshness marks.

## Decisions

1. **No Herdr agent state.** Independence is tested by removal, not fallback. Under tmux this is the only possible behaviour; under Herdr the TUI also stops using Herdr's own agent state and metadata.
2. **Exact join.** Location plus process birth identity only. Registry reads add `process.claim` with `{boot_id, pid, start_ticks}` when a registration claims one, keeping `claimed` and `verification` unchanged. The collector explicitly samples each candidate foreground process independently of Herdr-derived agents, with bounded deduplicated work. A missing, ambiguous or mismatched tuple leaves the registration unresolved; verified PID alone is insufficient. Reject a new daemon join operation as unnecessary duplication of the existing observation seam.
3. **Unknown over idle.** Without a fresh execution snapshot the state is unknown, matching the existing "stale never silently idle" rule.
4. **Derivations stay in the dashboard.** Waiting, lost and settling are pure functions of published facts and freshness, so the registry stays a fact store.
5. **Bounded reads.** Registry reads are paged and byte-bounded; one slow read leaves the last good snapshot stale instead of blocking the loop, as the collector does today.
6. **Direct runtime.** Without a daemon the TUI is ordinary panes only. This is an explicit degradation, not a fallback.
7. **Protection is not close eligibility.** A registry candidate at a pane protects it from generic unmanaged close even when its join is unresolved. Exact bound spawn edges establish daemon managed-close eligibility, not a session, label or metadata flag. Preserve existing owner-routed control; unsupported registered or ambiguous targets refuse. Whole-tab containment must retain the same protection. This change does not add lifecycle dispatch or weaken close guards.

## Risks

- Per-refresh registry cost grows with registrations. Registration pruning is a separate daemon item; the bound keeps one refresh cheap and the cost is measured in verification.
- Until `agent-facts-channels` lands and publishers adopt it, model/context/awaited/background rows are unavailable. That is accepted and visible.
