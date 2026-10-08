# Agent registration and direct publication

For the overall surface, lifecycle and adoption path, start with the
[daemon consumer guide](daemon.md).

**Contract version 1.** This is the normative wire reference for direct agent
registry producers. The canonical request/response exchanges are in
[`agent-registration.fixture.jsonl`](agent-registration.fixture.jsonl); the
stdlib-only reconnecting owner publisher is
[`examples/agent-publisher/publisher.py`](../examples/agent-publisher/publisher.py),
exercised against a disposable daemon by
[`tests/agent_publisher.py`](../tests/agent_publisher.py). The fixture uses
`<daemon-time>` and `<opaque-uuid>` placeholders for generated values; fixture
validation executes its request shapes against the daemon and checks those
response templates. `agent.get` and `agent.list` differ intentionally: writer-facing channel replies expose `writer.handle`, whereas public get/list channel facts omit both the current writer handle and accepted-snapshot handle.

This documents the durable agent-registration records and direct publication
channels served on the trusted control socket. They are a **foundation**: private
storage, writer fencing and freshness rules are implemented and tested. Registry
operations are independent of mux capabilities and make no Herdr calls. They
perform no lifecycle controls.

The registry is backend-independent. It holds agent identity separately from
mux inventory and durable mutation records, and mutable channels separately from
identity. Writing or publishing dispatches no physical control and enters no mux
mutation lane.

## Registration record

One registration is one JSON file, `<state root>/agents/<agent_id>.json`, mode
`0600`:

```json
{
  "version": 1,
  "agent_id": "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22",
  "registered_at": "2026-01-01T00:00:00.000Z",
  "request": {
    "source": "herdsman",
    "incarnation": "1c2d3e4f-5678-4abc-9def-0123456789ab",
    "session": "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d",
    "owner": null,
    "run": null,
    "label": "worker",
    "location": {
      "backend": "herdr",
      "instance": null,
      "workspace": "wA",
      "tab": "wA:t1",
      "pane": "wA:p1"
    },
    "process": { "boot_id": "b3e0…", "pid": 4242, "start_ticks": 99311 },
    "launch": {
      "executable": "/usr/bin/pi",
      "argv": ["--resume"],
      "cwd": "/home/dev/proj",
      "session": { "uuid": null, "path": "/home/dev/.pi/sessions/…" },
      "provenance": "herdsman",
      "revision": "7"
    }
  }
}
```

The daemon-issued UUID is the durable identity key. Source plus publisher
incarnation define an idempotent registration; session is context, not identity,
so duplicate attaches stay distinct. `owner`, `run`, `label`, backend-qualified
location, claimed process identity and explicit private launch specification
are optional facts. The public registration projection has no fields for launch
executable, argv, cwd or session path; it exposes launch availability and
revision only. Process identity is a strict registry-wire shape but a claim, not
verification.

Registration holds **identity and configuration only**. Mutable activity,
waiting reason, last outcome and actions belong to publication, not to this
record. A `state` field is refused as unknown; unknown vocabulary is preserved
where the publication channel accepts it. Registration retry with identical
source/incarnation/content returns the same handle; conflicting content refuses.

Records are bounded (64 KiB, 1 KiB text fields, max 256 argv entries), private
(mode 0600), written atomically and read only as bounded regular non-symlink
files. Corrupt, oversized, wrong-version and filename-mismatched records fail
explicitly. Registration listings are bounded to 100 and use stable agent-id
order. One daemon owns the state root; independent `Registry` instances over one
root are unsupported for concurrent writes.

## Socket methods

The trusted control socket serves `agent.register`, `agent.acquire`,
`agent.publish`, `agent.retire`, `agent.get` and `agent.list`. `ping` advertises
`agent_registry` regardless of mux backend. The methods are independent of the
physical backend and do not call Herdr. All method parameter objects reject
unknown fields. The exact canonical exchanges are in the JSONL fixture; field
semantics and transitions follow.
### One channel is one record

A channel is one file, `<state root>/publications/<agent_id>.<channel>.json`,
holding the current `writer` binding and an optional `snapshot`. **Acquire,
publish and retire each rewrite that single record atomically**: a mode-`0600`
temporary sibling is flushed, renamed into place, and the containing directory is
synced before the call returns. There is no separate writer file a partial write
could leave behind, so a failed write leaves the previous binding, sequence and
snapshot byte-identical and an identical retry still succeeds.

`AcquireRequest` names a target `agent_id`, a `channel`, a `PublisherIdentity`
(`source`, publisher `incarnation` UUID, optional `reporting_owner`) and an
optional expected incumbent:

