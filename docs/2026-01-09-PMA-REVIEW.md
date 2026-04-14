# 2026-01-09 Preliminary review notes

- NounSpace seems reasonable, but file is baked into the shared `Arena` type which is used for both a stack and the PMA.
- `memfd` appears to still be getting used by default for `NockStack::new` and `new_` which seems wrong.
  * `memfd` was used expressly for the stack because the stack is ephemeral, but we should probably go back to the anonymous mmap slab or malloc.
  * Update: `NockStack` is back to using map_anon and malloc now. Tests passed.
- The PMA currently (by design) encompasses more than just `arvo`, it includes cold/warm/open/testjets, etc. Shouldn't be problematic AFAIK.
- The addition of `space: &NounSpace` to a bunch of methods/functions seems pointless in cases where there's already a `noun: Noun` argument:

```diff
-pub fn mug_u32_one(mut noun: Noun) -> Option<u32> {
+pub fn mug_u32_one(mut noun: Noun, space: &NounSpace) -> Option<u32> {
```

I noticed changes like this in the GitHub PR diff.

However,

```rust
  #[derive(Copy, Clone)]
  pub struct NounHandle<'a> {
      noun: Noun,
      space: &'a NounSpace,
  }
```

It might make more sense to change the `mut noun: Noun` parameter to `noun: &mut NounHandle` or `noun: &NounHandle` instead of adding a second parameter for `space: &NounSpace`. I'm investigating.

- `alloc_would_oom` in `Pma` is currently just a panic, we need to make it able to reallocate and grow itself safely.
