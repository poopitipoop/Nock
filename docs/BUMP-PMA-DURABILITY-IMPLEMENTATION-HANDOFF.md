# Bump PMA Durability + Integrity Implementation Handoff

This document is the implementation memory for the first trustworthy durability slice and the follow-on snapshot/integrity work. The goal is that implementation can continue autonomously from here without losing critical constraints, repeating false starts, or regressing the intended durability contract.

## Current Target

The target design remains the one in `docs/BUMP-PMA-SNAPSHOTS-EVENT-LOG-INTEGRITY-PLAN.md`:

1. PMA is still the live runtime image.
2. Accepted events become durable only when committed to an append-only SQLite event log.
3. PMA snapshots become immutable files with manifests and explicit verification.
4. Boot becomes `verified snapshot -> replay event log -> checkpoint/state-jam fallback`.

This handoff covers:

1. The invariants that must not be violated.
2. The existing codepaths that matter.
3. The concrete first implementation slice being landed now.
4. The remaining work after that slice.

## Non-Negotiable Invariants

1. Caller-visible live `Ack` must not be emitted before the accepted event is durably committed to SQLite.
2. The SQLite event log is the first authoritative durability boundary. PMA durability is not the acceptance boundary.
3. The exact accepted event noun must be logged, not a reconstructed version.
4. The exact accepted event noun must be captured in the success path that called `event_update()`.
5. Snapshot integrity must not depend on `pma_base`.
6. The current PMA `.meta` sidecar format is migration input only, not the long-term integrity contract.
7. Crash-on-commit-failure is correct once in-memory state has advanced and the durable commit fails.

## Critical Current-Code Facts

### Live poke/ack path

1. `SerfAction::Poke` currently sends the poke result over the `result` oneshot before PMA preservation:
   - `crates/nockapp/src/kernel/form.rs`
   - `result.send(...)` happens in the serf loop before `preserve_event_update_leftovers()`.
2. `result_ack` only blocks the serf from moving on to the next action. It does not gate caller-visible ack.
3. `NockApp::handle_poke()` converts `Kernel::poke()` success into `PokeResult::Ack` immediately.
4. Therefore today an event can be acknowledged without any durable event-log commit.

### Accepted event capture

1. Success is established in `Serf::do_poke()` or `Serf::poke_swap()`.
2. `poke_swap()` may replace the accepted event noun with a `%crud` event using the same event number.
3. Therefore the accepted event noun must be captured in the success path, not regenerated later from external `wire`/`cause`.

### PMA boot path

1. Current PMA slab selection is still the legacy `0.pma` / `1.pma` plus `*.meta` scheme.
2. PMA load path still validates and/or maps against saved `pma_base`.
3. `pma_persist` currently disables checkpoint load path indirectly by disabling checkpointing.
4. This means checkpoint bootstrap fallback is currently coupled to background checkpoint saving mode, which is wrong for the target design.

### Snapshot/integrity substrate already present

1. `PmaDirectReader` already exists and can structurally walk nouns directly from a PMA file.
2. `jam_pma_to_writer()` already proves the direct-reader path against the live PMA root.
3. `Pma::sync_all()` exists, but finer-grained sync APIs required by the plan do not exist yet.

## First Implementation Slice To Land Now

This first slice is intentionally limited to trustworthy live event durability. It does **not** switch boot to snapshot replay yet and does **not** implement rotating snapshots yet.

### Scope

1. Add a SQLite-backed event log module.
2. Add boot plumbing for an event log path.
3. Capture the accepted event noun in the live poke success path.
4. Commit the accepted event to SQLite before exposing success back to the caller.
5. Add basic tests for event-log append/schema behavior.

### Explicitly Deferred In This Slice

1. Snapshot manifests.
2. Snapshot hashing.
3. Snapshot verification.
4. Snapshot rotation.
5. Boot replay from SQLite.
6. Decoupling checkpoint bootstrap import from checkpoint background saving.

That checkpoint decoupling is still required, but it is intentionally not mixed into the first durability patch to keep the surface area reviewable.

## Implementation Notes For The First Slice

### Event log module

1. Use synchronous `rusqlite`, not an async DB layer.
2. Open the connection on the serf thread.
3. Apply:
   - `journal_mode = WAL`
   - `synchronous = FULL`
   - `temp_store = MEMORY`
   - `foreign_keys = ON`
4. Create `meta`, `events`, and `snapshots` tables now even if only `events` is used immediately.
5. Store `schema_version` in `meta`.

### Accepted-event capture

1. Introduce a type that returns:
   - effect noun
   - durable event-log payload
2. Capture:
   - `event_num`
   - `job_jam`
   - `wire_source`
   - `wire_version`
   - `wire_tags_json`
   - `cause_hash`
   - `job_hash`
   - `created_at_ms`
3. `cause_hash` is over the original external `cause` noun.
4. `job_jam` is the exact accepted event noun, which may be the `%crud` noun.
5. `job_hash` is `blake3(job_jam)`.

### Ack gating

The live success path must become:

1. `serf.poke(...)`
2. copy effects to `NounSlab`
3. `preserve_event_update_leftovers()`
4. append+commit SQLite event
5. send success result to the kernel caller
6. `NockApp` maps success to caller-visible `Ack`

The SQLite append must happen after `preserve_event_update_leftovers()` in this slice so the code remains aligned with the current plan’s ordering of “event applied in memory -> PMA updated in memory -> event committed -> ack”.

### Commit failure behavior

1. If SQLite commit fails after the event was accepted and in-memory state advanced, abort the process.
2. Do not translate this into `Nack`.
3. Do not keep the process limping.

## Remaining Work After The First Slice

### Next slice

1. Decouple checkpoint import from checkpoint save mode.
2. Add PMA sync primitives:
   - `sync_used_data()`
   - `sync_trailer()`
   - `sync_file()`
3. Add snapshot manifest type.
4. Add used-range hashing.
5. Add full verifier on top of `PmaDirectReader`.

### After that

1. Add `epoch.pma` creation.
2. Add boot candidate selection from SQLite snapshots table.
3. Add replay from SQLite `job_jam`.
4. Add checkpoint/state-jam fallback only if no snapshot is usable.
5. Add rotating snapshots after epoch snapshot path is stable.

## Tests That Must Eventually Exist

1. No caller `Ack` before SQLite commit.
2. Commit failure aborts instead of `Nack`ing.
3. Replay from logged `job_jam` reproduces live state, including the `%crud` path.
4. Event sequence continuity failure refuses boot.
5. Manifest corruption rejects snapshot.
6. Corrupted newest snapshot falls back to older snapshot.
7. Corrupted rotating snapshots fall back to epoch.

## Files Most Relevant To Continue The Work

1. `crates/nockapp/src/kernel/form.rs`
2. `crates/nockapp/src/nockapp/mod.rs`
3. `crates/nockapp/src/kernel/boot.rs`
4. `crates/nockvm/rust/nockvm/src/pma.rs`
5. `crates/nockvm/rust/nockvm/src/pma/stream.rs`
6. `docs/BUMP-PMA-SNAPSHOTS-EVENT-LOG-INTEGRITY-PLAN.md`

## Review Checklist For Any Future Patch

1. Does live `Ack` still imply durable SQLite commit?
2. Is the accepted event noun still captured from the exact success path?
3. Did any new code accidentally reintroduce `pma_base` as an integrity requirement?
4. Did any boot-path change accidentally make checkpoint fallback disappear?
5. Are snapshot files still immutable once committed?
