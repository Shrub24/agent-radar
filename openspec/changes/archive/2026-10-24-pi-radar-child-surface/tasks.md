## 1. Edge carries the child subject

- [x] 1.1 Confirm from the bind path that the subject is known when a managed child's edge binds (reserved attachment for the launched subject); if it is not, stop and revise decision 4.
- [x] 1.2 Add optional `subject_id` to `BoundChild`, write it at bind, expose it through the public spawn DTO and generated schemas, and decode it in the SDK. One test: a bound edge carries the subject that presence later grants.

## 2. Launch surface

- [x] 2.1 Take the launch parent from the publication; the launch passes `pane_id`, drops `RadarLaunchParent`, and is refused before any request when the publication has no granted subject or the registered pane differs.
- [x] 2.2 Return the child's `subject_id` from a ready launch; an edge bound without one is unresolved.
- [x] 2.3 The handoff sender is matched by subject id: the decoded sender already carries it, so this is documented in the README and the Herdsman brief with no code change.
- Moved to `pi-radar-child-control`: subject-addressed assign, retire, inspect and observe.

## 3. Release

- [x] 3.1 Regenerate contracts and update the README. The release ships with `unified-launch` and `pi-radar-child-control` as pi-radar 0.9.0 and Radar 0.4.0 (wire revision 7); 0.8.0 and 0.3.1 were never tagged.
- [x] 3.2 Rewrite the Herdsman brief section: pin 0.9.0, drop the parent's `registration_id` and the handoff registration match, key children by `subject_id`, and delete `support-radar.ts`. (Items 7 to 13 of the brief, with child control and continue.)
- [x] 3.3 Tagged and published `pi-radar-v0.9.0` and `radar-v0.4.0`; superseded by 0.10.0 / 0.5.0 (wire revision 8).
