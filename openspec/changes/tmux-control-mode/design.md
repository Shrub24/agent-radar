# Design

## Context

The shared UI session distributes selection, navigation settings, canonical rows and command interactions. Commands already carry displayed row identities and the actor freezes their targets. tmux focus executes through the daemon adapter, but native tmux focus changes are not the shared cursor's authority.

The supported model is one unified operator tmux client, regardless of the number of Radar views. Nested tmux stays: the agent server contains no sidebar panes. See proposal.md for motivation and scope.

## Goals / Non-Goals

Preserve the existing shared distribution and command machinery while separating terminal focus from inspection. Notifications improve responsiveness but never replace evidence reads. Do not introduce a second shared cursor, per-client focus maps, a general observer framework or a domain-actor rewrite.

## Decisions

### 1. One server-wide observer, attached lazily

Use one control-mode child with read-only, ignore-size and no-output, supervised on the daemon's async runtime. Wait when there are no sessions; never create a placeholder. Reattach when its session disappears, with bounded backoff after failure. Verify the owned serving incarnation before attachment and discard old-incarnation work on replacement. Shutdown cancels and reaps the child.

Disposable tmux 3.8 experiments demonstrated server-wide notifications from an observer attached to one session. This chooses one observer rather than a per-session fallback. Finish lifecycle and flag verification before depending on it. Ordinary control notifications are the initial mechanism; do not install speculative monitors or extra hooks. Any necessary additional hook must use an owned slot without replacing user hooks.

### 2. Hints trigger reads; timers recover losses

Client, session, pane, window and layout events request coalesced refresh work. A refresh reads operator clients, their active location, zoom and inventory. Keep the existing sampling/demand policy; ensure focus-following views supply the demand they need. Changes during an in-flight read leave one further refresh pending. Subscribe before the baseline read and preserve a dirty hint during it, so establishment does not lose a transition.

Parsing and buffering are bounded; unknown notifications are ignored and unexpected pane output is discarded without logging. No layout parser is needed to determine zoom: read its format field. Observer health includes connection state, last notification and loss reason. Loss retains timer-based reads and bounded reconnect, never an inference of no change.

Alternative rejected: reduce notifications directly into terminal facts. They omit context, can be lost, and cannot establish process exit or pane absence.

### 3. One typed unified focus fact

Represent available focus with workspace, window and pane identities and zoom state. Distinguish no operator, unreadable/unavailable evidence and unsupported multiple operators; none licenses an implicit target. Retain last-good information only as explicitly stale display data.

Filter control-mode clients before operator counts and focus-client selection. More than one operator client refuses focus-changing operations rather than choosing the most active or target-session client. Explicit non-focus commands remain governed by their normal evidence and eligibility checks. A server read, not an observer's attached session, identifies operator focus.

Alternative rejected: per-client maps and follow selectors. They expand the product beyond its supported single-client model.

### 4. Evolve shared selection into focus; extract local selection

Reuse existing shared-selection publication/adoption wiring for shared focus, changing both names and authority. tmux reads establish focus. Focus requests use the existing daemon executor; request acceptance is not observed success. Adopting a focus observation never executes a focus request.

Extract browsing movement, jumps, mouse selection and selection anchoring into the frontend. Details read that local selection. Remove actor-owned browsing-move actions, shared-cursor fallbacks and global-selection reconciliation. No permanent transitional cursor or parallel focus-distribution path.

The actor still owns filters, folds, pane mode, ordering, projection and interactions. Each frontend preserves its selected identity through updates and reconciles locally when canonical membership changes. Local navigation uses the canonical row ordering, not a lagging independently rebuilt tree; available row facts supply kind-based jump information. A row not locally explained must never cause selection or command fallback to another identity.

Alternative rejected: universal cursor equals focus. Inspection and details do not imply terminal-switch intent. Also rejected: deleting the distribution path and adding a separate shared-focus path.

### 5. Following is a local event-driven bridge

Sidebar defaults on; dashboard defaults off. Provide a local toggle, expose the mode in help, and use a focus glyph distinct from selection highlight. Enabling following or adopting a reconnect baseline selects the current focus if visible. A subsequent changed focused place selects its row; repeated identical focus, zoom-only changes and unrelated publications do not undo browsing.

Resolve a focused pane to its canonical terminal/agent row, not an arbitrary task or historical subject sharing a place. If it is hidden or cannot be represented, indicate focus outside the listing; do not change shared filters/folds or select a neighbour as though focused. A later projection that makes that same focus representable may complete the pending follow once, but must not repeatedly reset deliberate browsing. Clear stale focus indications on loss.

