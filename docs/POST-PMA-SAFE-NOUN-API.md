# Post-PMA Safe Noun API: Options and Tradeoffs

This captures the options for unifying arena-typed wrappers with `NounHandle<'a>` while keeping
lifetimes tied to provenance (specific `NounSlab`, `NockStack`, or `PMA`). It also calls out
where separate noun types remain useful and how to avoid overlapping API surfaces.

## Goals and constraints

- Make it hard (ideally impossible) to dereference or persist nouns from the wrong arena.
- Keep lifetimes tied to the specific arena instance that owns the noun.
- Avoid API surface overlap while preserving grepability (e.g., `SlabNoun`, `StackNoun`).
- Support mixed-ownership trees (stack + PMA) in a principled way.
- Minimize churn where possible.

## Option A (recommended): Generic `NounHandle<'a, K>` with type aliases

Make `NounHandle` generic over a marker type that encodes which arena(s) are valid:

```rust
pub struct NounHandle<'a, K> {
    noun: Noun,
    space: SpaceRef<'a, K>,
    _marker: PhantomData<K>,
}

pub struct SlabKind;
pub struct StackKind;
pub struct PmaKind;
pub struct StackOrPmaKind; // union for runtime tag discrimination

pub type SlabNoun<'a> = NounHandle<'a, SlabKind>;
pub type StackNoun<'a> = NounHandle<'a, StackKind>;
pub type PmaNoun<'a> = NounHandle<'a, PmaKind>;
pub type StackPmaNoun<'a> = NounHandle<'a, StackOrPmaKind>;
```

### Why it helps

- **Single handle type**: no duplicate API surface, just typed aliases.
- **Greppable**: `SlabNoun` / `StackNoun` / `PmaNoun` appear at call sites.
- **Static safety**: APIs can accept `NounHandle<'a, StackKind>` when they must not
  touch PMA pointers, or `StackOrPmaKind` when tag-based dispatch is allowed.
- **No cross-arena misuse**: `SpaceRef<'a, K>` only constructed from the correct arena.

### Implementation notes

- `SpaceRef<'a, K>` should borrow the actual arena instance (e.g., `&'a NounSlab`,
  `&'a NockStack`, `&'a Pma`), not just address ranges. That ties the lifetime and
  prevents use-after-free when the arena resets.
- Provide explicit constructors on each arena:
  - `NounSlab::handle(noun) -> SlabNoun`
  - `NockStack::handle(noun) -> StackNoun`
  - `Pma::handle(noun) -> PmaNoun`
  - `NounSpace::handle(noun) -> StackPmaNoun` (only when tag-based logic is valid)
- For conversions, require explicit copying:
  - `StackNoun::copy_into_pma(&self, &Pma) -> PmaNoun`
  - `SlabNoun::copy_into_stack(&self, &NockStack) -> StackNoun`

### When it is insufficient

If you only have a raw `Noun` + runtime `NounSpace` and cannot statically know the
arena, you must use `StackOrPmaKind`. This is still safe because tag-based lookup
is centralized and validated.

## Option B: Generic `NounSpace<K>` with typed handles

Make `NounSpace` generic and return handles bound to it:

```rust
pub struct NounSpace<'a, K> { /* arena refs */ }
pub struct NounHandle<'a, K> { noun: Noun, space: &'a NounSpace<'a, K> }
```

### Pros

- Stronger link between space and handle (single source of truth).
- Easy to restrict constructors to space capabilities.

### Cons

- More churn: `NounSpace` is currently a concrete type used broadly.
- Harder to keep `NounSpace` ergonomic in code paths that already manage multiple
  arenas (slab + stack + PMA).

## Option C: Newtype wrappers around a non-generic `NounHandle<'a>`

Keep a single handle but wrap for safety:

```rust
pub struct SlabNoun<'a>(NounHandle<'a>);
pub struct StackNoun<'a>(NounHandle<'a>);
pub struct PmaNoun<'a>(NounHandle<'a>);
```

