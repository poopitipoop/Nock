# 2026-03-16 PMA, macOS, and Boot Changes

This document explains the current PMA/macOS/boot hardening diff: what changed, what motivated it, and why the overall patch set makes sense together.

## High-Level Themes

The diff falls into six broad buckets:

1. Make PMA paging and PMA-related tests compile and behave sensibly on macOS.
2. Strengthen macOS durability semantics so PMA-related file writes use `F_FULLFSYNC`.
3. Make bootstrap flows safer by preventing destructive or contradictory startup combinations.
4. Prevent `hoonc` prewarm from importing bootstrap state into an already-live durability domain.
5. Remove `pma_base` from the PMA sidecar and stop doing fixed-address PMA reopen.
6. Clean up warnings exposed by release-test builds so important regressions are easier to see.

These changes fit together because they all push in the same direction: safer durability, less surprising boot behavior, and more trustworthy PMA behavior across Linux and macOS.

## Top-Level Build and Dependency Changes

### `Makefile`

A new top-level `contracts-deps` target was added.

- It forwards to `crates/bridge/contracts`.
- The motivation is ergonomics: bridge contract dependencies are part of normal repo setup, and a top-level target makes that easier to discover and repeat.
- This makes sense because it turns a crate-local setup step into a standard project entry point.

### `crates/bridge/Cargo.toml`

The `nockapp-grpc` dependency was changed from a workspace dependency override to an explicit path dependency.

- Before this change, `bridge` asked for `default-features = false` while also using `workspace = true`.
- Cargo warned that the `default-features` override was being ignored in that configuration.
- Switching to `path = "../nockapp-grpc"` makes the intent explicit: `bridge` wants the client-only variant without inheriting the default server feature.

This makes sense because feature selection should be real, not advisory. A warning-free manifest is better, and explicit client-only linkage is clearer than depending on Cargo override behavior.

### `crates/nockapp/Cargo.toml` and `Cargo.lock`

`nockapp` now depends on `libc` explicitly.

- The motivation is the macOS `F_FULLFSYNC` implementation in the shared durability layer.
- `Cargo.lock` changed accordingly because `nockapp` now lists `libc` directly.

This makes sense because the new durability behavior is intentionally low-level and platform-specific; it should declare the dependency it actually uses.

## `hoonc` Prewarm Safety

### `crates/hoonc/src/lib.rs`

The `hoonc` bootstrap path now checks for any existing durability state, not just checkpoints.

The helper logic now treats the following as evidence of existing runtime state:

- regular files in `checkpoints/`
- regular files in `pma/`
- `event-log.sqlite3`
- `event-log.sqlite3-wal`
- `event-log.sqlite3-shm`

The prewarm path also now forces `boot_cli.new = true` when it injects the embedded bootstrap `state_jam`.

The motivation was a real failure mode:

- `hoonc` could boot from an existing PMA and event log
- then still import the prewarm `state_jam`
- which rewound the in-memory event number
- and caused SQLite event-number collisions later

This makes sense because prewarm state is bootstrap input, not an overlay for an already-initialized durability domain. Detecting PMA and SQLite state closes the hole that only checking `.chkjam` files left open.

The added tests reflect that intent:

- existing PMA state blocks prewarm
- existing SQLite state blocks prewarm
- an empty `hoonc` dir still allows prewarm

## NockApp Boot Safety

### `crates/nockapp/src/kernel/boot.rs`

There are two related boot-policy changes here.

### `--state-jam` now requires `--new`

This is enforced in two places:

- the CLI parser (`requires = "new"`)
- the runtime setup path, so programmatic callers cannot bypass the rule

The motivation is consistency and safety. Importing a serialized kernel state into an already-populated data directory is not a normal replay path; it creates split-brain state between imported PMA data and existing event-log history.

This makes sense because `state_jam` is a bootstrap mechanism, not a live migration primitive.

### `--new` no longer deletes existing state

Before this diff, `--new` would remove:

- `checkpoints/`
- PMA slab files
- PMA metadata sidecars
- SQLite event-log files and sidecars

Now `--new` refuses to proceed if:

- the target data directory already contains entries
- the target event-log file or its SQLite sidecars already exist

It logs a warning and returns an `AlreadyExists` error instead of deleting anything.

The motivation is operator safety. `--new` should mean "fresh bootstrap into a fresh target", not "destroy whatever is already there and start over".