- **Absent `replace`** acquires only a channel with no writer. An identical retry
  by the same publisher/target/channel returns the **same** binding; a different
  publisher is refused (`replacement is explicit`).
- **Present `replace`** names the exact incumbent `generation` and `handle` the
  caller observed. If the current writer no longer matches, the call is refused
  (`incumbent writer changed`), so a replayed or delayed replacement cannot
  overwrite a newer writer.

A `registration` `agent_id` is a target identity, **not** a writer credential. A
channel writer is a separately issued opaque UUID bound to actual publisher
source, publisher incarnation, reporting owner, target agent ID, channel,
generation, sequence and retirement state. The **publisher is not looked up**: a
publisher restart is a new incarnation UUID and needs no second agent
registration. A genuinely new subject/process gets a new agent record; session
or pane reuse never implies continuity. Replacement keeps the same target agent
ID and never creates a fake second child.

### Provenance and replacement

`execution` and `assignment` are separate streams. Execution is normally
authored by the target agent; assignment may be authored by its registered
owner. Writer-facing publication replies name the actual writer handle and
source/incarnation. Public get/list channel facts omit the fencing handle and
expose source/incarnation/generation, separately naming the reporting owner —
they do not copy the target's registration source and call that the publisher.
The two complete snapshots are never merged.
Each snapshot contains:

- `activity`;
- optional, separate `waiting_reason`;
- optional `last_outcome` (`result` and optional `detail`), distinct from activity;
- bounded `actions` the publisher advertises, which certify neither completion
  nor restart eligibility.

All words are free-form bounded strings, so unknown vocabulary survives; object
shapes reject unknown fields. An idle process with a failed last turn and
unsettled owner work can retain all of those facts separately.

A channel that is acquired but not yet reported exposes `snapshot: null`: no
invented "unreported" activity is presented. When a writer is replaced, the
binding changes but the accepted snapshot keeps its **own** source, incarnation,
handle, generation and sequence, and reads `freshness: "stale"`: the report was
true when made and is never relabelled as the successor's. Before its first
accepted snapshot the successor's sequence is zero.

### Sequences, leases and freshness

A publish requires a positive per-generation sequence. Lease defaults to 30
seconds and is bounded to 1–300 seconds. Strictly newer content replaces the
whole snapshot and the writer's stored sequence in one record write, before
acknowledgment. Equal-sequence identical content (snapshot, lease and observation
time) is returned unchanged and does **not** refresh the lease; equal sequence
with conflicting content and older sequence refuse. Heartbeat requires a newer
sequence. A failed write consumes no sequence, so an identical retry succeeds.

Producer `observed_at` is provenance, never authority for freshness. Daemon
receipt time and expiry control the lease. Every registry open creates a random
serving epoch, independent of the wall clock: persisted snapshots are stale
across daemon restart even if startup timestamps are equal or the clock moves
backwards. An unreplaced handle can reconnect with a newer sequence. Replaying
old content cannot make restored facts fresh.

Freshness is `fresh` only when the stored snapshot was accepted in the current
serving epoch, belongs to the current writer generation/handle, that writer is
not retired, and the lease has not expired. It is `stale` when restored, when the
writer was replaced, when the writer retired, or when the lease expired.

A fresh writer cannot be taken over. Explicit replacement requires retirement or
daemon-clock lease expiry, persists a new opaque handle/generation before
acknowledgment, and fences the old handle even if it submits a larger sequence.
An acquired writer that never reported has no live lease and may be replaced,
but only by naming its exact incumbent. A successor that has not reported is not
fresh merely because it sits beside the previous generation's unexpired facts. Stale and retired mean only stale and
retired: neither proves process exit, assignment completion or safe restart.

### Persistence, ownership and bounds

The channel record is a bounded (64 KiB), versioned private JSON file written by
atomic sibling-temporary replacement, mode `0600`, file and directory flushed
before acknowledgment. Each snapshot has at most 16 actions and text fields at
most 1 KiB. Corrupt records are errors, not successful empty reads. A local
transition mutex serializes writers within one `Registry`; the state root has one
owning daemon, with no cross-process locking or cross-user authorization system.

## Process identity verification

