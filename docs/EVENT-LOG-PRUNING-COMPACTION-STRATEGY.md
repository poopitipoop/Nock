# Event Log Pruning And Compaction Strategy

This document scopes the next concrete engineering step after the current no-truncation phase: pruning and compaction of the SQLite event log.

## Problem

The current system keeps every accepted event forever in SQLite.

That is correct for the first trustworthy rollout, but it creates an unbounded operational cost:

1. Disk usage grows forever.
2. `quick_check` and backup operations get slower over time.
3. Boot-time replay scans get more expensive even if only a suffix is needed.

## Invariants

Any pruning/compaction work must preserve:

1. No accepted event may be discarded until there is at least one verified ready snapshot at or beyond the truncation cut line.
2. Snapshot boot plus replay must remain sufficient to reconstruct current state.
3. The event log remains authoritative for accepted events after the active snapshot point.
4. Truncation must never cross the minimum event number required by any ready snapshot candidate we still want to keep.

## Safe Cut Line

In the current design, the safe truncation boundary is:

`min_ready_snapshot_event_num`

More precisely:

1. Compute the minimum `event_num` among the snapshot rows we intend to retain.
2. Events strictly less than that value are eligible for pruning.
3. Events at or above that value must remain.

Example:

1. `epoch` at event `100`
2. rotating snapshots at events `220` and `260`

If all three are retained, safe prune boundary is `< 100`.

If epoch is retired and only the two rotating snapshots are retained, safe prune boundary is `< 220`.

## Recommended Phases

### Phase 1: Prune Metadata Only

Add a read-only reporting path first:

1. current event-log size
2. oldest retained event
3. bytes/event estimate
4. reclaimable prefix size estimate

This gives operators visibility before any destructive behavior exists.

### Phase 2: Prefix Prune

Add a controlled prefix prune operation:

1. Determine safe cut line from retained ready snapshots.
2. Delete `events` rows where `event_num < cut_line`.
3. `VACUUM` or equivalent maintenance only under explicit operator control.

This should be an explicit maintenance action first, not an automatic background task.

### Phase 3: Compaction Policy

Only after manual prune is proven:

1. add policy thresholds
2. prune automatically after successful rotating snapshot commit
3. retain a configurable safety buffer if desired

## SQLite Shape

The current schema already supports this work because:

1. `events.event_num` is monotonic and unique
2. `snapshots.event_num` anchors snapshot rows to replay position
3. `meta.active_snapshot_id` identifies the preferred boot snapshot

No schema rewrite is strictly required for phase 2.

## Suggested Operator Controls

Recommended future controls:

1. `--event-log-prune-before-event <N>` for manual maintenance
2. `--event-log-max-bytes` for policy-based automatic pruning
3. `--event-log-prune-safety-events <N>` to keep a replay buffer beyond the strict cut line

## Tests Required

Before enabling any pruning:

1. prune below epoch boundary refuses
2. prune below retained rotating boundary refuses
3. prune below safe cut line succeeds
4. boot from active snapshot + replay still succeeds after prune
5. fallback to older rotating / epoch still succeeds if those snapshots are retained

## Non-Goal For The First Prune Patch

Do not mix:

1. row pruning
2. rotating snapshot creation
3. boot selection changes
4. recovery semantics changes

The first prune patch should be a narrow maintenance feature with strong invariants and tests.
