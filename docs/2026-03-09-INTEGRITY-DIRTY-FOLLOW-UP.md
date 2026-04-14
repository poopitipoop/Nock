Right now the weak spot is that a live PMA can be treated as authoritative on boot if its .meta and trailer look plausible, even if it was only partially synchronized. A dirty/clean marker would help with that. The model I’d
recommend is:

- Mark the operative PMA dirty as soon as the process boots.
- Leave it dirty for the entire runtime.
- On graceful shutdown only:
    - stop intake
    - sync_used_data()
    - sync_trailer()
    - sync_file()
    - persist the small clean record
    - fsync that clean record
    - then mark the PMA clean at event_num = N
- On next boot, only trust live PMA if that clean marker is present and matches the PMA/trailer/kernel hash.

That gives you a strong answer to “did we shut down cleanly after PMA sync completed?” If the process crashes, power drops, or shutdown dies mid-sync, the marker stays dirty and boot falls back to verified snapshot + replay. That
is exactly the right failure mode.

What I would not do is per-event dirty/clean flipping. Since accepted-event durability already lives in SQLite, making every event do PMA msync/fsync would probably be expensive and redundant. It would turn the live PMA into a
second hot-path durability layer, which cuts against the current design.

A few important details:

- I would not reuse the current .meta file as the integrity contract. It still carries pma_base, and we already agreed that should not be the long-term trust boundary.
- The clean marker should be a new minimal record, or possibly event-log-backed metadata, with fields like:
    - version
    - clean/dirty
    - slab id
    - event_num
    - ker_hash
    - alloc_words
    - kernel_root_raw
    - cold_offset
    - checksum
- If we do keep a live-PMA fast path, we should also quick_check SQLite before trusting it.
