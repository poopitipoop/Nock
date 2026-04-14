# Incremental PMA GC Options (nockvm in this branch)

This is a comprehensive, code‑grounded survey of **incremental GC options for the PMA** as implemented in this branch, with trade‑offs and expected outcomes specific to nockvm’s current design.

## Current constraints in this branch (why options are limited)

- **Single PMA arena, single location bit**: `TaggedPtr` only distinguishes *stack pointer* vs *PMA offset*. `NounSpace` only carries a single PMA arena reference. Anything that needs two PMAs (from‑space / to‑space) will need either:
  - another tag bit, or
  - a multi‑arena resolution scheme in `NounSpace`.
- **PMA is bump‑only / append‑only**: `Pma::alloc_*` only advances `alloc_offset`; no free list or reuse.
- **Copying currently mutates the source**: `copy_to_pma` uses forwarding pointers, which is fine for stack data but not acceptable as a stable PMA state.
- **PMA objects don’t have universal size headers**: indirect atoms have explicit size metadata, cells are fixed size, but custom PMA structs (WarmEntry, Batteries, Cold, etc.) do **not** carry sizes. That makes sweep or compaction harder without extra metadata.

These constraints define which GC approaches are viable without changing the noun/tag scheme.

---

## Option 1: Non‑moving mark‑sweep + free list (single PMA)

**Idea:** Keep one PMA arena. Mark reachable objects from the live roots (`warm/test_jets/hot/cache/cold/arvo`), then sweep and build a free list. Allocate from the free list before bumping `alloc_offset`.

**Pros**
- Compatible with current **single‑tag PMA**.
- No pointer relocation: existing offsets remain valid.
- Can be made incremental (tri‑color marking + chunked sweeping).

**Cons**
- Requires **size metadata** for every PMA allocation (cells, atoms, and all custom PMA structs).
- Fragmentation likely; allocator becomes more complex.
- Incremental correctness is tricky with existing mutability patterns.

**Expected outcome for nockvm**
- **Works without changing noun tags** and without multiple PMAs.
- Moderate engineering effort to add size tracking and a free list.
- Lower memory growth but potential fragmentation and more allocator overhead.

---

## Option 2: Incremental copying collector (two‑space, from‑PMA → to‑PMA)

**Idea:** Allocate a new PMA and gradually evacuate live data into it. Use a forwarding table or per‑object forwarding to handle old references until the copy is complete, then swap PMA arenas.

**Pros**
- **Compaction**: removes fragmentation.
- Allocation stays fast (bump‑only).
- Naturally incremental: copy a bounded amount per event.

**Cons**
- Requires **two PMAs at once**, which is **blocked by the single location bit** unless you change tagging or make `NounSpace` multi‑arena aware.
- Requires **read barriers** or forwarding logic in PMA (currently treated as invalid state).
- Temporary 2× memory footprint during evacuation.

**Expected outcome for nockvm**
- Strong long‑term memory behavior if you’re willing to extend the tag scheme.
- Larger architectural change than Option 1.

---

## Option 3: Segmented / region‑based PMA (coarse incremental GC)

**Idea:** Split the PMA into fixed‑size segments. Track live bytes per segment; when a segment’s live ratio is low, evacuate it and reclaim the whole segment.

**Pros**
- Reclaims in large chunks with **bounded overhead**.
- Less fragmentation than free‑list mark‑sweep.
- Incremental in “segment units”.

**Cons**
- Still requires **relocation** and forwarding (like Option 2).
- Needs per‑segment liveness accounting (marking).
- Requires a multi‑arena or segmented addressing scheme.

**Expected outcome for nockvm**
- Could be efficient, but still blocked by single‑PMA tag scheme.
- Higher engineering cost than Option 1.

---

## Option 4: Generational PMA (young/old regions)

**Idea:** Allocate in a young PMA region and promote to old; most events only touch young. Use remembered sets for old → young refs.

**Pros**
- Great incremental performance if most data dies young.
- Reduces amount of work per event.

**Cons**
- Requires multiple PMAs or multi‑region tagging.
- Needs write barriers (old → young pointers).
- Large refactor across noun operations.

**Expected outcome for nockvm**
- Potentially excellent for throughput, but **incompatible with current single‑tag PMA** and high complexity.

---

## Option 5: Periodic stop‑the‑world PMA rebuild (non‑incremental)

**Idea:** When PMA reaches a threshold, build a new PMA by copying all live roots once, then swap and discard old.

**Pros**
- Very low implementation risk (reuses `copy_to_pma`).
- No need for size metadata or free list.
- Minimal changes to noun representation.

**Cons**
- Not incremental; **large pause** during rebuild.
- Requires temporary 2× memory.
- Still blocked by single‑PMA tags unless you do a stop‑the‑world swap immediately.

**Expected outcome for nockvm**
- Easiest to ship quickly; useful as a pragmatic “pressure release valve”.
- Doesn’t help latency‑sensitive scenarios.

---

# Recommended path by constraint set

**If you want incremental GC without changing tagging:**  
**Option 1 (non‑moving mark‑sweep + free list)** is the only realistic path that fits the current PMA design.

**If you can extend tagging / NounSpace to support multiple PMAs:**  
**Option 2 (incremental copying)** gives the best long‑term behavior (compaction, fast allocation).

**If you want a low‑risk, short‑term safety valve:**  
**Option 5 (periodic rebuild)** is the simplest to ship but comes with large pauses.

---

If you want, I can sketch how mark‑sweep or incremental copying would hook into `PmaCopy` and `NounSpace` at a design level without changing code.