`agent.get` and `agent.list` read the immutable registration claim, then invoke
the verifier outside registry/store locks. Verification is bounded by a worker
deadline; a timeout or daemon stop reports `unavailable`, never `absent`. The
daemon runs at most `MAX_VERIFICATION_JOBS` (4) verifier calls at once, shared by
every connection and page: exceeding the bound and waiting for a busy slot are
also `unavailable`. A call whose deadline expires or whose connection ends keeps
its slot until the verifier actually returns, so repeated requests cannot
accumulate blocked verifier threads, and a request arriving with no slot starts
no work at all. A verifier that blocks forever therefore holds its slot, and up
to that many uncancellable verifier threads can outlive a stop request; shutdown
does not join them, and each worker holds only the claimed process identity and
the verifier seam. The `Registry::verify_process(agent_id, verifier)` local API
remains available for
deterministic tests. No registry lock is held across procfs or an injected verifier. The result is
`verified` only when the full boot ID, PID and `/proc/<pid>/stat` start-tick tuple
matches; a known missing process is `absent`; a live PID with another boot ID,
PID or start time is `mismatched`; missing claim, unsupported procfs, permission
failure and other unreadable evidence are `unavailable`. No claim is not evidence
of absence. The production `LocalProcfsVerifier` reads local Linux procfs, using
the same stat parser as the process sampler; non-Linux platforms report
unavailable.

Process evidence is independent of publication freshness. A verified process may
have a stale publication lease, and an absent process may have a fresh publisher
snapshot. Neither value infers lifecycle eligibility or triggers a physical
operation. Tests inject a deterministic verifier; no live mux or process-control
operation is performed.

The endpoint contract exposes explicit writer acquire and replace (with expected
generation/handle), publish and retire under the opaque handle, plus bounded
public channel reads/listing and separately labelled process verification. Do
not overload immutable `agent.register` with succession. Automatic replacement
policy, process-death inference from a stale lease, launch/stop/resume,
task-completion certification and physical mux controls remain out of scope.

## Reconnecting owner-publisher example

Run the example against an already-running daemon with a socket path that passes
the daemon's trust check:

```sh
python3 examples/agent-publisher/publisher.py /run/user/1000/agent-radar/control.sock --once
```

The script is dependency-free Python 3.10+ and demonstrates only owner assignment
projection. A production publisher stores its immutable registration and
publisher-incarnation identity outside the process, repeats identical
`agent.register` content after reconnect, and calls `agent.acquire` without
`replace` to recover the same current writer. The example process generates an
incarnation once for its lifetime; it is illustrative, not durable publisher
state. It publishes a complete snapshot and uses a newer sequence for every
heartbeat. A lost response may be retried with exactly the same sequence,
snapshot, lease and `observed_at`; identical replay returns the accepted report
unchanged and does not extend expiry. Advance sequence to renew the lease.

A different writer is never adopted implicitly. A publisher may replace only
when it has explicitly observed incumbent generation and handle and sends both
under `replace`; replacement is valid only after incumbent retirement or lease
expiry. A refusal that says the writer/handle changed means this producer has
been fenced: stop and reconcile with the owner, do not retry with `replace` or
new identity. This example contains no Herdr invocation, Herdr token parsing,
subprocess or physical lifecycle call. Herdsman's existing owner assignment
projection can map to `channel: "assignment"`; target process activity belongs
on the separate `execution` channel with its own publisher identity and sequence.

Run the full disposable-daemon contract smoke with:

```sh
cargo build --locked
python3 tests/agent_publisher.py
```

It uses a private temporary socket/state root, starts and bounds daemon children,
checks public launch redaction and unknown vocabulary, exercises replay,
heartbeat, lease-expiry replacement, old-writer fencing and restart epoch
freshness/reconnect, then stops the child in `finally`. It is not live publisher
or fleet integration. The pi-extensions port is a separate follow-up: map
Herdsman's current owner projection to assignment and its child execution facts
to their separate channel, retain optional legacy metadata reporting only as an
intentional compatibility bridge, and do not add process execution or recovery
policy here.

## Errors, trust and limits

Switch on protocol error codes, not message prose: `bad_params` is invalid shape
or bounds, `not_found` is missing registration/writer, `refused` is a fenced,
conflicting or invalid transition/store record, and `internal` means persistence
or serving failure (a lost reply may leave the caller uncertain). Retry register
with identical content; retry an uncertain publish only with identical
sequence/content. Never infer that an absent response did not persist.

The publisher validates a socket path before connecting: parent and socket must
be real non-symlinks, the parent must be owned by the effective uid with mode
exactly `0700`, and the socket must be owned by that uid. Socket trust is not
same-user authentication. Writer handles fence cooperating producers but are
not secrets against hostile local code; the state is private `0600`, yet another
process under the same uid can read it. Do not expose the socket remotely.

Lease expiry and daemon epoch changes mean only that published facts are stale;
process verification is independent and none of registration, actions,
retirement, replacement, freshness or process verification establishes lifecycle
eligibility, assignment completion or authority to launch/stop/resume. No
publisher API here executes physical controls.

The existing mux `report` method and its Herdr-forwarding behaviour are
unchanged.
