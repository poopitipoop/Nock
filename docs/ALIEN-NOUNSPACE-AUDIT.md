# Alien `Noun` / `NounSpace` Audit

Date: 2026-04-09

Baseline:
- Audited after commit `d612d04` (`Fixing more sources of alien pointer errors`).
- I treated the recent fixes in `crates/nockapp/src/noun/slab.rs`, `crates/nockchain-libp2p-io/src/messages.rs`, `crates/nockapp/src/utils/error.rs`, and `crates/nockapp/src/utils/scry.rs` as the new baseline and looked for the same bug family elsewhere.

Scope:
- I audited for APIs that let a raw `Noun` escape its owning arena, reconstruct a `NounSpace` manually, or rebuild new noun structure from foreign raw nouns without copying.
- I did not make code changes in this pass. This is a findings-only review.

## Executive Summary

I found one concrete dangling-noun bug and several broader provenance-erasure APIs that can recreate the exact runtime failure class we just fixed.

The single most severe concrete bug is:
- `crates/nockapp/src/noun/extensions.rs`: `IntoNoun for &str` allocates into a temporary `NounSlab` and returns a raw `Noun` after that slab is dropped.

The broader systemic problems are:
- `noun-serde` still treats raw `Noun` as an identity encode/decode type.
- `nockvm::jets::cold` still treats raw `Noun` as a first-class decoded payload.
- `nockchain-math` and `zkvm-jetpack` still convert structured data into `Vec<Noun>` / `Option<Noun>` and then rebuild new nouns from those raw values without copying.
- `bridge::signing` reconstructs a `NounSpace` from raw pointers instead of carrying the owner, and it rejects PMA offset-form nouns.

I did not find another already-triggering production path as obvious as the libp2p message bug we just fixed. Most remaining issues are latent or “one refactor away” bugs. They are still real PMA blockers because the API surfaces are wrong.

## Findings

### 1. Critical: `&str -> Noun` returns a dangling noun

Files:
- `crates/nockapp/src/noun/extensions.rs:69`
- `crates/nockapp/src/noun/extensions.rs:124`

What is wrong:
- `IntoNoun for &str` creates a fresh `NounSlab`, allocates an indirect atom into it, returns the raw `Noun`, and then drops the slab immediately.
- That returned noun points into freed slab memory.
- `IntoSlab for &str` is also wrong because it calls the broken `into_noun()` first, then installs that foreign/dangling noun as the root of a different slab with `set_root()`.

Why this is the same bug family:
- This is worse than a plain `Noun`/`NounSpace` mismatch. It is an actual use-after-free constructor for raw nouns.
- Any later dereference will rely on a `NounSpace` that cannot possibly be correct because the backing slab is already gone.

Current reachability:
- I found no current non-test call site for `IntoNoun for &str`.
- I found no current production call site for `IntoSlab for &str`; `nockchain/src/setup.rs` imports `IntoSlab`, but the live use there is `BlockchainConstants::into_slab()`, not `&str::into_slab()`.

Assessment:
- Latent, but unquestionably broken and should be treated as a must-fix.

### 2. High: `noun-serde` still has raw-`Noun` passthrough encode/decode

Files:
- `crates/noun-serde/src/lib.rs:113`
- `crates/noun-serde/src/lib.rs:119`

What is wrong:
- `impl NounEncode for Noun` returns `*self` without copying into the destination allocator.
- `impl NounDecode for Noun` returns `*noun` without preserving provenance.

Why this is the same bug family:
- The decode side erases arena provenance at the boundary where typed data is supposed to become safe Rust values.
- The encode side can smuggle a foreign noun into a newly allocated structure without copying it first.
- This recreates the exact “raw noun escaped from its original space” bug class.

Concrete downstream uses I found:
- `crates/nockapp-grpc/src/services/public_nockchain/v2/block_explorer.rs:1384`
  - `PageAndTxs { txs: Noun }`
- `crates/nockapp-grpc/src/services/public_nockchain/v2/block_explorer.rs:2041`
  - `FullPageData { txs: Noun }`
- `crates/nockapp-grpc/src/services/public_nockchain/v2/block_explorer.rs:2049`
  - `FullPageNoun { version_or_digest: Noun, rest: Noun }`

Current reachability:
- The block explorer path is live and currently decodes raw nouns through this escape hatch.
- In the current code, those raw nouns are consumed immediately with the same `space`, so I did not prove a current crash there.
- I did not find a production struct deriving `NounEncode` with a raw `Noun` field outside tests, so the encode half looks mostly latent today.

Assessment:
- Live unsafe boundary on decode.
- Encode half is a structural bug even if current production use is limited.

### 3. High: `FullPageNoun` in the block explorer manually reintroduces raw nouns

