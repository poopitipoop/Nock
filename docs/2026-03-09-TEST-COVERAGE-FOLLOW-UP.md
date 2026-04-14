1. Replay correctness, especially %crud.
    We still need a test that proves replay from persisted job_jam reproduces the live state for both the normal poke path and the %crud fallback path. That is the most important missing semantic check.
    Relevant files: form.rs, event_log.rs
2. A fuller boot recovery matrix.
    Build a table-driven set of boot scenarios around:
    - valid active snapshot
    - active snapshot corrupt, older rotating valid
    - both rotating corrupt, epoch valid
    - snapshot table empty, legacy PMA migration
    - snapshot table empty, checkpoint bootstrap
    - stale/missing active_snapshot_id
      Most of the helpers already exist in boot.rs; I’d expand there rather than scatter new tests all over.
3. Negative integrity cases.
    Add tests for:
    - PRAGMA quick_check failure path
    - missing event gap with multiple replayed events
    - manifest corruption on epoch, not just rotating
    - orphan artifact cleanup for more temp-file variants and mixed file sets
    - rotating snapshot interval none/0 truly disables new rotating snapshots
4. Metrics/config assertions.
    We now have more knobs and metrics, but almost no tests that prove they are wired.
    I’d add focused tests for:
    - --rotating-snapshot-interval-event-time default, override, and disable behavior
    - active snapshot transitions on fail/retire
    - snapshot/replay metrics increment on the paths we care about

If I were picking one concrete next patch, I’d do replay correctness + boot matrix refactor first. That will give us the most confidence per line of test code and make later compaction work much less risky.