This makes sense because destructive flags are easy to use accidentally, especially when durability state lives in multiple files and directories. Refusing to clobber makes recovery and intent much clearer.

The tests added here cover:

- parser rejection for `--state-jam` without `--new`
- runtime rejection for programmatic callers without `--new`
- rejection of `--new` against a non-empty data dir
- acceptance of `--new` for an actually empty target

## macOS Durability Semantics

### `crates/nockapp/src/utils/durability.rs`

This is the core durability fix behind the review finding.

The old code treated:

- `File::sync_all()`
- `File::sync_data()`

as the end of the story on every platform.

The new code introduces a platform-aware strategy:

- on macOS, regular files do the normal `sync_all` or `sync_data` first, then `fcntl(F_FULLFSYNC)`
- directories stay on the portable `fsync` path
- the async helper uses the same strategy
- log messages distinguish the stronger path with names like `fsync+fullfsync` and `fdatasync+fullfsync`

The motivation is Apple-specific durability semantics. On macOS, `F_FULLFSYNC` is the stronger request for flushing through device caches, which matters if PMA durability is supposed to be trusted across power loss in the same spirit as Linux durability paths.

This makes sense because:

- PMA slabs, PMA sidecar metadata, and snapshot artifacts already flow through this helper
- one shared durability shim is better than re-implementing platform logic at every call site
- directories are intentionally left on plain `fsync`, because rename-parent durability is still needed but `F_FULLFSYNC` is aimed at regular file data

The new durability tests check:

- syncing a read-only regular file handle
- syncing data for a regular file path
- syncing a parent directory
- `write_atomic()` behavior end to end

## PMA Portability and Paging Compatibility

### `crates/nockvm/rust/nockvm/src/pma.rs`

The PMA paging behavior now goes through a shared helper:

- Linux prefers `MADV_PAGEOUT`
- if that is not accepted, Linux falls back to `MADV_DONTNEED`
- non-Linux builds use `MADV_DONTNEED`

The paging tests were updated to use that same helper.

The motivation is portability. `MADV_PAGEOUT` is not exposed on macOS in `libc`, so using it directly made the PMA build fail there. Sharing the helper between production code and tests keeps the behavior consistent.

This makes sense because paging advice is a performance hint, not part of PMA correctness. The code should use the strongest available platform-specific hint where possible, but degrade gracefully where not.

### `crates/nockvm/rust/nockvm/src/pma/stream.rs`

`OpenOptionsExt` is now imported only on Linux.

The motivation is that the previous import assumed Linux-specific file option APIs were available on macOS too.

This makes sense because platform-specific imports should be gated exactly where the platform-specific behavior exists.

### `crates/nockvm/rust/nockvm/src/pma.rs` and `crates/nockchain/tests/pma_paging_kernel.rs`

The `mincore` vector pointer cast was changed from `*mut c_uchar` to `*mut c_char`.

The motivation is Darwin's `libc` signature for `mincore`, which expects `*mut c_char`.

This makes sense because it is the correct platform ABI, and it fixes the release-test compile error without changing runtime intent.

### `crates/nockvm/rust/nockvm/src/mem.rs`

The panic helper imports inside the test module are now guarded by `#[cfg(debug_assertions)]`.

The motivation is warning cleanup during `cargo test --release`. Those imports were only used by debug-only tests.

This makes sense because release test builds should not warn about imports that are only needed when the debug-only tests are compiled in.

## Removing `pma_base` and Fixed-Address Reopen

### `crates/nockapp/src/kernel/form.rs`

The PMA sidecar no longer persists `pma_base`.

The sidecar now contains only:

- kernel hash
- event number
- kernel root raw noun
- cold offset
- checksum

The sidecar format version was bumped from `2` to `3` to reflect that layout change.

The boot path also changed:

- PMA inspection now validates by opening the PMA normally, not by trying to reopen it at a saved virtual address
- normal PMA boot uses `Pma::open(...)` directly
- snapshot-restore metadata synthesis no longer writes a base address into `.meta`
- `Serf::new(...)` no longer treats same-base remapping as part of PMA validity

The motivation is that PMA nouns are already offset-based. Persisting an old process virtual address was not part of the true semantic state, and on macOS the old design was actively risky because fixed-address reopen falls back to clobber-prone `MAP_FIXED` behavior.

This makes sense because:

