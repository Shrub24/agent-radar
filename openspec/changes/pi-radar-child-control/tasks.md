Depends on `unified-launch` for continuation and the launch chain.

## 1. Resolve a subject to its child

- [x] 1.1 Superseded by `unified-launch`: a continuation reserves on the existing subject by construction.
- [ ] 1.2 Decode `bound_at` on the edge; resolve a subject to its current edge and registration per operation, with no cross-restart cache.

## 2. Subject-addressed control

- [ ] 2.0 `children()`: the caller's children from its own granted subject, with subject, session id, state, pane and label.

- [ ] 2.1 Address inspect and observe by subject.
- [ ] 2.2 Address assign and retire by subject, keeping the last assignment per subject and republishing it when the registration changes.
- [ ] 2.3 One test through the existing SDK fakes: a child replaced under the same subject is reached by the recorded key and carries its assignment.

## 3. Release

- [ ] 3.1 Remove `registration_id` and `bound_process` from the launch result and inspection evidence after the Herdsman cutover; pi-radar minor release; update the brief.
