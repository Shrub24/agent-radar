## Addressed agent messages replace derived notifications

**Id:** 4b1f6c2e-9d3a-4c58-8f71-2a6d5e9c4b07
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** design conversation, 2026-10-29

`agent-attention` introduced one durable, producer-addressed message lane with
typed bodies, a frozen set of subject recipients, per-recipient receipts and
derived per-subject attention. The handoff that already existed became its first
body kind rather than staying a special-case mailbox, and the old envelope, store
and capability were deleted in the same change.

**Reason:** agents need addressed communication, not daemon facts to interpret.
An earlier draft hung attention on the raw event journal: any journal movement
would have woken every agent, and even a subject-filtered journal cursor left
pi-radar inferring notification intent from lifecycle internals. Notification is
a producer decision with a recipient, so it belongs in a message; raw events stay
the daemon's record of its own transitions.

**Rejected alternatives:** a global or subject-filtered event wake (a consumer
guessing which transitions matter, and unrelated agents waking on other
subjects' work); keeping the handoff store and adding a count to it (the next
message kind would need a second flow); acknowledging on socket delivery (content
lost if the agent dies before capturing it); an always-on subscription per live
process (one socket per idle agent for no benefit).

## Attention is a mailbox fact, not a read position

**Id:** 8c2d7a41-6f5e-4b93-a1d8-5e30b94c7d12
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** design conversation, 2026-10-29

Heartbeat replies and the optional subscription both carry the same
`{revision, pending}` for one verified subject. The revision is opaque and is
computed from that subject's pending set plus a per-instance token; the pending
count, not revision inequality, is what makes a retry happen.

**Reason:** an idle agent must need no connection, so the heartbeat reply is
where it learns of work; a working lead wants the same state pushed. Tying both
to one derived fact means a lost or refused push cannot lose work, and a queued
hint is not a position a consumer could skip past. Mixing in a per-instance token
keeps a revision from one daemon from ever comparing equal to one from another.

## Only continuous literal idle releases the subscription

**Id:** 0f7a9d63-2b48-4e17-9c05-6a8d41e7b3f9
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** design conversation, 2026-10-29

A process opt-in closes its attention subscription after 30 seconds
continuously in the agent runtime's literal `idle` status. `waiting` — on a user,
a tool, a questionnaire or a child — does not start the timer, and any activity
cancels it.

**Reason:** waiting for a child is exactly the state in which a handoff matters,
so dropping the connection there would delay the message being awaited. The hold
is longer than one heartbeat so a brief pause does not churn the connection.

**Rejected alternative:** automatic subscription for every live process, which
reintroduces one socket per agent including those asleep.

## Retired communication records are refused, not migrated

**Id:** c6e05b28-4a71-4d3e-8b96-17f2c0a95e63
**Type:** constraint
**Status:** active
**Evidence:** confirmed
**Source:** design conversation, 2026-10-29

A state directory holding records from the retired handoff mailbox makes the
message store refuse to open, naming the directory and asking for an explicit
drain or reset. There is no implicit migration, dual-write period or compatibility
adapter, and no silent discard of pending content.

**Reason:** pending handoff content is someone's work. Converting it silently
would guess at a shape it never had, and dropping it would lose the only copy.
The lane changed and its cutover is explicit.

**See:** lifecycle.md#a-confirmed-deletion-closes-over-a-survivor-and-never-records-an-unverified-exit — 5c0a7f43-9c2e-4d31-8a6b-2f0e5d7c9b14 — as of 2026-10-29
