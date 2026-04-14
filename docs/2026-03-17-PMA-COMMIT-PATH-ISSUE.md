# 2026-03-17 PMA Commit Path Issue

This note explains the open PMA commit-path question in the current implementation: why the hot event path is weaker than the snapshot and shutdown paths, what that means semantically, and what a cleaner design would look like.

## Short Version

The PMA durability story currently has multiple commit protocols instead of one:

- the hot event path persists PMA state one way
- snapshot creation persists it a stronger way
- shutdown persists it a stronger way too, but not in exactly the same shape as snapshots

That is undesirable because the most frequently used persistence path should not be the least explicit one.

## The Current Hot Event Path

The hot path flows through `Serf::preserve_event_update_leftovers()` and eventually calls `persist_pma_state()` in `crates/nockapp/src/kernel/form.rs`.

Today that path does this:

1. `pma.persist_metadata()`
2. `durability::sync_path_data(pma.path(), "poke_cleanup_pma_fdatasync")`
3. `persist_pma_metadata(...)`

The important subtlety is step 1:

- `pma.persist_metadata()` writes the PMA trailer into the mmap'd region
- it does **not** itself flush the mmap'd PMA data or trailer to storage

So the hot path is relying on later file sync behavior to make dirty mmap pages durable.

## The Snapshot Path

Snapshot creation in `crates/nockapp/src/snapshot.rs` is more explicit:

1. `pma.sync_used_data()`
2. `pma.sync_trailer()`
3. `durability::sync_path_data(pma.path(), "snapshot_source_pma_fdatasync")`

That sequence first flushes the relevant mmap'd bytes with `msync`, then flushes the backing file descriptor.

This is a much clearer durability boundary than "write mmap'd bytes, then fdatasync the file and hope the OS folds the mapping writes in the way we expect."

## The Shutdown Path

Shutdown in `crates/nockapp/src/kernel/form.rs` is also stronger than the hot path:

1. `pma.persist_metadata()`
2. `pma.sync_all()`
3. `durability::sync_path_data(pma.path(), "shutdown_pma_fdatasync")`
4. `persist_pma_metadata_strict(...)`

This is explicit, but different from the snapshot path because it syncs the whole mapping rather than just the used data range plus trailer.

## Why This Is A Problem

The problem is not that the current hot path is guaranteed wrong on every filesystem. The problem is that it is less explicit about the intended ordering and durability contract than the other PMA commit paths.

In practice that creates three risks:

1. We do not have one obvious answer to "what makes a PMA event durable?"
2. Linux and macOS may differ more in mmap writeback timing than we want.
3. Future changes are more likely to regress correctness when there is no single canonical commit helper.

If the `.meta` sidecar is written after the event path runs, then the sidecar may claim a new PMA root and event number are durable even though the PMA mapping was not flushed using the same explicit `msync` protocol used elsewhere.

That asymmetry is the core issue.

## What The Cleaner Model Looks Like

The clean model is:

1. write PMA trailer into the mapping
2. `msync` the used PMA data range
3. `msync` the trailer range
4. flush the backing file descriptor
5. publish sidecar metadata

In other words:

- first flush mapped PMA bytes
- then flush the file
- then write and publish the sidecar that points at that PMA state

That gives one understandable ordering rule.

## Why Range Sync Is Attractive

The snapshot path already suggests the nicest form of the protocol:

- `sync_used_data()`
- `sync_trailer()`
- file flush

That is attractive because it syncs exactly the PMA bytes that matter:

- the data prefix up to `alloc_words`
- the trailer that carries PMA allocation metadata

Compared with `sync_all()`, that is narrower and expresses intent more clearly.

## A Reasonable End State

A good end state would be one shared helper used by:

- hot event persistence
- shutdown
- PMA GC output publication
- any future PMA publish path

That helper would do roughly:

1. `pma.persist_metadata()`
2. `pma.sync_used_data()?`
3. `pma.sync_trailer()?`
4. `durability::sync_path_data(pma.path(), context)?`
5. `persist_pma_metadata_strict(pma)?`

Then the sidecar publish becomes the final step in a single defined durability protocol.

## Open Design Question

The remaining design choice is mainly:

- should shutdown also move to the narrower `sync_used_data() + sync_trailer()` form for consistency
- or should shutdown keep `sync_all()` and only the hot path be upgraded

My bias is to standardize on the narrower explicit range sync everywhere unless there is a strong reason shutdown needs whole-mapping sync.

That would make the code easier to reason about and would line up the event path, snapshot path, and shutdown path under the same mental model.

## What This Note Is Not

This note is only about the PMA commit-path issue.

It is separate from:

- removing `pma_base` from the PMA sidecar
- removing fixed-address reopen behavior
- broader snapshot integrity or event-log policy questions

Those are related durability topics, but they are not the same issue.