Files:
- `crates/nockapp-grpc/src/services/public_nockchain/v2/block_explorer.rs:2049`
- `crates/nockapp-grpc/src/services/public_nockchain/v2/block_explorer.rs:2056`

What is wrong:
- `FullPageNoun` is not just inheriting the raw-`Noun` problem from `noun-serde`.
- Its custom `NounDecode` impl explicitly extracts `version_or_digest` and `rest` as raw `Noun`.

Why this matters:
- Even if `noun-serde` stopped decoding `Noun` by identity, this custom decoder would still be an escape hatch.
- The same applies to the block-range decoder’s `txs: Noun` and full-page decoder’s `txs: Noun`.

Current reachability:
- Live code path in the public block explorer.
- Current implementation reuses the same `space` immediately, so I am classifying this as fragile/live-hazard rather than proven-crash.

Assessment:
- Separate from the generic `noun-serde` problem because this module explicitly opted back into raw nouns.

### 4. High: `nockvm::jets::cold` still decodes and iterates raw nouns

Files:
- `crates/nockvm/rust/nockvm/src/jets/cold.rs:932`
- `crates/nockvm/rust/nockvm/src/jets/cold.rs:1019`

What is wrong:
- `NounListIterator<'a>` stores a raw `Noun` and yields `type Item = Noun`.
- `impl Nounable for Noun` returns raw nouns unchanged from `from_noun()` and `into_noun()`.
- This means generic cold-state decoding can freely erase provenance whenever a field type is `Noun`.

Why this is the same bug family:
- `cold` is a typed decode/encode layer, and it still treats raw `Noun` as an identity value.
- That is the same mistake as the old message helper and scry helper bugs, just in a different API.

Current reachability:
- The main snapshot load path in `crates/nockapp/src/kernel/form.rs:1591` through `:1596` first copies `saveable.cold` into the current stack and only then runs `Cold::from_noun(...)` / `Cold::from_vecs(...)`.
- Because of that copy, I did not find a currently live mismatch in the kernel cold-load path itself.

Assessment:
- High-confidence systemic bug source.
- Not currently proven to be crashing in the kernel path I checked.

### 5. High: `nockchain-math` containers still downgrade handles to raw nouns

Files:
- `crates/nockchain-math/src/structs.rs:8`
- `crates/nockchain-math/src/structs.rs:14`
- `crates/nockchain-math/src/structs.rs:86`
- `crates/nockchain-math/src/structs.rs:94`

What is wrong:
- `HoonList<'a>` stores `Option<Noun>` and yields `type Item = Noun`.
- `HoonMap<'a>` stores raw `Noun` nodes and children and returns `Option<Noun>` from `get()`.

Why this is the same bug family:
- These helpers accept a branded source (`&NounSpace` / `NounHandle`) and immediately throw away the provenance.
- The resulting raw nouns get threaded through higher-level math and jet code.

Concrete downstream uses I found:
- `crates/zkvm-jetpack/src/jets/proof_gen_jets.rs:22`
  - `MPUltra::Mega(Noun)`
- `crates/zkvm-jetpack/src/jets/proof_gen_jets.rs:27`
  - `MPComp { dep: Vec<Noun>, com: Vec<Noun> }`
- `crates/zkvm-jetpack/src/form/math/gen_trace.rs:11`
  - `TreeData { n: Noun }`

Current reachability:
- These are live helpers and live call sites.
- In the call sites I checked, the raw nouns appear to stay within the same jet stack and same `space`, so I did not prove a current alien-pointer crash.

Assessment:
- Live hazard, especially because this code is widely reused in the jetpack math stack.

### 6. High: `zkvm-jetpack` can rebuild fresh lists/tuples from foreign raw nouns without copying

Files:
- `crates/zkvm-jetpack/src/utils.rs:18`
- `crates/zkvm-jetpack/src/utils.rs:49`
- `crates/zkvm-jetpack/src/utils.rs:115`

What is wrong:
- `hoon_list_to_vecnoun()` extracts a `Vec<Noun>` from an input list.
- `vecnoun_to_hoon_list()` and `vecnoun_to_hoon_tuple()` then build new cells in a destination stack using those raw nouns directly.
- No copy happens into the destination allocator.

Why this is the same bug family:
- This is the encode-side version of the problem: raw nouns from one arena can be embedded into fresh cells allocated in another arena.
- That creates mixed trees and alien pointers immediately.

Concrete dependent code:
- `crates/zkvm-jetpack/src/jets/tip5_jets.rs:153`
  - `hash_pairs()` uses `hoon_list_to_vecnoun()` and then `vecnoun_to_hoon_list()`
- `crates/zkvm-jetpack/src/jets/tip5_jets.rs:245`
  - `hash_hashable_list()` collects raw nouns and rebuilds a list
