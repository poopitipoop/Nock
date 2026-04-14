# PMA persistence massively drops RSS and it makes Chris paranoid

In this succession of git branches we've been working on a persistent memory arena for nockvm. This most recent successor branch was adding an option for using the PMA slab file directly for persistence instead of the checkpoints working alongside the PMA.
Seems like it's working now, but the memory statistics difference vs. the checkpointing version concerns me.

Comparison:
Metric                     PMA         Base    PMA - base
-----------------  -----------  -----------  ------------
VmRSS               1587.2 MiB  10870.4 MiB   -9283.2 MiB
VmSize             50861.9 MiB  26330.0 MiB  +24531.9 MiB
RssAnon             1177.9 MiB  10835.5 MiB   -9657.6 MiB
RssFile              409.3 MiB     34.8 MiB    +374.4 MiB
VmSwap                 0.0 MiB      0.0 MiB      +0.0 MiB
PMA map size       unavailable          n/a           n/a
PMA rss_ratio      unavailable          n/a           n/a
PMA alloc_offset       unknown          n/a           n/a
Checkpoint latest          n/a          n/a           n/a
Checkpoint total           n/a          n/a           n/a

The RSS numbers were a lot closer when the PMA version was still populating from a checkpoint. PMA used less, yes, but not 70-90% less.

I'm running both instances under Docker containers limited to 32 GiB of memory to induce uniform memory pressure on them. My concern is that the checkpointing version wasn't successfully paging out even as memory pressure mounted in the Docker container.
It would get close to 32 GiB RAM used while checkpointing several times before a checkpoint save finally triggered an OOM kill. Both instances were getting OOM killed by checkpointing.

## High‑likelihood hypotheses (and easy ways to falsify)

- H1: Checkpointing allocates huge anonymous buffers (slab copy + jam + bincode output), which are not reclaimable without swap; paging out PMA won’t help. Evidence: create_checkpoint + SerfCheckpoint::new copies kernel/cold into slabs (crates/nockapp/src/
  kernel/form.rs, crates/nockapp/src/noun/slab.rs), then jam + encode duplications (crates/nockapp/src/nockapp/save.rs). Falsify: watch /proc/self/smaps_rollup during a save; if Anonymous/Private_Dirty spikes massively while File doesn’t, this is it.
- H2: Checkpointing touches almost the entire PMA, bringing file‑backed pages resident and “active,” so the kernel can’t drop them mid‑copy. Evidence: slab copy walks the noun graph via PMA‑resolved pointers (crates/nockapp/src/noun/slab.rs), which faults pages
  in. Falsify: sample PMA residency (mincore/vmtouch) immediately before and during save; if residency jumps to near‑full during saves, that’s the trigger.
- H3: PMA pages are dirty and slow to write back, so they aren’t reclaimable under pressure. Evidence: PMA writes are MAP_SHARED and persist_metadata writes directly into the mapping without msync (crates/nockvm/rust/nockvm/src/pma.rs); repeated event updates
  dirty lots of PMA pages. Under overlayfs + cgroup limits, writeback may lag. Falsify: check cgroup memory.stat file_dirty/file_writeback and /proc/meminfo Dirty while saving.
- H4: The NockStack is anonymous mmap; once touched it’s unreclaimable without swap. Checkpointing is stack‑heavy (cold/state conversion), so anon RSS grows and sticks. Evidence: NockStack uses anon mapping (crates/nockvm/rust/nockvm/src/mem.rs) and is never
  madvise’d; cold conversion builds large stack nouns (crates/nockapp/src/kernel/form.rs). Falsify: track anon RSS after saves; if it never drops even after save completes, this is a main contributor.
- H5: Multiple large buffers exist concurrently during a save (state slab + cold slab + jam buffers + bincode envelope + fs write buffer), which can be 2–3× the live state size at peak. Evidence: SaveableCheckpoint::to_jammed_checkpoint +
  JammedCheckpointV2::encode allocate new Vecs (crates/nockapp/src/nockapp/save.rs). Falsify: use heap profiling (jemalloc/heaptrack) to see overlapping large allocations during save.

