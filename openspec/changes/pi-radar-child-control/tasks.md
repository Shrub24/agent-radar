## 0. Continuation and the fresh-launch hole (daemon)

- [x] 0.1 Confirmed from the code: a fresh launch reserves a placeholder subject, and a child that opens a released subject's session is granted the placeholder (decision 5).
- [ ] 0.2 `spawn.create` `continue_subject_id`: validation, session from the subject record, reservation on the subject; published schema.
- [ ] 0.3 First presence refuses a placeholder subject a session another subject holds a record for, and refuses a continued child reporting another session.
- [ ] 0.4 Tests at the owned seams: a continuation binds its edge to the existing subject, and a fresh launch opening an owned session is refused while manual resume still reattaches.
- [ ] 0.5 Wire revision 7, `RADAR_MINIMUM_REVISION` 7; SDK `launchRadarWorker` gains `continues?: subject_id`.

## 1. Resolve a subject to its child

- [x] 1.1 Superseded by 0.1–0.2: a continuation reserves on the existing subject by construction.
- [ ] 1.2 Decode `bound_at` on the edge; resolve a subject to its current edge and registration per operation, with no cross-restart cache.

## 2. Subject-addressed control

- [ ] 2.1 Address inspect and observe by subject.
- [ ] 2.2 Address assign and retire by subject, keeping the last assignment per subject and republishing it when the registration changes.
- [ ] 2.3 One test through the existing SDK fakes: a child replaced under the same subject is reached by the recorded key and carries its assignment.

## 3. Release

- [ ] 3.1 Remove `registration_id` and `bound_process` from the launch result and inspection evidence after the Herdsman cutover; pi-radar minor release; update the brief.