- `crates/zkvm-jetpack/src/jets/mary_jets.rs:391`
  - `heapify_mary()` builds `Vec<Noun>` and then `vecnoun_to_hoon_list()`
- `crates/zkvm-jetpack/src/jets/mary_jets.rs:497`
  - `mary_to_list_fields()` builds `Vec<Noun>` and then `vecnoun_to_hoon_list()`

Current reachability:
- The current jet call sites look intra-stack, so they are probably not exploding today.
- The helper APIs themselves are wrong and would break immediately if fed foreign nouns.

Assessment:
- High-confidence latent/runtime hazard.
- This is the most obvious encode-side raw-noun bug I found outside `noun-serde`.

### 7. Medium-High: bridge signing reconstructs `NounSpace` manually and is PMA-incompatible

Files:
- `crates/bridge/src/signing.rs:33`
- `crates/bridge/src/signing.rs:102`
- `crates/bridge/src/signing.rs:122`
- `crates/bridge/src/signing.rs:139`

What is wrong:
- `sign_proposal(&self, proposal_hash_noun: Noun)` takes a raw noun without its owner.
- `proposal_hash_noun_space()` walks raw pointers and rebuilds a `NounSpace` via `NounSpace::empty().with_extra_ptr_ranges(...)`.
- The walker explicitly rejects offset-form PMA nouns.

Why this is the same bug family:
- This API reconstructs provenance after the fact instead of carrying it with the noun.
- That is exactly the class of workaround PMA is supposed to eliminate.

Current reachability:
- I found only test call sites for `sign_proposal(...)`.
- The main runtime signing path appears to use `sign_hash(...)` directly.
- Even so, this is not a theoretical issue: the function will reject PMA-backed proposal hashes with:
  - `"proposal-hash noun uses unsupported offset-form atom"`
  - `"proposal-hash noun uses unsupported offset-form cell"`

Assessment:
- Latent/test-only today, but incompatible with PMA-style nouns and structurally unsafe.

## Reviewed But Not Flagged As Current Bugs

These were worth checking because they looked similar, but I do not currently think they are active mismatch bugs:

- `crates/nockapp/src/utils/scry.rs`
  - The recent `ScryResult::Some(NounHandle<'a>)` change looks correct.
- `crates/nockchain-libp2p-io/src/messages.rs`
  - The recent `NounHandle` changes for block-id / tx-id extraction look correct.
- `crates/nockchain-libp2p-io/src/driver.rs:1583`
  - `prepend_tas(...)` still takes `Vec<Noun>`, but its only current caller imports the noun into `res_slab` first.
- `crates/bridge/src/types.rs:573`
  - `NockchainBlockCause` carries `page_noun` together with its owning `page_slab`, and its `NounEncode` impl copies through that slab’s `noun_space()`. That pattern looks correct on this axis.
- `crates/bridge/src/nockchain.rs:106`
  - `decode_page_from_peek(...)` returns `(Page, Noun)`, but the live caller keeps the accompanying `page_slab` and stores both together in `NockBlockEvent`.
- `crates/nockapp/src/kernel/form.rs:1591`
  - The current cold-state load path copies `saveable.cold` into the active stack before `Cold::from_noun(...)`, so I did not flag it as a live mismatch even though the `cold` API remains unsafe.

## Priority Order

If the goal is to finish the PMA effort without taking the full branded-handle API churn right now, I would prioritize fixes in this order:

1. `crates/nockapp/src/noun/extensions.rs`
   - This is a real dangling-pointer bug.
2. `crates/noun-serde/src/lib.rs`
   - Remove raw-`Noun` identity encode/decode or fence it off behind explicitly unsafe/manual APIs.
3. `crates/nockvm/rust/nockvm/src/jets/cold.rs`
   - Stop using raw `Noun` as a decoded/iterated value type.
4. `crates/nockchain-math/src/structs.rs` and `crates/zkvm-jetpack/src/utils.rs`
   - Stop converting structured data into `Vec<Noun>` / `Option<Noun>` unless the owning slab/stack is carried with it, or copy on re-encode.
5. `crates/bridge/src/signing.rs`
   - Replace manual `NounSpace` reconstruction with an API that takes an owning slab/handle.

## Bottom Line

The recent fixes closed the concrete bugs that were already biting the libp2p, scry, and kernel-error paths. The remaining PMA risk is now concentrated in a smaller set of raw-`Noun` escape hatches.

The codebase is not yet “completely correct” on this axis. The biggest remaining blockers are the raw noun passthrough APIs in `noun-serde`, `cold`, and the math/jetpack list helpers, plus the outright broken `&str -> Noun` constructor.

## Addendum After Commit `7c674ec`

I continued the audit from the committed baseline above and found additional slab-side and math-side escape hatches. I did not modify any Rust code in this pass.

### 8. High: `NounSlab` still has raw-noun constructors/mutators that can install mixed trees

