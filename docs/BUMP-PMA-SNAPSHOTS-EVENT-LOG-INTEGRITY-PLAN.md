# Bump PMA + Event Log + Integrity Plan

## Scope

This plan implements the target design:

1. PMA remains the primary runtime image.
2. Add an append-only SQLite event log (no truncation yet).
3. Create an immutable `epoch` snapshot on first boot (or first migration boot).
4. Maintain up to two rotating snapshots named by timestamp (`snap-${TIMESTAMP}.pma`), deleting the oldest (third non-epoch) after each new snapshot is durably committed to the event log.
5. Boot by loading latest valid PMA snapshot first, then replaying missing events from SQLite.
6. Add explicit integrity metadata + verification + deterministic recovery fallbacks.

## Non-Goals (for this iteration)

1. Event log truncation/pruning.
2. Full historical compaction of event log.
3. Multi-writer PMA/event pipeline.
4. Cross-node snapshot portability guarantees.

## Current Risks To Address

1. PMA metadata is updated in-memory each event but not durably ordered with poke acknowledgement.
2. Boot integrity checks are shallow today (metadata checks, not full graph checks).
3. PMA persist currently disables checkpoint load path entirely instead of supporting checkpoint bootstrap-only mode.

Note: PMA GC copy path mutates from-space (forwarding pointers), but this is accepted. If a crash occurs mid-GC, recovery uses snapshot + event replay. Future GC changes are likely to be more invasive, not less, so a non-destructive copy is not worth the complexity.

## High-Level On-Disk Layout

Under `data_dir`:

1. `pma/epoch.pma`
2. `pma/snap-${TIMESTAMP}.pma` (up to two non-epoch snapshots at a time)
3. `pma/epoch.manifest`
4. `pma/snap-${TIMESTAMP}.manifest` (one per snapshot)
5. `event-log.sqlite3`

`TIMESTAMP` is a monotonic identifier (e.g. Unix millis or `%Y%m%d%H%M%S%3f`) embedded in the filename at creation time. The event log SQLite `snapshots` table is the authoritative record of which snapshot files exist, which event IDs they correspond to, and which is newest.

Keep existing `checkpoints/*.chkjam` for bootstrap compatibility (read/import path only; no background saver required).

## SQLite Schema (v1)

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA temp_store = MEMORY;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  event_num INTEGER NOT NULL UNIQUE,           -- arvo event number
  job_jam BLOB NOT NULL,                       -- deterministic replay payload (full poke job noun)
  wire_source TEXT NOT NULL,                   -- for observability/index/debug
  wire_version INTEGER NOT NULL,
  wire_tags_json TEXT NOT NULL,                -- compact JSON array
  cause_hash BLOB NOT NULL,                    -- blake3(cause_jam) for audit
  job_hash BLOB NOT NULL,                      -- blake3(job_jam)
  created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS events_event_num_idx ON events(event_num);

CREATE TABLE IF NOT EXISTS snapshots (
  snapshot_id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL CHECK(kind IN ('epoch','rotating')),
  state TEXT NOT NULL CHECK(state IN ('writing','ready','failed','retired')),
  event_num INTEGER NOT NULL,
  pma_path TEXT NOT NULL,
  manifest_path TEXT NOT NULL,
  alloc_words INTEGER NOT NULL,
  kernel_root_raw INTEGER NOT NULL,            -- raw noun word (must be offset-form or direct)
  cold_offset INTEGER NOT NULL,                -- u32 serialized as INTEGER
  used_blake3 BLOB NOT NULL,                   -- hash of [0, alloc_words * 8)
  structure_blake3 BLOB,                       -- optional hash over streamed jam bytes from root
  created_at_ms INTEGER NOT NULL,
  activated_at_ms INTEGER,
  base_snapshot_id INTEGER,                    -- optional lineage (epoch or previous active)
  timestamp_tag TEXT NOT NULL,                 -- timestamp embedded in filename
  UNIQUE(kind, timestamp_tag)
);