## Lower‑likelihood but plausible

- H6: Cgroup accounting charges file‑backed PMA pages against the limit, so you hit OOM before reclaim keeps up, especially while actively scanning the PMA. Falsify: inspect cgroup memory.stat file during saves; if it spikes alongside anon, this contributes.
- H7: THP / huge page effects make file‑backed or anon pages harder to reclaim under pressure. Falsify: check AnonHugePages/FilePmdMapped in /proc/self/smaps; rerun with THP disabled and compare.
- H8: Save scheduling causes long‑lived allocations because the background save holds on to slabs/jam longer than expected. Evidence: save tasks are async and gated by a mutex, but the checkpoint is created on the serf thread and then consumed later (crates/
  nockapp/src/nockapp/mod.rs). Falsify: log timestamps for checkpoint creation vs jam encode completion; if the overlap is large, peak rises.

## Why PMA‑persist looks so much lower

With PMA persistence on, you’re not doing the slab + jam path at all, so anonymous memory stays low. The file‑backed PMA can be cold, so RSS stays small even though VmSize is huge (expected for mmap).

---

Hypotheses (ranked by likelihood + ease to falsify)

1. Full‑state traversal faults PMA pages in during checkpoint.
    SaveableCheckpoint::new copies kernel_state and cold_state into NounSlab via copy_into, which walks everything and reads PMA pages (crates/nockapp/src/kernel/form.rs, crates/nockapp/src/noun/slab.rs). That alone can make a big PMA map fully resident while the slab allocations
    are growing.
    Falsify: measure RssFile/mincore just before/after a checkpoint or run with a tiny state and see if RSS stays low.
2. Jam serialization doubles memory on top of the slab copy.
    SaveableCheckpoint::to_jammed_checkpoint builds two large Vec<u8> (state + cold) while the NounSlab copies still exist (crates/nockapp/src/nockapp/save.rs). That’s another full‑state footprint, and it’s anonymous memory (hard to reclaim without swap).
    Falsify: skip jam (or stream it) and see if OOM goes away.
3. Dirty PMA pages are hard to reclaim under pressure.
    We write PMA via MmapMut without msync/madvise in production; dirty pages can’t be evicted until writeback keeps up (crates/nockvm/rust/nockvm/src/pma.rs, crates/nockvm/rust/nockvm/src/mem.rs). Under checkpoint pressure, the kernel may not reclaim fast enough.
    Falsify: watch Dirty in /proc/<pid>/smaps for the PMA mapping; add a temporary msync/madvise(MADV_PAGEOUT) and see if it changes the spike.
4. PMA size == stack size; a large stack means a huge file map.
    PMA is created with the same words as stack size (PmaConfig in crates/nockapp/src/kernel/boot.rs, crates/nockapp/src/kernel/form.rs). If you’re on --stack-size medium/large/huge, the PMA file is 16–64 GiB and a checkpoint scan can fault a lot of that into RSS.
    Falsify: run --stack-size small/normal and compare checkpoint RSS.
5. No swap in the container → anon pages can’t be reclaimed.
    NockStack + NounSlab + jam buffers are anonymous. In Docker with --memory and no swap, these can’t be evicted. Even if PMA pages are reclaimable, anon growth can still trigger OOM first.
    Falsify: enable swap (--memory-swap) or run outside Docker.
6. Allocator retention after checkpoint.
    NounSlab deallocates, but the allocator may keep large arenas around; RSS stays high across checkpoints and you eventually OOM.
    Falsify: check RSS after checkpoint completes; experiment with MALLOC_TRIM_THRESHOLD_ or jemalloc decay.
7. Cgroup page‑cache accounting.
    PMA is file‑backed and charged to the container (RssFile), so page‑cache reclaim is constrained by cgroup limits. Under heavy dirty/writeback it may OOM rather than evict.
    Falsify: inspect memory.stat for file/anon and pgscan/pgsteal during checkpoint.