Files:
- `crates/nockapp/src/noun/slab.rs:88`
- `crates/nockapp/src/noun/slab.rs:94`
- `crates/nockapp/src/noun/slab.rs:251`
- `crates/nockapp/src/noun/slab.rs:425`
- `crates/nockchain-wallet/src/main.rs:513`

What is wrong:
- `NounSlab::modify(...)` passes the raw current root into a closure, accepts `Vec<Noun>` back, and builds a fresh tuple in the destination slab without copying any returned nouns first.
- `NounSlab::modify_noun(...)` accepts a raw `Noun` from the closure and roots it directly.
- `impl From<[Noun; N]> for NounSlab` builds a fresh root tuple from raw array elements without importing them.
- `NounSlab::set_root(...)` only checks whether the top-level root pointer lives in the slab. It does not verify that all reachable descendants do. A fresh in-slab cell wrapping foreign children therefore passes validation.

Why this is the same bug family:
- This recreates the exact mixed-tree / alien-pointer class that `modify_with_imports3(...)` had before the recent fix.
- The shallow `set_root(...)` validation is the reason these helpers can silently succeed even when descendants still belong to a different arena.

Current reachability:
- The current `modify(...)` / `modify_noun(...)` call sites I checked appear safe today because they only reuse the slab’s own root or direct atoms.
- `impl From<[Noun; N]> for NounSlab` is only used in tests right now.
- The private wallet helper at `crates/nockchain-wallet/src/main.rs:513` repeats the same pattern locally by accepting `args: &[Noun]` and feeding them straight into `T(slab, args)`. Its current callers build those args in the same slab, so I did not flag it as a live crash.

Assessment:
- High-confidence latent bug source.
- This is the most important new finding from the continued audit because it leaves the slab API surface able to recreate the same bug family we just fixed.

### 9. Medium-High: public `NounMap` in `slab.rs` stores raw noun keys with no owner

Files:
- `crates/nockapp/src/noun/slab.rs:553`
- `crates/nockapp/src/noun/slab.rs:565`
- `crates/nockapp/src/noun/slab.rs:581`

What is wrong:
- `NounMap<V>` stores raw `Noun` keys in `Vec<(Noun, V)>`.
- `insert(...)` and `get(...)` require the caller to provide a `&NounSpace` later when comparing those retained keys.
- If a caller inserts keys from a temporary slab/stack and keeps the map longer than the owner, the retained keys become dangling or require exactly the kind of after-the-fact provenance reconstruction PMA is trying to eliminate.

Why this is the same bug family:
- Provenance is discarded at insertion time and reintroduced later by passing a `space` back in.
- That is the same structural mistake as the other raw-`Noun` container helpers.

Current reachability:
- I only found internal serialization/backref uses today, where the source noun stays live for the duration of `jam(...)`.
- I did not find an external production use that is already misbehaving.

Assessment:
- Latent public API bug.
- Lower priority than the slab mutators above, but still wrong on the PMA axis.

### 10. Medium-High: `nockchain-math::NounMathExt::uncell()` still downgrades traversal back to raw `[Noun; N]`

Files:
- `crates/nockchain-math/src/noun_ext.rs:17`
- `crates/nockchain-math/src/convert.rs:150`

What is wrong:
- The raw-`Noun` trait variant `NounMathExt::uncell(...)` takes a `space`, traverses with it, and then returns raw `[Noun; N]`.
- That immediately discards the provenance that was just supplied.
- The handle-shaped alternative already exists as `NounMathExtHandle::uncell(...)`, but a wide swath of code still uses the raw variant.

Concrete downstream uses I found:
- `crates/zkvm-jetpack/src/jets/proof_gen_jets.rs:220`
- `crates/zkvm-jetpack/src/jets/proof_gen_jets.rs:291`
- `crates/zkvm-jetpack/src/jets/mega_jets.rs:28`
- `crates/zkvm-jetpack/src/jets/mary_jets.rs:337`
- `crates/zkvm-jetpack/src/jets/tip5_jets.rs:289`
- `crates/zkvm-jetpack/src/jets/verifier_jets.rs:335`
- `crates/nockchain-math/src/zoon/zmap.rs:22`
- `crates/nockchain-math/src/zoon/zset.rs:17`

Why this is the same bug family:
- This is tuple-shaped provenance erasure.
- It is the same underlying mistake already called out for `HoonList`, `HoonMap`, `Vec<Noun>`, and `Option<Noun>` helpers.

Current reachability:
- The raw nouns returned by `uncell(...)` are currently consumed immediately in the same stack/space in the call sites I checked.
- I did not prove a present-day crash from these uses alone.

Assessment:
- Systemic latent hazard.
- Not as urgent as the slab mutators, but still part of the same remaining PMA blocker set.
