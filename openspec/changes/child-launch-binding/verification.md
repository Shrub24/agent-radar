# Verification

## Status

All seven tasks are accepted. Launch context, exact child binding and the publisher bootstrap are implemented and verified. The change is not committed, pushed, synced or archived. Pi-extensions adoption is separate; managed lead restart remains unavailable pending Herdr's positive pane-readiness contract.

## Verified behavior

- Spawn carries optional absolute `cwd` and a bounded UTF-8 `env` overlay. Omitted fields preserve inherited behavior. Invalid/reserved-token values and unsupported context refuse before recording an edge or creating a pane.
- Herdr transports a single quoted command with guarded `cd`, preserves unrelated inherited variables, and preflights the encoded command against its 4096-byte text-input bound. Controlled-shell and real-Herdr checks cover empty/overridden values, shell metacharacters and literal argv; a failed `cd` prevents execution.
- Environment values and the private spawn token are absent from public operations, topology and failure responses. Malformed decoding uses value-free diagnostics. Same-ID requests changed only in private env or cwd refuse without redispatch or replacing stored content.
- `child_launch_context_v1` is advertised only when the selected backend supports context plus creation and launch.
- New token-consuming registrations persist the actual returned child `agent_id` in `bound.agent_id`. Identical and interrupted-ack retries preserve that identity. Conflicting IDs refuse; malformed stored IDs fail explicitly. Legacy absence remains readable without inferred identity.
- The publisher example captures/removes `PI_RADAR_SPAWN_TOKEN`, privately registers the exact child, and supports identical retry after an uncertain acknowledgement. The parent resolves the child through `spawn.get` and `agent.get`, without aliases. Binding without publication leaves execution unknown, not application-ready.

## Independent owner gates

The following passed on the final source/test worktree:

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed |
| `cargo test --locked` | 663 tests, zero failures; opt-in live checks skipped in this default run |
| `cargo build --locked` | Passed |
| `python3 tests/terminal_smoke.py target/debug/radar` | Both PTY scenarios passed |
| `python3 tests/agent_publisher.py` | Full canonical registry fixture and scripted spawn/bind/close smoke passed against disposable daemon state |
| `openspec validate --all --strict` | 19/19 passed |
| `git diff --check` | Passed |
| `nix flake check -L && nix build .#radar --print-out-paths -L` | Passed; owner retrieved certified background result `bg-1273`, exit 0 |

Nix output: `/nix/store/l2nh9p63zwgg040hqm19dkw9qaqjzccq-agent-radar-0.1.0`.
The owner rerun reused the same checked derivation built by the worker, whose sandbox check phase passed. Nix covers Rust tests/build, not Python or live Herdr checks. Validation is native x86_64-linux; the default command omitted aarch64-linux and Darwin systems. The existing `homeManagerModules` output warning remains.

## Real Herdr evidence

The worker completed and reported both opt-in checks:

```sh
RADAR_HERDR_SMOKE=1 RADAR_HERDR_SMOKE_WORKSPACE=w1H \
  cargo test --locked --test runtime a_real_herdr -- --nocapture
```

The tests shared the configured Herdr server, creating only fresh nonfocused tabs in the explicitly selected workspace. Cleanup checked exact tab-scoped containment and closed only returned test resources. The direct launch check observed explicit cwd, environment overlay and literal argv. The public-daemon check registered its parent at `w1H:p8`, spawned child `w1H:p9`, observed durable create/launch completion and exact `bound.agent_id`, then confirmed durable `child.close`, identical replay and unchanged stored topology/publications. The owner reviewed the harness and reran the non-live gates; these valid live operations were not repeated merely for reporting.

Earlier failures were test defects, not demonstrated production routing defects:

1. `spawn.get` flattens edge fields; indexing a nonexistent `spawn.edge` produced null binding and invalid null-to-null comparisons. The corrected test checks non-null request, parent and bound identity, and compares stored fields independently of changing observation freshness.
2. A launch oracle assumed the Rust test's cwd instead of the pane's inherited cwd. The final test explicitly supplies and verifies cwd.
3. Workspace-created hooks from the installed `hhdebb.herdr-radar` plugin added an anchor outside Herdr's initial returned resources. Fresh-tab tests avoid that hook without changing plugin configuration.
4. The first tab-scoped cleanup still compared the whole shared workspace. It now filters by exact tab ID; an offline regression permits unrelated tabs but refuses a foreign pane inside the owned tab.

The worker verified and removed test-created tabs `w1H:t3`/`t4` and cleaned the final-run tabs. The owner then separately removed the original disposable fixture workspace `w1H`, after fresh complete inventory showed only its original root `t1/p1` and independently identified hook-created anchor `t2/p3`; the root was an unoccupied shell and the anchor's executable/process generation were rechecked. A fresh snapshot confirmed `w1H` absent. Historical `w1E`/`w1F` absence is not used as evidence of their origin. No fixture resources remain from the final run; existing fleet resources and plugin configuration were not altered.

## Pi-extensions adoption handoff

Start with `docs/daemon.md`, `docs/control-plane.md` (spawn contract), `docs/agent-registration.md` (bootstrap/topology), and `examples/agent-publisher/publisher.py`. `docs/agent-registration.fixture.jsonl` is validated in full by `tests/agent_publisher.py`.

- Check compatible protocol plus declared creation, launch and `child_launch_context_v1` support before context-bearing spawn. This checkout has no published commit pin yet.
- Publish fresh parent session context and name both the exact parent registration and `parent_session_id`; never select a parent by searching session UUIDs.
- Supply resolved executable/argv, absolute cwd and required `PI_HERDSMAN_*` environment values as an overlay. Do not supply Radar's reserved token field through env.
- In the child, capture/remove the delivered token and privately present it as `spawn_token` to `agent.register` with the child's exact source/incarnation/process claim. On uncertain acknowledgement retry identical registration; never persist the token in owner sidecars or facts.
- Read the flat `spawn.get` topology's `bound.agent_id`, then `agent.get` that exact ID. Registration correlation is not assignment/application readiness; keep independent execution and owner-assignment evidence separate.
- Once bound, use the existing `child.close` contract. Never fall back to Herdr or resend a possibly-dispatched spawn after uncertainty.

## Limits

No Herdsman deployment or launch migration was performed. These checks establish launch transport and registration correlation, not a general shell-ready acknowledgement. Lead/child restart, resume, process termination and assignment-authority changes are outside this change.