If you want to dig deeper without changing behavior yet, I can help design instrumentation to answer: “how many PMA pages are resident during checkpoint” and “how big are the slab + jam allocations” so we can pinpoint the dominant factor.

Suggested next steps:

1. Capture smaps/memory.stat snapshots right before and during a checkpoint to confirm whether PMA RssFile or anon slabs/jam are the main spike.
2. Temporarily short‑circuit jam or the slab copy to isolate which phase causes the OOM.
3. Try a smaller stack size to see if the PMA mapping size is the dominant driver.

---

Comprehensive Analysis: Why Kernel Can't Page Out PMA During Checkpoint OOM

Based on my exhaustive review of the codebase, here are my hypotheses ranked by likelihood and ease of falsification:

H1: Anonymous Buffers Are Unswappable (HIGHEST LIKELIHOOD)

Evidence:
- SerfCheckpoint::new at form.rs:1950-1994 creates two NounSlab instances via copy_into()
- NounSlab allocates via std::alloc::alloc() at slab.rs:72 - this is heap memory, which is anonymous
- Then SaveableCheckpoint::to_jammed_checkpoint at save.rs:246-260 creates BitVec buffers during jam()
- Finally JammedCheckpointV2::encode() creates another Vec<u8> for bincode output

Memory footprint at peak:
state_slab (~state size) + cold_slab (~cold size) +
jam_bitvec_state (~state size) + jam_bitvec_cold (~cold size) +
bincode_vec (~total jam size)
This can be 2-3× the live state size in pure anonymous memory.

Why it causes OOM: Without swap, anonymous memory cannot be reclaimed by the kernel under any circumstances. Even if the kernel could page out the entire PMA, these anonymous buffers would remain resident.

Falsification: Monitor /proc/self/smaps_rollup during checkpoint save. If Private_Anonymous spikes to multi-GB while Private_File stays relatively flat, this confirms the hypothesis.

---
H2: PMA Traversal Faults All Pages Resident (HIGH LIKELIHOOD)

