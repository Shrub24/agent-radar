## 1. Edge carries the child subject

- [x] 1.1 Confirm from the bind path that the subject is known when a managed child's edge binds (reserved attachment for the launched subject); if it is not, stop and revise decision 4.
- [x] 1.2 Add optional `subject_id` to `BoundChild`, write it at bind, expose it through the public spawn DTO and generated schemas, and decode it in the SDK. One test: a bound edge carries the subject that presence later grants.

## 2. Subject-addressed child surface

- [ ] 2.1 Resolve a subject to its current registration from `spawnList` inside pi-radar, with no cross-restart cache; report an unbound or legacy edge as not yet addressable.
- [ ] 2.2 Address assign, retire, inspect and observe by subject id, and return the subject (not registration or bound process) from a launch.
- [ ] 2.3 Expose the handoff sender as a subject id.
- [ ] 2.4 Take the launch parent from the publication; drop identity from `RadarLaunchParent`; refuse with no granted subject.
- [ ] 2.5 One test through the existing SDK fakes: a child replaced under the same subject is still reached by the recorded key.

## 3. Release

- [ ] 3.1 Regenerate contracts, bump `RADAR_RUNTIME_VERSION` to 3 and pi-radar to 0.8.0, update the README and changelog.
- [ ] 3.2 Rewrite the Herdsman brief section: pin 0.8.0, the call sites that change, the persisted-entry migration of decision "Migration", and deleting `support-radar.ts`.
- [ ] 3.3 Run fmt, strict Clippy, the Rust suites, SDK checks and strict OpenSpec validation; tag `pi-radar-v0.8.0` only after the Herdsman brief is sent.