### Pros

- Minimal disruption to existing handle code.
- Clear grepable types at call sites.

### Cons

- Boilerplate and duplicated impls (traits, methods, conversions).
- Easier to accidentally re-expose unsafe constructors.

## Option D: Fully generic `Noun<A>` (arena-typed noun)

Make nouns themselves generic over arena instance type:

```rust
pub struct Noun<A> { /* pointer + PhantomData<A> */ }
```

### Pros

- Strongest static safety.
- Prevents mixing nouns across arenas without explicit conversion.

### Cons

- Very high churn: `Noun` appears everywhere.
- Hard to represent mixed trees (stack + PMA) without a union arena type.
- Can lead to type explosion or trait indirection.

## Option E: Hybrid: generic handle + typed wrappers (best of both)

Use Option A internally, but provide thin wrappers for clarity:

```rust
pub type SlabNoun<'a> = NounHandle<'a, SlabKind>;
pub type StackNoun<'a> = NounHandle<'a, StackKind>;
pub type PmaNoun<'a> = NounHandle<'a, PmaKind>;
```

Then implement APIs in terms of `NounHandle<'a, K>` + trait bounds where needed
(`impl NounAccess for NounHandle<'a, K>`). This keeps the surface area unified while
still letting call sites declare intent.

## Key correctness points regardless of option

- **Arena provenance** must be tied to a specific instance. Passing a `NounSpace` that
  only stores ranges is insufficient; it must borrow the arena itself so lifetimes
  enforce validity.
- **Conversions should be explicit**. Copying between arenas should require a method
  with the target arena, not an implicit cast.
- **Mixed trees require a union type**. Structural sharing across stack + PMA means
  some APIs must accept `StackOrPma` handles (tag-dispatched).
- **No TLS**. All provenance is explicit through references passed to constructors.

## Why Option A is the cleanest unification

- One handle type = one API surface.
- Type aliases give you the grepability of distinct nouns without duplicating code.
- Marker types let the compiler enforce correct arena usage and disallow invalid
  conversions without explicitly copying.

If you want the least churn while gaining strong safety, Option A + E is the most
practical path.

# Precipitating error