Evidence:
The copy_into function at slab.rs:286-347 walks the noun graph:
while let Some((noun, dest)) = copy_stack.pop() {
    // ...
    let indirect_ptr = unsafe { indirect.as_atom().in_space(space).raw_pointer() };
    // ^^^ This dereferences PMA pointers, faulting pages in

Each pointer dereference in the PMA:
1. Triggers a page fault if the page isn't resident
2. Marks the page as accessed/active in the kernel's page aging algorithm
3. Makes it a poor candidate for reclaim during the save

Why it causes OOM: Even though PMA pages are file-backed and theoretically reclaimable, the kernel's LRU aging algorithm sees them all as "recently used" during the copy. Under memory pressure + cgroup limits, the kernel prefers to reclaim pages that were touched longer ago - but you just touched them all.

Falsification: Use mincore() or vmtouch to sample PMA residency immediately before and during a checkpoint save. If residency jumps from ~10% to ~90%+ during saves, this is the trigger.

---
H3: Dirty PMA Pages Must Write Back Before Eviction (HIGH LIKELIHOOD)

Evidence:
- persist_metadata() at pma.rs:344-360 writes directly into the mmap'd region:
unsafe {
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
}
- This happens on every allocation (lines 135, 192, 253, 298, 313, 340)
- No msync() is ever called on the PMA in production code
- The documentation at persistence.md:43 describes the intended msync-based commit, but it's not implemented

Why it causes OOM: For MAP_SHARED mappings, dirty pages cannot simply be discarded - the kernel must write them back to the file first. Under:
- cgroup memory limits (Docker 32GB limit)
- overlayfs (Docker storage driver)
- high dirty page count (continuous metadata writes)

...writeback becomes the bottleneck. The kernel can't evict pages faster than writeback I/O allows.

Falsification: Check memory.stat in the cgroup during saves:
cat /sys/fs/cgroup/memory/docker/<container>/memory.stat | grep -E 'dirty|writeback'
If dirty and writeback are high (>1GB) during OOM, this contributes.

---
H4: NockStack Anonymous Memory Never Releases (MODERATE-HIGH LIKELIHOOD)

Evidence:
- Arena::allocate at mem.rs:290-302 uses MmapMut::map_anon(bytes)
- Stack size is NOCK_STACK_SIZE (likely hundreds of MB to several GB)
- Cold state conversion at form.rs:1962 builds large stack nouns:
let cold_stack_noun = cold_state.into_noun(stack);
- No madvise(MADV_DONTNEED) is ever called on the stack after operations complete

Why it causes OOM: Once touched, anonymous mmap pages are resident until the process exits. The NockStack grows during checkpoint conversion and stays inflated. This compounds with H1.

Falsification: Track RssAnon in /proc/self/status before and after checkpoint saves. If it never decreases even after save completes and stack frame pops, this confirms it.

---
H5: Concurrent Buffer Lifetimes at Peak (MODERATE LIKELIHOOD)

Evidence:
The save flow at save.rs:193-225 shows overlapping lifetimes:
let saveable = checkpoint.to_saveable();        // state_slab + cold_slab still alive
let jammed = saveable.to_jammed_checkpoint();   // + jam buffers now alive
jammed.save_to_file(&path).await?;              // + file write buffer
std::mem::drop(jammed);                         // finally dropped at line 207

Why it causes OOM: The peak memory moment is just before save_to_file completes - all buffers coexist. If state is 3GB, you might need 6-9GB of anonymous memory simultaneously.

Falsification: Use heaptrack or jemalloc profiling to visualize overlapping allocations. Look for a "memory mountain" during checkpoint saves.

---
H6: Cgroup Accounting Charges File Pages (MODERATE LIKELIHOOD)

Evidence:
- Docker uses cgroup memory limits (32GB in your test)
- File-backed pages (PMA) are charged against the cgroup limit by default (memory.use_hierarchy)
- Under pressure, the cgroup sees "32GB used" even though much is reclaimable file cache

Why it causes OOM: The cgroup OOM killer triggers when memory.usage_in_bytes >= memory.limit_in_bytes, regardless of how much is actually reclaimable. If the kernel can't reclaim fast enough during allocation spike, OOM triggers.

Falsification: Check memory.stat for cache vs rss during the OOM. If cache is high (many GB), the cgroup is counting file pages.

---
H7: Transparent Huge Pages Impede Reclaim (LOWER LIKELIHOOD)

Evidence:
- THP can be enabled by default on Linux
- Large contiguous allocations (like the NockStack) might get backed by 2MB huge pages
- Huge pages are harder to split and reclaim under pressure

Falsification: Check /proc/self/smaps for AnonHugePages. If it's significant, test with THP disabled:
echo never > /sys/kernel/mm/transparent_hugepage/enabled

---
H8: Save Task Async Scheduling Delays Drop (LOWER LIKELIHOOD)

Evidence:
- The checkpoint is created on the serf thread at form.rs:642
- But save happens asynchronously at save.rs:193-225
- The checkpoint might be held in memory longer than necessary if the async task is delayed

Falsification: Add timestamps to checkpoint creation vs completion. If the gap is large (>seconds), this extends peak memory duration.

---
Why PMA-Persist Has 85% Lower RSS

With NOCK_PMA_PERSIST=1:
1. No slab copies - checkpoint path is skipped entirely
2. No jam/bincode encoding - state stays in file-backed form
3. VmSize is huge (50GB) but VmRSS is tiny (1.6GB) because file-backed pages can be cold
4. The kernel can evict PMA pages freely since there's no checkpoint traversal keeping them "active"

The 50GB VmSize is just the mmap'd region - only 1.6GB is actually resident because unused pages stay on disk.

---
Recommended Investigation Order

1. Easiest/Fastest: Monitor smaps_rollup during checkpoint - if Private_Anonymous >> Private_File, H1 is confirmed
2. Second: Use mincore/vmtouch before/during save to check PMA residency spike (H2)
3. Third: Check cgroup dirty/writeback during saves (H3)
4. Fourth: Track RssAnon before/after saves over time (H4)
