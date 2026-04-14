1. Critical: dangling &str -> Noun is still real in crates/nockapp/src/noun/extensions.rs:69. It allocates into a temporary NounSlab and returns a raw Noun after the slab is dropped. IntoSlab for &str in crates/nockapp/src/noun/extensions.rs:124 still depends on that broken constructor. I still don’t see non-test callers, so this is latent, but it is an actual use-after-free constructor.

Fixed now, banned `IntoNoun for &str` with a negative trait impl. Alternatives:

```rust
#![feature(auto_traits, negative_impls)]

auto trait IntoNounAllowed {}
impl !IntoNounAllowed for &str {}

#[diagnostic::on_unimplemented(
    message = "`{Self}` is not allowed with `IntoNoun`",
    note = "use `IntoSlab` or an allocator-taking API instead"
)]
pub trait IntoNoun: IntoNounAllowed {
    fn into_noun(self) -> Noun;
}
```

```rust
mod private {
    pub trait IntoNounAllowed {}
    impl IntoNounAllowed for u64 {}
    impl IntoNounAllowed for IndirectAtom {}
    // deliberately no impl for &str
}

#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be converted with `IntoNoun`",
    note = "`&str` must use `IntoSlab` or an allocator-taking conversion"
)]
pub trait IntoNoun: private::IntoNounAllowed {
    fn into_noun(self) -> Noun;
}
```

2. High: noun-serde still has raw-Noun identity encode/decode in crates/noun-serde/src/lib.rs:113. That is still the broadest unsafe provenance-erasure boundary in the tree. The decode side is live today through block-explorer raw fields; the encode side is more latent, but the API is still wrong.

Fixed. Banned NounEncode/NounDecode for Noun.

3. Medium-High: NounSlab can still build mixed trees even after the newer set_root() guard. modify() in crates/nockapp/src/noun/slab.rs:88 and From<[Noun; N]> in crates/nockapp/src/noun/slab.rs:251 feed raw Nouns into T(...), then set_root() only validates the top allocation in crates/nockapp/src/noun/slab.rs:425. So foreign children can still be embedded under a local root cell. I did not find a bad live caller, but this is still areal footgun.

Mostly fixed, modify helpers for NounSlab now re-home nouns, mitigates foreign noun pointers in slabs. It's still possible to feed raw foreign Nouns into `T()` and to set_root that.

4. Medium-High: jets::cold still treats raw Noun as a decoded payload in crates/nockvm/rust/nockvm/src/jets/cold.rs:932 and crates/nockvm/rust/nockvm/src/jets/cold.rs:1019. This is less urgent than the original audit implied because the live checkpoint restore path first copies saveable.cold into the active stack before Cold::from_noun(...) in crates/nockapp/src/kernel/form.rs:1646. So I do not see a current live mismatch there, but the codec API is still unsafe.

Hamt/Batteries Nounable re-home now. Might want to change Nounable encode to use NounSpace/NounHandle.

5. Medium: the block explorer still explicitly opts back into raw nouns in crates/nockapp-grpc/src/services/public_nockchain/v2/block_explorer.rs:1384 and crates/nockapp-grpc/src/services/public_nockchain/v2/block_explorer.rs:2041, plus the custom FullPageNoun decoder in crates/nockapp-grpc/src/services/public_nockchain/v2/block_explorer.rs:2056. This path is live, but the raw nouns are immediately consumed with the same space, so I’d rank it below the generic noun-serde escape hatch.

- PageAndTxs and FullPageData store raw Noun only so they can immediately call handle-based helpers later with
  the same space.
- FullPageNoun also stores raw Noun for version_or_digest and rest, then FullPageDetails::from_raw() immediately
  re-wraps them with space.
- Most of the real decoding logic already wants &NounHandle: extract_tx_ids_from_map,
  extract_transactions_from_map, decode_pow, decode_coinbase, decode_page_msg, etc.

The catch is the trait boundary:

- NounHandle<'a> borrows &'a NounSpace.
- These types are currently decoded via NounDecode, which is an owned-return API.
- So changing txs: Noun to txs: NounHandle<'a> means those structs need lifetimes, and then the current
  #[derive(NounDecode)] path for BlockRangeEntryNoun / FullPageEntryNoun stops fitting cleanly.

So my recommendation is:

- Don’t try to make these existing NounDecode structs hold NounHandles directly.
- Instead, replace the raw-noun intermediates with local borrowed parser helpers, e.g. PageAndTxsView<'a> /
  FullPageView<'a>, built from a root NounHandle<'a>.
- Then parse the entry in one pass from the handle, without ever storing raw Noun subfields.

That would remove the raw-noun escape hatch here and be cleaner than the current setup. It’s a reasonable
refactor. I’d call it moderate effort, low semantic risk, but it wants a small restructuring of the decode path
rather than a one-line type change.

6. Medium-Low: the zkvm-jetpack raw Vec<Noun> helpers are still structurally wrong for cross-space use in crates/zkvm-jetpack/src/utils.rs:18 and crates/zkvm-jetpack/src/utils.rs:115. After re-checking the main call sites, they currently look intra-stack jet code, so I’d treat them as hazardous API surface, not the next likely production crash.

7. Medium-Low: the nockchain-math side is now less severe than the audit said. HoonList/HoonMap still store raw Nouns in crates/nockchain-math/src/structs.rs:8, and raw NounMathExt::uncell(space) still exists in crates/nockchain-math/src/convert.rs:150. But HoonMapIter now yields NounHandle in crates/nockchain-math/src/structs.rs:148, and handle-based uncell() exists in crates/nockchain-math/src/convert.rs:196. A lot of downstream decoding has already moved onto the safer handle path.

8. Low: bridge::signing still reconstructs NounSpace manually and rejects offset-form nouns in crates/bridge/src/signing.rs:33 and crates/bridge/src/signing.rs:102. I only found test call sites, not a live runtime path. It is still wrong and PMA-incompatible if exercised.

9. Low: public NounMap in crates/nockapp/src/noun/slab.rs:553 still stores raw noun keys without carrying an owner, but I did not find external use outside the slab jammer internals. I would not prioritize it ahead of the items above.

What Changed Since The Audit

- `NounSlab::set_root()` is now materially better, so the slab findings should be read as “mixed-tree constructors still bypass the spirit of the guard,” not “foreign roots are trivially installable.”
- The math-side situation is better than the audit snapshot because handle-based traversal has already landed in important places.
- bridge::signing looks less urgent than the audit suggested because it appears test-only right now.

This was static re-triage only; I didn’t make changes or run runtime repros on these paths yet.
