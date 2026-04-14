# Dynamic NockStack Growth Options (Current Architecture Constraints)

Here’s a comprehensive, code‑grounded review of how the NockStack works today and what that implies for *dynamic growth*, plus options with pros/cons and what I think makes the most sense for this PMA branch.

**What the code actually guarantees today (constraints)**  
- The NockStack is a **single contiguous arena** (`Arena::allocate`), with a **fixed base pointer** for the lifetime of the stack.  
- Internally, `NockStack` uses **offsets** (`frame_offset`, `stack_offset`, `alloc_offset`) for stack/alloc movement, but **noun references are raw pointers**, not offsets.  
  - `TaggedPtr::from_stack_ptr(ptr, tag)` stores `ptr >> 3` when `LOCATION_BIT` is 0.  
  - `NounSpace::resolve_stack_ptr` reconstructs `ptr = payload << 3` and validates it against the arena’s base/end.  
  - That means **stack nouns are absolute addresses**; they are *derived* from offsets but stored as pointers.  
- Therefore, **any growth strategy that changes the base address invalidates live stack nouns.**  
  That is the primary constraint.

---

## Option 1: Reserve a big virtual region, commit on demand (base never moves)
**Idea:** `mmap` a large region with `PROT_NONE` (and optionally `MAP_NORESERVE` on Linux), then `mprotect`/`mmap(MAP_FIXED)` to commit more pages as the stack grows. Base stays constant, so stack noun pointers remain valid.

**Pros**
- Preserves existing pointer invariants; no retagging needed.
- Compatible with offset‑based stack internals.
- Works on Linux + macOS (with some OS‑specific handling).
- Gives “dynamic growth” without moving.

**Cons**
- Still a **max cap** (the reserved size).
- VmSize becomes large, which can confuse tooling.
- Requires custom mapping logic (memmap2 doesn’t handle “grow in place”).

**Fit for PMA branch:** **Best balance.** Keeps the pointer model intact and avoids large refactors.

---

## Option 2: `mremap` in place (Linux‑only)
**Idea:** Use Linux `mremap` without `MREMAP_MAYMOVE` to grow the mapping if the adjacent space is free.

**Pros**
- Minimal internal change, still contiguous.
- Base remains stable.

**Cons**
- Linux‑only.
- Can fail if address space after the mapping is not free.
- Not a real solution for macOS.

**Fit:** Useful as a Linux optimization, but not sufficient for cross‑platform.

---

## Option 3: Grow by reallocating and retagging at safe points
**Idea:** Allocate a new larger stack and **copy/retag** live stack nouns to new addresses. In practice, you can only do this **at a safe point** (event boundary), not mid‑slam.

**Pros**
- Works even without huge virtual reservations.
- Cross‑platform.

**Cons**
- Requires a root set and a copy/retag pass.  
- Not safe mid‑slam (you’d need to stop the world and rebuild).  
- Adds complexity and increases event latency.

**Fit:** Could be a fallback strategy if you treat it like an “event‑boundary stack resize,” but it’s heavyweight.

---

## Option 4: Multi‑segment stack (non‑contiguous arenas)
**Idea:** Turn the stack into multiple arenas and allow noun pointers to reference any segment. Use `extra_ptr_ranges` to validate.

**Pros**
- True dynamic growth by adding segments.
- No relocation.

**Cons**
- NockStack frame/stack/alloc logic assumes a single contiguous slab.  
- You’d need to rework frame/layout math, `push`/`pop`, and allocation logic.  
- Pointer tagging doesn’t encode a segment id; you’d still need range tables or a new tag scheme.

**Fit:** Big redesign; not aligned with the “get PMA paging working” focus.

---

## Option 5: Increase the fixed size (status quo + more headroom)
**Idea:** Choose a larger `--stack-size` (e.g. 32 GiB), accept a fixed slab but reduce OOM risk.

**Pros**
- Zero code change.
- Matches current architecture.
- Simple operationally.

**Cons**
- VmSize grows; RSS high‑water sticks.
- Doesn’t actually “grow,” just a bigger fixed cap.

**Fit:** Good operational workaround, not a dynamic growth solution.

---

## Option 6 (not growth, but helpful): Decommit on reset
**Idea:** On `stack.reset` or after `preserve_event_update_leftovers`, `madvise(MADV_DONTNEED)` the freed range.

**Pros**
- Reduces RSS high‑water for the ephemeral stack.
- Works with current model.

**Cons**
- Doesn’t increase capacity; just helps RSS/pressure.
- Extra syscalls and performance cost.

**Fit:** Good complement if RSS is the pain point, but not “growth.”

---

## What makes the most sense for the PMA branch
Given the goals (paging out persistent state, keep pointer model stable, cross‑platform), **Option 1 is the cleanest long‑term answer**: reserve a large virtual range and commit on demand so the base address never changes. It gives “dynamic” growth without touching noun representation or stack semantics.

If that’s too much right now, **Option 5 (bigger fixed stack)** plus **Option 6 (decommit on reset)** gives you a pragmatic path: fewer OOMs and less RSS pressure without redesign.

If you ever want true unbounded growth, the only robust path is a **segment‑aware pointer scheme** (Option 4), which is a major architecture shift.

---

If you want, I can sketch a concrete design for Option 1 (mapping layout, growth thresholds, and how it interacts with `least_space` and `alloc_would_oom`), but I’ll keep it at discussion level unless you ask for changes.