```
I (01:10:31) handle-command: timer
I (01:10:31) [no] kernel::form: stack-usage: used_words=5190 used_mib=0.040 least_space_words=2147478458 total_words=2147483648 event_num=145911
I (01:10:31) [no] kernel::form: pma-timing: event_ms=8.835 pma_copy_ms=0.022 total_ms=8.857 alloc_words=29 alloc_mib=0.000 event_num=145911
I (01:10:31) peek: %heavy-n

thread 'tokio-runtime-worker' panicked at crates/nockvm/rust/nockvm/src/noun.rs:256:9:
pointer-form noun 0x7d7054132750 is not within stack or PMA arenas
stack backtrace:
   0: rust_begin_unwind
             at ./rustc/a567209daab72b7ea59eac533278064396bb0534/library/std/src/panicking.rs:695:5
   1: core::panicking::panic_fmt
             at ./rustc/a567209daab72b7ea59eac533278064396bb0534/library/core/src/panicking.rs:75:14
   2: nockvm::noun::NounSpace::classify_ptr
             at ./home/callen/work/zorp/pma-nockchain/crates/nockvm/rust/nockvm/src/noun.rs:256:9
   3: nockvm::noun::NounSpace::resolve_stack_ptr
             at ./home/callen/work/zorp/pma-nockchain/crates/nockvm/rust/nockvm/src/noun.rs:228:9
   4: nockvm::noun::TaggedPtr::resolve_const
             at ./home/callen/work/zorp/pma-nockchain/crates/nockvm/rust/nockvm/src/noun.rs:142:35
   5: nockvm::noun::Allocated::to_raw_pointer
   6: nockvm::noun::Allocated::get_metadata
             at ./home/callen/work/zorp/pma-nockchain/crates/nockvm/rust/nockvm/src/noun.rs:2066:10
   7: nockvm::noun::Allocated::get_cached_mug
             at ./home/callen/work/zorp/pma-nockchain/crates/nockvm/rust/nockvm/src/noun.rs:2104:17
   8: nockvm::mug::get_mug
             at ./home/callen/work/zorp/pma-nockchain/crates/nockvm/rust/nockvm/src/mug.rs:74:29
   9: nockapp::noun::slab::slab_mug
             at ./home/callen/work/zorp/pma-nockchain/crates/nockapp/src/noun/slab.rs:625:25
  10: nockapp::noun::slab::NounMap<V>::get
             at ./home/callen/work/zorp/pma-nockchain/crates/nockapp/src/noun/slab.rs:587:23
  11: <nockapp::noun::slab::NockJammer as nockapp::noun::slab::Jammer>::jam
             at ./home/callen/work/zorp/pma-nockchain/crates/nockapp/src/noun/slab.rs:701:36
  12: nockapp::noun::slab::NounSlab<J>::jam
             at ./home/callen/work/zorp/pma-nockchain/crates/nockapp/src/noun/slab.rs:478:9
  13: nockchain_libp2p_io::driver::create_scry_response
             at ./home/callen/work/zorp/pma-nockchain/crates/nockchain-libp2p-io/src/driver.rs:1622:65
  14: nockchain_libp2p_io::driver::handle_request_response::{{closure}}
             at ./home/callen/work/zorp/pma-nockchain/crates/nockchain-libp2p-io/src/driver.rs:1047:33
  15: nockchain_libp2p_io::driver::make_libp2p_driver::{{closure}}::{{closure}}::{{closure}}
             at ./home/callen/work/zorp/pma-nockchain/crates/nockchain-libp2p-io/src/driver.rs:317:215
  16: tokio::runtime::task::core::Core<T,S>::poll::{{closure}}
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/core.rs:365:17
  17: tokio::loom::std::unsafe_cell::UnsafeCell<T>::with_mut
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/loom/std/unsafe_cell.rs:16:9
  18: tokio::runtime::task::core::Core<T,S>::poll
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/core.rs:354:30
  19: tokio::runtime::task::harness::poll_future::{{closure}}
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/harness.rs:535:19
  20: <core::panic::unwind_safe::AssertUnwindSafe<F> as core::ops::function::FnOnce<()>>::call_once
             at ./home/callen/.rustup/toolchains/nightly-2025-02-14-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/panic/unwind_safe.rs:272:9
  21: std::panicking::try::do_call
             at ./home/callen/.rustup/toolchains/nightly-2025-02-14-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/std/src/panicking.rs:587:40
  22: std::panicking::try
             at ./home/callen/.rustup/toolchains/nightly-2025-02-14-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/std/src/panicking.rs:550:19
  23: std::panic::catch_unwind
             at ./home/callen/.rustup/toolchains/nightly-2025-02-14-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/std/src/panic.rs:358:14
  24: tokio::runtime::task::harness::poll_future
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/harness.rs:523:18
  25: tokio::runtime::task::harness::Harness<T,S>::poll_inner
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/harness.rs:210:27
  26: tokio::runtime::task::harness::Harness<T,S>::poll
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/harness.rs:155:15
  27: tokio::runtime::task::raw::poll
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/raw.rs:325:5
  28: tokio::runtime::task::raw::RawTask::poll
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/raw.rs:255:18
  29: tokio::runtime::task::LocalNotified<S>::run
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/mod.rs:509:9
  30: tokio::runtime::scheduler::multi_thread::worker::Context::run_task::{{closure}}
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/scheduler/multi_thread/worker.rs:677:22
  31: tokio::task::coop::with_budget
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/task/coop/mod.rs:167:5
  32: tokio::task::coop::budget
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/task/coop/mod.rs:133:5
  33: tokio::runtime::scheduler::multi_thread::worker::Context::run_task
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/scheduler/multi_thread/worker.rs:591:9
  34: tokio::runtime::scheduler::multi_thread::worker::Context::run
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/scheduler/multi_thread/worker.rs:539:24
  35: tokio::runtime::scheduler::multi_thread::worker::run::{{closure}}::{{closure}}
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/scheduler/multi_thread/worker.rs:504:21
  36: tokio::runtime::context::scoped::Scoped<T>::set
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/context/scoped.rs:40:9
  37: tokio::runtime::context::set_scheduler::{{closure}}
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/context.rs:176:26
  38: std::thread::local::LocalKey<T>::try_with
             at ./home/callen/.rustup/toolchains/nightly-2025-02-14-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/std/src/thread/local.rs:310:12
  39: std::thread::local::LocalKey<T>::with
             at ./home/callen/.rustup/toolchains/nightly-2025-02-14-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/std/src/thread/local.rs:274:15
  40: tokio::runtime::context::set_scheduler
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/context.rs:176:9
  41: tokio::runtime::scheduler::multi_thread::worker::run::{{closure}}
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/scheduler/multi_thread/worker.rs:499:9
  42: tokio::runtime::context::runtime::enter_runtime
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/context/runtime.rs:65:16
  43: tokio::runtime::scheduler::multi_thread::worker::run
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/scheduler/multi_thread/worker.rs:491:5
  44: tokio::runtime::scheduler::multi_thread::worker::Launch::launch::{{closure}}
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/scheduler/multi_thread/worker.rs:457:45
  45: <tokio::runtime::blocking::task::BlockingTask<T> as core::future::future::Future>::poll
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/blocking/task.rs:42:21
  46: tokio::runtime::task::core::Core<T,S>::poll::{{closure}}
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/core.rs:365:17
  47: tokio::loom::std::unsafe_cell::UnsafeCell<T>::with_mut
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/loom/std/unsafe_cell.rs:16:9
  48: tokio::runtime::task::core::Core<T,S>::poll
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/core.rs:354:30
  49: tokio::runtime::task::harness::poll_future::{{closure}}
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/harness.rs:535:19
  50: <core::panic::unwind_safe::AssertUnwindSafe<F> as core::ops::function::FnOnce<()>>::call_once
             at ./home/callen/.rustup/toolchains/nightly-2025-02-14-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/panic/unwind_safe.rs:272:9
  51: std::panicking::try::do_call
             at ./home/callen/.rustup/toolchains/nightly-2025-02-14-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/std/src/panicking.rs:587:40
  52: std::panicking::try
             at ./home/callen/.rustup/toolchains/nightly-2025-02-14-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/std/src/panicking.rs:550:19
  53: std::panic::catch_unwind
             at ./home/callen/.rustup/toolchains/nightly-2025-02-14-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/std/src/panic.rs:358:14
  54: tokio::runtime::task::harness::poll_future
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/harness.rs:523:18
  55: tokio::runtime::task::harness::Harness<T,S>::poll_inner
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/harness.rs:210:27
  56: tokio::runtime::task::harness::Harness<T,S>::poll
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/harness.rs:155:15
  57: tokio::runtime::task::raw::poll
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/raw.rs:325:5
  58: tokio::runtime::task::raw::RawTask::poll
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/raw.rs:255:18
  59: tokio::runtime::task::UnownedTask<S>::run
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/task/mod.rs:546:9
  60: tokio::runtime::blocking::pool::Task::run
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/blocking/pool.rs:161:9
  61: tokio::runtime::blocking::pool::Inner::run
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/blocking/pool.rs:516:17
  62: tokio::runtime::blocking::pool::Spawner::spawn_thread::{{closure}}
             at ./home/callen/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.1/src/runtime/blocking/pool.rs:474:13
note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.
E (01:10:31) driver: Task error: JoinError::Panic(Id(121853), "pointer-form noun 0x7d7054132750 is not within stack or PMA arenas", ...)
```