- the persisted PMA state is fundamentally base-agnostic
- ASLR makes same-address reopen a fragile contract
- removing `pma_base` eliminates a whole class of platform-specific mapping risk
- normal `mmap` without fixed-address requirements is the safer and more portable default on both Linux and macOS

One operational consequence is intentional: old `.meta` files from the pre-removal layout are no longer expected to work. PMA is still unreleased, so the chosen policy is "delete stale PMA state" rather than carrying migration compatibility.

### `crates/nockvm/rust/nockvm/src/pma.rs` and `crates/nockvm/rust/nockvm/src/mem.rs`

The fixed-address reopen machinery was removed entirely:

- `Pma::open_with_base(...)` is gone
- `Arena::open_file_with_base(...)` is gone
- the `FixedMapping` machinery and the `MAP_FIXED_NOREPLACE` / `MAP_FIXED` split are gone with it

The motivation is straightforward: once `pma_base` is no longer part of the sidecar contract, fixed-address reopen is no longer desirable as a normal PMA boot behavior.

This makes sense because the safest mapping strategy is the simplest one: open the PMA file normally and let the kernel choose the address.

## Warning Cleanup

### `crates/noun-serde/tests/serde.rs`

An unused `AtomExt` import was removed.

### `crates/noun-serde-derive/tests/wallet_types_derived.rs`

An unused `NockStack` import was removed from the test module.

### `crates/nockapp/src/event_log.rs`

`SnapshotRow.state` was renamed to `_state`.

The motivation here is straightforward: these warnings were noise in release-test output.

The `SnapshotRow` case deserves one extra note:

- the field still exists because Diesel is loading full snapshot rows
- it is simply not used when converting to the runtime `ReadySnapshotRecord`

Renaming it to `_state` preserves the row shape and makes the intent explicit: the field is present for deserialization compatibility, not because current logic needs to read it.

## Why This Patch Set Makes Sense As A Whole

Even though the diff touches many files, the changes are coherent.

- The durability changes make macOS behavior closer to the trust model expected on Linux.
- The boot-policy changes eliminate two dangerous workflows: destructive `--new` boot and importing bootstrap state into an existing durability domain.
- The `hoonc` prewarm fix closes a real event-number corruption path.
- The PMA paging and `libc` fixes make the code and tests compile cleanly on macOS without pretending Linux-only APIs exist there.
- The `pma_base` removal simplifies the persistence contract and removes fixed-address reopen from normal PMA behavior.
- The warning cleanup matters because it keeps real regressions visible in large `cargo test --release` runs.

In short, the diff is not random churn. It is a focused hardening pass on durability, boot safety, PMA portability, and persistence semantics.

## What This Diff Does Not Do

This patch set does not yet address every known PMA concern.

In particular, it does not yet:

- unify the event-path PMA commit protocol with the stronger snapshot/shutdown `msync` path

That remains a follow-up item and is discussed separately in [2026-03-17-PMA-COMMIT-PATH-ISSUE.md](/Users/callen/work/zorp/nockchain/docs/2026-03-17-PMA-COMMIT-PATH-ISSUE.md).

## Validation Performed

The work behind this diff was validated incrementally with focused commands, including:

- `cargo build -p nockvm --release --offline`
- `cargo test -p nockvm --release --offline --no-run`
- `cargo build -p nockapp --offline`
- `cargo build -p nockapp --release --offline`
- `cargo test -p nockapp --lib durability::tests --offline -- --nocapture`
- `cargo test -p nockapp --lib setup_rejects_state_jam_without_new_for_programmatic_callers --offline -- --nocapture`
- `cargo test -p nockapp --lib setup_rejects_new_when_data_dir_is_nonempty --offline -- --nocapture`
- `cargo test -p nockapp --lib setup_allows_new_for_empty_data_dir --offline -- --nocapture`
- `cargo test -p nockapp --lib --release --offline valid_pma_skips_corrupt_checkpoint_files -- --nocapture`
- `cargo test -p nockapp --lib --release --offline restores_epoch_snapshot_when_pma_is_missing -- --nocapture`
- `cargo build -p hoonc --release --offline`
- `cargo test -p noun-serde --test serde --release --offline --no-run`
- `cargo test -p noun-serde-derive --test wallet_types_derived --release --offline --no-run`
- `cargo test -p nockchain --test pma_paging_kernel --release --offline --no-run`
- `cargo build -p bridge --release --offline`

That validation pattern makes sense for a patch set like this because the changes span multiple crates and several of them were triggered by release-only or macOS-specific build/test paths.