CREATE INDEX IF NOT EXISTS snapshots_kind_ts_idx ON snapshots(kind, timestamp_tag DESC);
CREATE INDEX IF NOT EXISTS snapshots_event_idx ON snapshots(event_num DESC);
```

Notes:

1. `job_jam` is required to avoid replay nondeterminism from regenerated `eny/now` fields.
2. Store snapshot rows in the same DB so event/snapshot references are naturally co-located.
3. No truncation yet: `events` grows unbounded in this phase.

## Snapshot Manifest Format

Use a versioned bincode struct (separate from SQLite row) written to `*.manifest` atomically.

Fields:

1. `magic`
2. `version`
3. `kind` (`epoch|rotating`)
4. `timestamp_tag`
5. `ker_hash`
6. `event_num`
7. `pma_words`
8. `alloc_words`
9. `kernel_root_raw`
10. `cold_offset`
11. `used_blake3`
12. `structure_blake3` (optional in early rollout)
13. `created_at_ms`
14. `checksum` (blake3 over all prior fields)

Important:

1. Do not persist `pma_base` as an integrity requirement.
2. `kernel_root_raw` must be offset-form or direct atom; never pointer-form.

## Event Commit Protocol (Durability + Atomicity)

Ack contract for poke success:

1. Event applied in memory.
2. PMA updated in memory (`preserve_event_update_leftovers`).
3. Event appended and committed in SQLite (`BEGIN IMMEDIATE` + `INSERT` + `COMMIT`).
4. Only then return poke success to caller.

Rationale:

1. SQLite is the authoritative durability boundary for accepted events.
2. PMA can lag durability and be recovered by replay.

### Architecture change required: ack gating

The current serf loop sends the poke result (effects) to the NockApp main loop via `result.send()` **before** `preserve_event_update_leftovers` runs. The NockApp can then dispatch `PokeResult::Ack` to the caller while the serf thread is still doing PMA copy, and before any SQLite commit. The `result_ack` channel only gates the serf proceeding to the next action — it does not gate the caller-visible ack.

To honor the durability contract above, the serf→NockApp protocol must be restructured so that `PokeResult::Ack` is not sent to the caller until the SQLite commit has succeeded. The specific mechanism (moving the commit before `result.send()`, adding a second synchronization gate, etc.) is to be determined during implementation.

Implementation details:

1. Add event-log append in serf thread after successful event update.
2. Log the full `job_jam` event noun (not only external `wire/cause`).
3. On SQLite commit failure, **crash the process**. By the time the commit is attempted, the in-memory state (`event_num`, `arvo`) and PMA have already advanced irrevocably. Crashing forces recovery from the last consistent snapshot + event log, which is the standard approach for event-sourced systems.

## Snapshot Construction (Efficient, No Second VM)

### Snapshot trigger

Periodic (configurable interval or event count threshold), executed at event boundaries in serf thread.

### Build algorithm

Snapshot creation uses a **kernel syscall zero-copy** of the PMA slab and meta file (e.g. `copy_file_range`, `FICLONE`/reflink where available), not a noun-by-noun traversal. This keeps snapshot creation fast and avoids blocking the serf thread for extended periods. The GC copying compactor is a separate mechanism for compacting live data.

1. Generate a new timestamp tag for the snapshot filename (e.g. `snap-1709472000000.pma`).
2. `msync` active PMA used range and trailer (ensure backing file is up to date).
3. Kernel zero-copy active PMA file to target snapshot file (`snap-${TIMESTAMP}.pma`).
4. Write manifest from current metadata to `snap-${TIMESTAMP}.manifest`.
5. Compute `used_blake3` over target snapshot used range `[0, alloc_words * 8)`.
6. Optionally compute `structure_blake3` by streaming jam from target root to `io::sink`.
7. `fsync` target snapshot file.
8. Write manifest temp file -> `fsync(temp)` -> `rename` -> `fsync(parent dir)`.
9. Insert SQLite `snapshots` row with `state = 'ready'` in a transaction. Mark active snapshot in `meta` (`active_snapshot_id`). `COMMIT`.
10. After durable commit: if there are now more than two non-epoch `ready` snapshots, delete the oldest (by `timestamp_tag`) — remove its PMA and manifest files, then mark its row `retired` in SQLite.

### Epoch snapshot creation

If no snapshots exist:

1. Boot from checkpoint/state-jam/current PMA migration source.
2. Create `epoch` snapshot with event number equal to current boot event.
3. Mark it `ready` and active.
4. Subsequent snapshots use timestamp-based naming (`snap-${TIMESTAMP}.pma`), keeping at most two non-epoch snapshots.

## PMA Changes Required

Add PMA sync methods for snapshot construction:

1. `sync_used_data()` (range to `alloc_words`)
2. `sync_trailer()`
3. `sync_file()` (`fdatasync`/`fsync` on backing file)

## Integrity Verification

## Snapshot-level checks

### Fast checks (post-write)

1. Manifest decode + checksum + version.
2. PMA trailer magic/version/data_words/alloc bounds.
3. Manifest-vs-trailer consistency (`alloc_words`, `data_words`).
4. `used_blake3` recomputation on PMA used range.
5. Root noun classification sanity:
   1. direct atom OR
   2. offset-form indirect/cell within bounds.
6. `cold_offset` in range and decodable.

### Full verification (boot recovery)

When recovering from a snapshot (an already-exceptional circumstance), run the full structural walk in addition to the fast checks above:

7. Structural walk from root using `PmaDirectReader` (or `jam_pma_to_writer` to sink) must complete without:
   1. out-of-bounds offsets
   2. invalid noun tags
   3. forwarding pointers
   4. invalid indirect atom sizes

## SQLite checks (boot)

1. `PRAGMA quick_check`.
2. `SELECT max(event_num) FROM events`.
3. Verify monotonic continuity from chosen snapshot event:
   1. For no-truncation phase, enforce existence of all events `snapshot_event+1..max_event_num`.
   2. If gaps exist in the event sequence, **refuse to boot** and log a clear error message explaining which event numbers are missing, so the operator can diagnose whether the gap is due to a bug or corruption.

## Orphan file cleanup (boot)

On boot, scan the `pma/` directory for `snap-*.pma` and `snap-*.manifest` files that have no corresponding `ready` row in the SQLite `snapshots` table. This handles two cases: (1) the process crashed after creating a target PMA file but before the manifest was written or the DB row was committed, and (2) the process crashed after the DB commit but before the oldest snapshot was deleted (leaving three non-epoch snapshots temporarily). In case (2), simply complete the deferred deletion. In case (1), move orphan files to `corrupted_pma/` for later analysis.

## Boot Selection + Recovery/Fallback Matrix

Candidate order (determined from SQLite `snapshots` table):

1. Active snapshot (newest `ready` rotating snapshot by `timestamp_tag`).
2. Other rotating snapshot (second-newest `ready` rotating snapshot).
3. Epoch snapshot.
4. Checkpoint/state-jam bootstrap.

For each candidate snapshot:

1. Run full integrity checks.
2. If valid:
   1. Kernel zero-copy snapshot PMA to the operative PMA slab location (overwriting the current/corrupted operative slab). Move the overwritten slab to a `corrupted_pma/` subdirectory for later analysis if needed.
   2. Load the operative PMA slab.
   3. Replay events `(snapshot_event+1..event_log_max)`.
   4. Run `preserve_event_update_leftovers`.
   5. Continue startup.
3. If the process crashes mid-replay, the operative PMA slab is inconsistent. On next boot, discard it and re-copy from the snapshot, replaying from scratch. The snapshot files themselves are never modified during replay.

If candidate invalid:

1. mark snapshot `failed` in DB
2. continue to next candidate

If no snapshot valid:

1. try checkpoint/state-jam bootstrap
2. create fresh epoch snapshot
3. if bootstrap unavailable, fail startup with explicit operator message

## Replay Semantics

1. Replay from persisted `job_jam` in strict `event_num ASC` order.
2. Feed replay through the same serf event path used for live events, but with a replay-specific entrypoint that:
   1. bypasses wire/cause re-synthesis randomness
   2. uses logged event noun directly
3. During replay boot mode:
   1. disable re-logging of replayed events
   2. preserve PMA periodically (batch) and once at end

## Snapshot References in Event Log (Recommended)

Keep references in DB (already via `snapshots` table):

1. `snapshots.event_num` anchors each snapshot to log position.
2. `snapshots.base_snapshot_id` tracks lineage.
3. `meta.active_snapshot_id` identifies startup default.

No per-event `snapshot_id` column needed in this phase.

## Step-by-Step Implementation Plan

## Phase 1: Foundations (Schema + Plumbing)

1. Add `event_log` module in `crates/nockapp`:
   1. SQLite open/init/migrations (see schema migration strategy below)
   2. schema v1 from this doc
   3. typed APIs: `append_event`, `max_event_num`, `iter_events_from`, `insert_snapshot`, `set_active_snapshot`
2. Add config flags:
   1. `--event-log-path` (default `data_dir/event-log.sqlite3`)
   2. `--snapshot-interval-events` and/or `--snapshot-interval-ms`
3. Wire event log handle into serf thread state.

### Schema migration strategy

Store a `schema_version` key in the `meta` table. On open, check the current version and apply sequential migrations in order to bring the schema up to date. Each migration is a numbered SQL script or function. This also enables migrating the existing `*.meta` PMA metadata into SQLite in a future version, consolidating all metadata into a single store with a well-defined migration path.

## Phase 2: Deterministic Event Capture

1. Capture full event noun/job as jam bytes.
   1. The job noun lives on the NockStack which is reset by `preserve_event_update_leftovers`. The jam (or a slab copy) must be captured **inside** `do_poke`/`poke_swap` before `event_update` or stack reset occurs.
   2. Keep the captured bytes available through `preserve_event_update_leftovers` so the serf loop can append them to SQLite afterward.
2. Append event in SQLite transaction before success ack.
3. Add metrics:
   1. `event_log_append_ms`
   2. `event_log_commit_failures`

## Phase 3: Snapshot Format + Integrity

1. Implement manifest struct + encode/decode + checksum.
2. Implement PMA used-range hashing.
3. Implement structural verifier using `PmaDirectReader` traversal.
4. Add `verify_snapshot(slot)` function that executes full check suite.

## Phase 4: Snapshot Builder (Epoch + Rotating)

1. Implement snapshot manager:
   1. create epoch if absent
   2. create new `snap-${TIMESTAMP}.pma` snapshots
   3. after durable SQLite commit, delete oldest (third non-epoch) snapshot
2. Implement durable write ordering:
   1. kernel zero-copy active PMA to target `snap-${TIMESTAMP}.pma`
   2. verify target (fast checks)
   3. fsync snapshot file
   4. write+fsync manifest
   5. DB insert + active-snapshot update in one transaction, `COMMIT`
   6. delete oldest non-epoch snapshot files and mark row `retired`
3. Record snapshot rows in SQLite.

## Phase 5: Boot Recovery Pipeline

1. Candidate selection logic from SQLite (newest rotating -> second-newest rotating -> epoch).
2. Full verify each candidate (fast checks + structural walk).
3. Kernel zero-copy snapshot to operative PMA slab, move old slab to `corrupted_pma/`.
4. Replay missing events from SQLite.
5. Fallback to checkpoint/state-jam only if no valid snapshot.
6. After fallback boot, create/recreate epoch snapshot.

## Phase 6: Migration from Current PMA Persist

1. If old `*.meta` exists and new DB has no snapshots:
   1. attempt old PMA load
   2. verify and register as epoch snapshot in new DB
2. Keep checkpoint import path available regardless of PMA persistence mode.
3. Keep old files untouched until first successful new snapshot commit.
4. The new snapshot load path must **skip the `pma_base` validation** (`form.rs:1392-1397`). All PMA data is already base-address-agnostic (offset-form), and snapshots are zero-copied to the operative slab location where the base address is determined by the current process's mmap.

## Phase 7: Hardening + Tests

1. Unit tests:
   1. schema migration
   2. event append/replay continuity
   3. manifest hash mismatch detection
2. Integration tests:
   1. crash before/after SQLite commit
   2. crash mid-snapshot build
   3. corrupted active snapshot fallback to alternate
   4. corrupted both rotating snapshots fallback to epoch
3. Property tests:
   1. replay produces same end state as live run for deterministic test streams.

## Acknowledgement Rules

Live poke success must mean:

1. event applied in memory
2. event committed to SQLite

It does not mean PMA snapshot has advanced. PMA advancement is periodic and recoverable via replay.

## Operational Defaults

1. SQLite: WAL + FULL sync.
2. Snapshot cadence: conservative default (for example every 10k events or 10 minutes; tune later).
3. Boot always verifies snapshot before using it.
4. No log truncation in this phase.

## Implementation Notes

### `poke_swap` (crud recovery) and event capture

When `do_poke` fails, `poke_swap` constructs a different job noun (a crud poke) and retries. Both the original and crud poke use the same event number (since `event_update` only runs on success). `play_list` (replay) has no `poke_swap` — it fails immediately on error.

The `job_jam` capture must happen at the point `event_update` is called, inside whichever code path succeeded (`do_poke` success or `poke_swap` success). This ensures the logged noun is exactly what produced the accepted state transition, and replay can feed it directly through `soft(ovo, POKE_AXIS)` without needing error recovery.

## Future Extension (Explicitly Deferred)

1. Event log truncation to `min_ready_snapshot_event`.
2. Optional archival export before truncation.
3. Incremental snapshot delta encoding.
4. Background verification/scrubbing.
