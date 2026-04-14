# PMA Durability Operations

This document is the operator-facing guide for the PMA durability path now implemented in `nockapp`.

## What Exists Now

The current runtime has:

1. SQLite event durability for accepted events.
2. An immutable `epoch` snapshot.
3. Rotating snapshots with retention of two ready non-epoch snapshots.
4. Verified snapshot restore with replay from SQLite `job_jam`.
5. Fallback across snapshot candidates, then checkpoint/state-jam bootstrap.
6. Orphan snapshot artifact cleanup into `pma/corrupted_pma/`.

## Relevant CLI Flags

These are the main boot flags/operators should know about:

1. `--pma-persist`
   Enables PMA durability mode and disables background checkpoint saves.

2. `--event-log-path`
   Overrides the SQLite event log path.
   Default: `data_dir/event-log.sqlite3`

3. `--rotating-snapshot-interval-event-time`
   Controls how much cumulative accepted event-processing time must elapse before a new rotating snapshot is attempted.
   Use `none` or `0` to disable rotating snapshots.
   Default: `300` seconds

4. `--data-dir`
   Overrides the root durability directory.

5. `--checkpoint-mode`
   Controls background checkpoint writing only.
   Checkpoint bootstrap import remains available even when checkpoint saving is disabled.

## On-Disk Layout

Under `data_dir`:

1. `event-log.sqlite3`
2. `pma/epoch.pma`
3. `pma/epoch.manifest`
4. `pma/snap-${TIMESTAMP}.pma`
5. `pma/snap-${TIMESTAMP}.manifest`
6. `pma/corrupted_pma/`
7. `pma/0.pma`, `pma/1.pma`, and `*.meta`

Notes:

1. `epoch` and `snap-*` files are immutable snapshot artifacts.
2. `0.pma` / `1.pma` remain the operative runtime slabs.
3. `corrupted_pma/` is where orphan snapshot artifacts or crash leftovers are moved for later inspection.

## Boot Order

When PMA durability mode is enabled, boot uses this order:

1. Active ready snapshot from SQLite, if present.
2. Other ready rotating snapshot(s), newest first.
3. Ready epoch snapshot.
4. Legacy operative PMA / `.meta` migration path.
5. Checkpoint/state-jam bootstrap.

If a chosen snapshot is behind the event log head, boot replays the missing `job_jam` events.

If continuity is broken in the event log for the chosen snapshot, boot fails rather than silently falling back to stale state.

## Snapshot Cleanup Behavior

On boot:

1. Extra ready rotating snapshots beyond the retention window are retired/deleted.
2. Snapshot files in `pma/` that have no corresponding ready SQLite row are moved into `pma/corrupted_pma/`.
3. If a snapshot candidate fails verification, it is marked `failed` in SQLite and boot continues to the next candidate.

## Metrics

The following `nockapp` metrics are relevant to durability and recovery:

1. `nockapp.event_log.append`
2. `nockapp.event_log.commit_failures`
3. `nockapp.snapshot.build`
4. `nockapp.snapshot.build_failures`
5. `nockapp.snapshot.verify`
6. `nockapp.snapshot.verify_failures`
7. `nockapp.snapshot.cleanup`
8. `nockapp.snapshot.cleanup_failures`
9. `nockapp.replay.apply`
10. `nockapp.replay.failures`
11. `nockapp.replay.events`

## Suggested Dashboards

Recommended dashboard panels:

1. Event log append latency: p50 / p95 / p99 of `nockapp.event_log.append`
2. Snapshot build latency: `nockapp.snapshot.build`
3. Snapshot verify latency: `nockapp.snapshot.verify`
4. Snapshot cleanup latency and failure count
5. Replay duration and replayed event count per boot
6. Event log commit failure count
7. Snapshot verify failure count
8. Replay failure count

## Suggested Alerts

Recommended alerts:

1. Any non-zero `nockapp.event_log.commit_failures`
2. Any non-zero `nockapp.snapshot.verify_failures`
3. Any non-zero `nockapp.replay.failures`
4. Sustained `nockapp.snapshot.build_failures`
5. Sustained `nockapp.snapshot.cleanup_failures`

## Operator Guidance

If boot leaves files in `pma/corrupted_pma/`:

1. Do not delete them immediately.
2. Check the SQLite `snapshots` table and recent logs.
3. Confirm whether the moved files correspond to a crash during snapshot write or a deferred cleanup case.

If boot fails on event-log continuity:

1. Treat it as a real durability problem, not a transient startup failure.
2. Inspect the `events` table for missing `event_num` values.
3. Do not assume checkpoint fallback is safe if accepted events may be missing from the log.