Following only copies focus into local selection. Local movement never turns into a focus request; this prevents feedback loops. The exact toggle key and glyph follow existing binding and rendering conventions during implementation.

### 6. Resolve once, then freeze

All row-dependent TUI actions supply their locally selected row, including configured Launch and palette/mouse commands. If no row is selected, the TUI refuses locally instead of accidentally invoking implicit focus. Missing row in the command contract means use observed focus, for future global hotkeys and other intentional implicit callers. Targetless commands remain targetless.

The actor validates an explicit identity against its canonical tree, or resolves the focused place through the existing command eligibility rules when omitted. It then uses one plan/freezing path. No invalid explicit row falls back to focus. No absent, stale or ambiguous focus licenses implicit execution. If focus has no eligible canonical target, refuse rather than fabricate one.

Resolve before preparation or confirmation. The plan retains target, execution identities and preconditions; configured Launch supplies command/environment preparation without changing the target. Confirmations and detached presenters show that plan, not anyone's current cursor. Execution revalidates domain preconditions without retargeting.

Preserve frontend-owned no-client terminal recovery for an explicit focus request, retrying the same captured target. An implicit request without observed focus cannot use that recovery to guess a destination.

### 7. Gates fence actions, not independent browsing state

Keep bounded ordered submission, displayed-gate checks, receipt correlation and coherent publication adoption for shared actions. Local cursor movement needs no actor acknowledgement. A command immediately after local navigation captures the new local identity.

A newer gate fences stale queued actions and key sequences even while replies/content are pending. Gate adoption does not restore a global browsing selection. Reconcile local selection only for projection validity or the explicit follow policy. Popup target and action receipts survive coalescing as before.

### 8. Keep lifecycle evidence and packaging changes narrow

Require 3.8 from the serving server version, not merely the local executable. Audit current Home Manager and flake support before adding an override; use packaged 3.8+ if already available. No older-version fallback.

Exit hooks, remain-on-exit, process verification, absence proof and scope handling stay unchanged. OSC 133 notifications, if received, are wakes only. Do not add new monitors for foreground state without a demonstrated missing wake requirement.

## Verification evidence and remaining checks

Recorded disposable tmux 3.8 experiments found:
- zero-session control attach fails; lazy attachment is necessary;
- notifications cover sessions other than the observer's attached session;
- client-session-changed names the client and fires for first attach and session switches;
- client-detached, session-window-changed, window-pane-changed, layout-change (including zoom), renames and sessions-changed provide relevant wakes;
- control clients appear in list-clients and must be excluded;
- no pane-exited notification replaces exit hooks.

These are experiment findings, not a claim that the observer exists. Record reproducible commands/results during implementation and finish size/selection invariance, no-output, destroy-unattached, detach-on-destroy and attached-session loss checks. The old audit's local-3.7c and missing-feature claims are stale and must not be treated as current findings.

## Risks / Trade-offs

- Focus may briefly lag input → coalesced reads plus timer recovery; no unbounded focus-command queue.
- State changes across reads → validate the sampled client/location relationship, rerun on dirty hints, and report unavailable rather than synthesize a mixed location.
- Shared filtering hides focus → explicit outside-listing indication, no forced global filter mutation.
- Canonical rows may not have local detail → preserve row identity; never retarget through a neighbour.
- Observer-only attachment affects tmux lifecycle → verification gate before building on attachment semantics.
- Older frontend contract → wire revision change and coordinated daemon/frontend upgrade, not legacy adapters.

## Migration Plan

1. Inventory selection writers/readers, row target defaults, gate reconciliation and public DTOs; finish 3.8 verification.
2. Land observer, refresh seam and typed focus facts with their tests and docs.
3. Cut over shared selection to focus and extract frontend selection in one coordinated slice. Migrate all callers and delete replaced actions/fields in that slice.
4. Complete follow controls, markers and behaviour tests; verify from two Radar views sharing one operator tmux client.
5. Regenerate contracts and update wire revision for the incompatible UI contract. Deploy matching daemon and TUI; rollback requires matching versions too.

Archive dependency: sidebar-mode and shared-ui-session introduce baseline UI capabilities; unified-tui-publications and duplex-tui-attachments modify them. Sync predecessors in dependency order before applying this change's MODIFIED ui-session, ui-interactions and tui-publications requirements. Do not rewrite their historical deltas to disguise this change or archive out of order. Existing main-spec deltas likewise target the post-predecessor state.

## Open Questions

No unresolved product/ownership decisions block planning. Exact refresh gap, retry cap, parser line bound and follow-toggle key are implementation choices to select from existing conventions and verify, not new configuration surfaces.
