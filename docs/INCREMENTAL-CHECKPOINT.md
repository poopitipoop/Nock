# Reducing peak memory usage while checkpointing

Checkpoint Path

- PMA is kept current after each poke by copying the persistent state (warm/test_jets/hot/cache/cold/arvo) into the PMA arena and persisting metadata in preserve_event_update_leftovers, which is why checkpoint reads pull from PMA‑backed nouns when PMA is
  enabled. crates/nockapp/src/kernel/form.rs:1418, crates/nockapp/src/kernel/form.rs:1546.
- The checkpoint request runs in the serf thread: SerfAction::Checkpoint -> create_checkpoint, which extracts arvo at axis 6 and the current cold state. crates/nockapp/src/kernel/form.rs:508, crates/nockapp/src/kernel/form.rs:647.
- Cold is materialized as a noun on the stack via Cold::into_noun, which reuses existing nouns (no copy), then both cold and kernel state are copied into fresh NounSlabs with copy_into. crates/nockvm/rust/nockvm/src/jets/cold.rs:1202, crates/nockapp/src/kernel/
  form.rs:1712, crates/nockapp/src/noun/slab.rs:286.
  The copy uses NockStack::noun_space() (includes PMA arena) so PMA offset/pointer forms are resolved and duplicated into the slab. crates/nockvm/rust/nockvm/src/mem.rs:447.
- Jamming happens in SaveableCheckpoint::to_jammed_checkpoint, which calls NounSlab::jam (NockJammer) for state and cold. crates/nockapp/src/nockapp/save.rs:233, crates/nockapp/src/noun/slab.rs:475, crates/nockapp/src/noun/slab.rs:662.
- Persistence is a two‑stage bincode encode (payload then envelope) followed by a full write to disk. crates/nockapp/src/nockapp/save.rs:492, crates/nockapp/src/nockapp/save.rs:532.

Where The Memory Spikes Come From

- Full slab duplication: copy_into traverses the noun graph and copies every allocated atom/cell into heap slabs, preserving sharing via an IntMap. This duplicates everything in PMA into heap memory while the checkpoint is built. crates/nockapp/src/noun/
  slab.rs:286.
- Cold list materialization: Cold::into_noun builds three lists on the stack before the slab copy (temporary but large for big cold maps). crates/nockvm/rust/nockvm/src/jets/cold.rs:1202.
- Jam output + backref map: NockJammer::jam builds a BitVec and a NounMap for backrefs; then Bytes::copy_from_slice duplicates the bitvec buffer into a second allocation. crates/nockapp/src/noun/slab.rs:662, crates/nockapp/src/noun/slab.rs:726.
- Encode duplication: JammedCheckpointV2::encode allocates payload and then an envelope Vec, duplicating jam bytes at least twice while the JammedCheckpointV2 still holds both Bytes blobs. crates/nockapp/src/nockapp/save.rs:492.
- Disk write holds full buffer: tokio::fs::write writes the full encoded Vec, so the peak includes jam bytes + payload + envelope simultaneously. crates/nockapp/src/nockapp/save.rs:532.

Incremental/Streaming Options

- Stream jam output into an io::Write (or async writer) instead of building a full BitVec/Bytes. Implement a bit‑writer that tracks current bit offset and uses the existing backref map; compute checksum/length as you write, then either (a) seek back to fill the
  header or (b) write a trailing footer with checksum/lengths. This drops output memory to O(backref‑map + small buffer).
  Touches crates/nockapp/src/noun/slab.rs:662 and checkpoint encoding in crates/nockapp/src/nockapp/save.rs:492.
- Skip NounSlab copying entirely and jam directly from live nouns in stack/PMA space. The NockJammer already takes a NounSpace, so you can jam from serf.arvo and serf.context.cold without heap duplication (still uses backref map + jam buffer). This needs a new
  “jammed checkpoint” path that bypasses SaveableCheckpoint.
  Entry points: crates/nockapp/src/kernel/form.rs:647, crates/nockapp/src/nockapp/save.rs:179.
- Chunked checkpoint format (new version): split state/cold into subtrees or size‑bounded chunks, jam each chunk independently, and store as a sequence of {size, bytes} records. This enables streaming and limits per‑chunk memory, at the cost of losing
  cross‑chunk backrefs (bigger output) and a new loader.
- PMA‑native snapshots: since PMA is bump‑allocated, store checkpoints by persisting only new PMA pages since last alloc_offset (delta snapshots), plus a tiny metadata file containing event number, kernel hash, and root offsets. This makes checkpointing
  incremental and avoids jam entirely, but changes the snapshot format and load path.
  PMA copy happens in crates/nockapp/src/kernel/form.rs:1418 and the PMA metadata is already persisted in crates/nockapp/src/kernel/form.rs:1582.

Peak Memory Mitigations (Non‑Incremental)

- Robust: avoid the extra jam copy by replacing Bytes::copy_from_slice(buffer.as_raw_slice()) with Bytes::from(buffer.into_vec()). That drops one full copy of jam output. crates/nockapp/src/noun/slab.rs:726.
- Robust: avoid double bincode buffers by encoding directly into a file/BufWriter (encode_into_std_write) or writing a custom header + jam bytes, so you don’t allocate payload and envelope Vecs. crates/nockapp/src/nockapp/save.rs:492.
- Robust: store state and cold as separate files (or separate sections with size prefixes) and have a tiny metadata file that references them; write each sequentially and drop memory between them. This reduces peak without a full streaming jam.
- Hacky: disable backrefs for checkpoint jam (always emit atoms/cells, no NounMap). Output grows, but memory drops (no backref map + simpler traversal), and it’s easy to stream.
- Hacky: allocate jam output in an mmap/memfd buffer so peak RSS pressure on the heap is reduced (still big, but file‑backed and reclaimable).
- Hacky: aggressively trim allocator arenas after checkpoint (malloc_trim, jemalloc purge) to reduce post‑checkpoint RSS even if peak remains high.

If you want, I can sketch concrete changes for one of the streaming paths (e.g., a jam_into<W: Write> + v3 checkpoint format) or prototype the lowest‑risk memory reductions (removing the extra jam copy + streaming bincode).
