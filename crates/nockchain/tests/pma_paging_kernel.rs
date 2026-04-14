use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use ibig::UBig;
use kernels_open_dumb::KERNEL as NOCKCHAIN_KERNEL;
use libp2p::PeerId;
use memmap2::MmapOptions;
use nockapp::kernel::boot::{self, NockStackSize, TraceOpts};
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockapp::wire::{SystemWire, Wire, WireRepr};
use nockapp::{AtomExt, Bytes, NockApp, NockAppError};
use nockchain::mining::MiningWire;
use nockchain::setup::{self, BlockchainConstants, Seconds, DEFAULT_GENESIS_BLOCK_HEIGHT};
use nockchain_libp2p_io::driver::Libp2pWire;
use nockchain_libp2p_io::tip5_util::tip5_hash_to_base58_stack;
use nockchain_math::belt::{Belt, PRIME};
use nockchain_math::crypto::cheetah::{
    ch_add, ch_neg, ch_scal_big, trunc_g_order, A_GEN, A_ID, G_ORDER,
};
use nockchain_math::noun_ext::NounMathExt;
use nockchain_math::tip5::hash::hash_varlen;
use nockchain_math::zoon::common::DefaultTipHasher;
use nockchain_math::zoon::zset::z_set_put;
use nockchain_types::tx_engine::common::{
    BlockHeight, BlockHeightDelta, Hash, Name, Nicks, SchnorrPubkey, SchnorrSignature, Signature,
    Source, TimelockRangeAbsolute, TimelockRangeRelative, Version,
};
use nockchain_types::tx_engine::v0::{
    Input, Inputs, Lock, NoteHead, NoteTail, NoteV0, RawTx, Seed, Seeds, Spend, Timelock,
    TimelockIntent,
};
use nockvm::ext::noun_equality;
use nockvm::mem::NockStack;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace, D, NO, SIG, T, YES};
use nockvm_macros::tas;
use noun_serde::{NounDecode, NounEncode};
use rand::rngs::StdRng;
use rand::{Rng, RngCore, SeedableRng};
use tempfile::TempDir;
use tracing::{info, warn};
use zkvm_jetpack::hot::produce_prover_hot_state;
use zkvm_jetpack::jets::tip5_jets::hash_hashable;

const DEFAULT_TARGET_BLOCKS: usize = 100_000;
const DEFAULT_TARGET_BYTES: u64 = 10 * 1024 * 1024 * 1024;
const DEFAULT_OUTPUTS_PER_TX: usize = 256;
const DEFAULT_PUBKEYS_PER_OUTPUT: usize = 4;
const DEFAULT_EXTRA_GIFT: u64 = 1;
const DEFAULT_BYTES_CHECK_INTERVAL: usize = 1000;
const DEFAULT_TXS_PER_BLOCK: usize = 4;
const DEFAULT_NOTE_POOL: usize = 4;

const HASH_STACK_WORDS: usize = 1 << 22;

const PMA_MAGIC: u64 = u64::from_le_bytes(*b"NOCKPMA1");
const PMA_VERSION: u64 = 1;
const PMA_TRAILER_BYTES: usize = 32;

#[repr(C)]
#[derive(Clone, Copy)]
struct PmaTrailer {
    magic: u64,
    version: u64,
    data_words: u64,
    alloc_offset: u64,
}

impl PmaTrailer {
    fn from_bytes(buf: [u8; PMA_TRAILER_BYTES]) -> Self {
        let magic = u64::from_le_bytes(buf[0..8].try_into().expect("magic slice"));
        let version = u64::from_le_bytes(buf[8..16].try_into().expect("version slice"));
        let data_words = u64::from_le_bytes(buf[16..24].try_into().expect("data_words slice"));
        let alloc_offset = u64::from_le_bytes(buf[24..32].try_into().expect("alloc_offset slice"));
        Self {
            magic,
            version,
            data_words,
            alloc_offset,
        }
    }
}

#[derive(Clone)]
struct MiningCandidate {
    version: NounSlab,
    header: NounSlab,
    _target: NounSlab,
    _pow_len: u64,
}

struct Poke {
    wire: WireRepr,
    noun: NounSlab,
    tx_id: Option<Hash>,
}

struct KeyMaterial {
    sk: UBig,
    pk: SchnorrPubkey,
    pk_b58: String,
}

struct TxPlan {
    raw_tx: RawTx,
    refund_note: NoteV0,
    refund_name: Name,
}

struct PendingTx {
    id: Hash,
    refund_note: NoteV0,
    refund_name: Name,
}

#[derive(Default)]
struct TxEffectStats {
    sent: u64,
    accepted: u64,
    seen: u64,
    dropped: u64,
    liar_causes: HashMap<String, u64>,
}

impl TxEffectStats {
    fn record_liar(&mut self, cause: String) {
        *self.liar_causes.entry(cause).or_insert(0) += 1;
    }
}

struct TxHasher {
    stack: NockStack,
    lock_cache: Option<LockCache>,
}

struct LockCache {
    lock: Lock,
    hashable_sig: Noun,
    sig_hash: Hash,
}

impl TxHasher {
    fn new() -> Self {
        Self {
            stack: NockStack::new(HASH_STACK_WORDS, 0),
            lock_cache: None,
        }
    }

    fn reset(&mut self) {
        unsafe { self.stack.reset(0) };
        self.lock_cache = None;
    }

    fn cache_lock(&mut self, lock: &Lock) {
        let hashable_sig = self.hashable_sig_uncached(lock);
        let sig_hash = self.hash_hashable(hashable_sig, &self.stack.noun_space());
        self.lock_cache = Some(LockCache {
            lock: lock.clone(),
            hashable_sig,
            sig_hash,
        });
    }

    fn hash_hashable(&mut self, hashable: Noun, space: &NounSpace) -> Hash {
        let digest = hash_hashable(&mut self.stack, hashable, space).expect("hash_hashable failed");
        Hash::from_noun(&digest, &self.stack.noun_space()).expect("decode hash")
    }

    fn hashable_leaf(&mut self, noun: Noun) -> Noun {
        T(&mut self.stack, &[D(tas!(b"leaf")), noun])
    }

    fn hashable_hash(&mut self, hash: &Hash) -> Noun {
        let hash_noun = hash.to_noun(&mut self.stack);
        T(&mut self.stack, &[D(tas!(b"hash")), hash_noun])
    }

    fn hashable_unit_leaf<T: NounEncode>(&mut self, value: Option<&T>) -> Noun {
        match value {
            None => self.hashable_leaf(SIG),
            Some(value) => {
                let none_leaf = self.hashable_leaf(SIG);
                let value_noun = value.to_noun(&mut self.stack);
                let value_leaf = self.hashable_leaf(value_noun);
                T(&mut self.stack, &[none_leaf, value_leaf])
            }
        }
    }

    fn hashable_timelock_range_abs(&mut self, range: &TimelockRangeAbsolute) -> Noun {
        let min = self.hashable_unit_leaf(range.min.as_ref());
        let max = self.hashable_unit_leaf(range.max.as_ref());
        T(&mut self.stack, &[min, max])
    }

    fn hashable_timelock_range_rel(&mut self, range: &TimelockRangeRelative) -> Noun {
        let min = self.hashable_unit_leaf(range.min.as_ref());
        let max = self.hashable_unit_leaf(range.max.as_ref());
        T(&mut self.stack, &[min, max])
    }

    fn hashable_timelock_intent(&mut self, intent: &Option<TimelockIntent>) -> Noun {
        match intent {
            None => self.hashable_leaf(SIG),
            Some(intent) => {
                let marker = self.hashable_leaf(SIG);
                let abs = self.hashable_timelock_range_abs(&intent.absolute);
                let rel = self.hashable_timelock_range_rel(&intent.relative);
                T(&mut self.stack, &[marker, abs, rel])
            }
        }
    }

    fn hashable_timelock(&mut self, timelock: &Timelock) -> Noun {
        self.hashable_timelock_intent(&timelock.0)
    }

    fn hash_timelock(&mut self, timelock: &Timelock) -> Hash {
        let hashable = self.hashable_timelock(timelock);
        self.hash_hashable(hashable, &self.stack.noun_space())
    }

    fn hashable_source(&mut self, source: &Source) -> Noun {
        let hash = self.hashable_hash(&source.hash);
        let coinbase = self.hashable_leaf(if source.is_coinbase { YES } else { NO });
        T(&mut self.stack, &[hash, coinbase])
    }

    fn hash_source(&mut self, source: &Source) -> Hash {
        let hashable = self.hashable_source(source);
        self.hash_hashable(hashable, &self.stack.noun_space())
    }

    fn hashable_unit_source(&mut self, source: &Option<Source>) -> Noun {
        match source {
            None => self.hashable_leaf(SIG),
            Some(source) => {
                let none_leaf = self.hashable_leaf(SIG);
                let hashable = self.hashable_source(source);
                T(&mut self.stack, &[none_leaf, hashable])
            }
        }
    }

    fn hash_schnorr_pubkey(&mut self, pubkey: &SchnorrPubkey) -> Hash {
        let pk_noun = pubkey.to_noun(&mut self.stack);
        let hashable = self.hashable_leaf(pk_noun);
        self.hash_hashable(hashable, &self.stack.noun_space())
    }

    fn hashable_pubkeys(&mut self, node: Noun, space: &NounSpace) -> Noun {
        if unsafe { node.raw_equals(&D(0)) } {
            return self.hashable_leaf(node);
        }
        let Ok([entry, left, right]) = node.uncell(space) else {
            panic!("pubkeys node not a cell");
        };
        let pubkey = SchnorrPubkey::from_noun(&entry, space).expect("decode schnorr pubkey");
        let pk_hash = self.hash_schnorr_pubkey(&pubkey);
        let entry_hashable = self.hashable_hash(&pk_hash);
        let left_hashable = self.hashable_pubkeys(left, space);
        let right_hashable = self.hashable_pubkeys(right, space);
        T(
            &mut self.stack,
            &[entry_hashable, left_hashable, right_hashable],
        )
    }

    fn hashable_sig(&mut self, lock: &Lock) -> Noun {
        if let Some(cache) = &self.lock_cache {
            if &cache.lock == lock {
                return cache.hashable_sig;
            }
        }
        self.hashable_sig_uncached(lock)
    }

    fn hashable_sig_uncached(&mut self, lock: &Lock) -> Noun {
        let mut slab: NounSlab = NounSlab::new();
        let mut pubkeys_set = D(0);
        for pubkey in &lock.pubkeys {
            let mut key_noun = pubkey.to_noun(&mut slab);
            pubkeys_set = z_set_put(&mut slab, &pubkeys_set, &mut key_noun, &DefaultTipHasher)
                .expect("z_set_put for pubkeys");
        }
        let space = slab.noun_space();
        let pubkeys_hashable = self.hashable_pubkeys(pubkeys_set, &space);
        let m = self.hashable_leaf(D(lock.keys_required));
        T(&mut self.stack, &[m, pubkeys_hashable])
    }

    fn hash_sig(&mut self, lock: &Lock) -> Hash {
        if let Some(cache) = &self.lock_cache {
            if &cache.lock == lock {
                return cache.sig_hash.clone();
            }
        }
        let hashable = self.hashable_sig_uncached(lock);
        self.hash_hashable(hashable, &self.stack.noun_space())
    }

    fn hashable_nname(&mut self, name: &Name) -> Noun {
        let first = self.hashable_hash(&name.first);
        let last = self.hashable_hash(&name.last);
        let null_leaf = self.hashable_leaf(SIG);
        T(&mut self.stack, &[first, last, null_leaf])
    }

    fn hash_nname(&mut self, name: &Name) -> Hash {
        let hashable = self.hashable_nname(name);
        self.hash_hashable(hashable, &self.stack.noun_space())
    }

    fn hashable_seed(&mut self, seed: &Seed) -> Noun {
        let recipient = self.hashable_sig(&seed.recipient);
        let timelock_intent = self.hashable_timelock_intent(&seed.timelock_intent);
        let gift = self.hashable_leaf(D(seed.gift.0 as u64));
        let parent_hash = self.hashable_hash(&seed.parent_hash);
        T(
            &mut self.stack,
            &[recipient, timelock_intent, gift, parent_hash],
        )
    }

    fn sig_hashable_seed(&mut self, seed: &Seed) -> Noun {
        let output_source = self.hashable_unit_source(&seed.output_source);
        let recipient = self.hashable_sig(&seed.recipient);
        let timelock_intent = self.hashable_timelock_intent(&seed.timelock_intent);
        let gift = self.hashable_leaf(D(seed.gift.0 as u64));
        let parent_hash = self.hashable_hash(&seed.parent_hash);
        T(
            &mut self.stack,
            &[output_source, recipient, timelock_intent, gift, parent_hash],
        )
    }

    fn hashable_seeds_tree(&mut self, node: Noun, space: &NounSpace) -> Noun {
        if unsafe { node.raw_equals(&D(0)) } {
            return self.hashable_leaf(node);
        }
        let Ok([seed_noun, left, right]) = node.uncell(space) else {
            panic!("seed node not a cell");
        };
        let seed = Seed::from_noun(&seed_noun, space).expect("decode seed");
        let seed_hashable = self.hashable_seed(&seed);
        let left_hashable = self.hashable_seeds_tree(left, space);
        let right_hashable = self.hashable_seeds_tree(right, space);
        T(
            &mut self.stack,
            &[seed_hashable, left_hashable, right_hashable],
        )
    }

    fn sig_hashable_seeds_tree(&mut self, node: Noun, space: &NounSpace) -> Noun {
        if unsafe { node.raw_equals(&D(0)) } {
            return self.hashable_leaf(node);
        }
        let Ok([seed_noun, left, right]) = node.uncell(space) else {
            panic!("seed node not a cell");
        };
        let seed = Seed::from_noun(&seed_noun, space).expect("decode seed");
        let seed_hashable = self.sig_hashable_seed(&seed);
        let left_hashable = self.sig_hashable_seeds_tree(left, space);
        let right_hashable = self.sig_hashable_seeds_tree(right, space);
        T(
            &mut self.stack,
            &[seed_hashable, left_hashable, right_hashable],
        )
    }

    fn hashable_signature_map(&mut self, signature: &Signature) -> Noun {
        let signature_noun = signature.to_noun(&mut self.stack);
        self.hashable_signature_tree(signature_noun, &self.stack.noun_space())
    }

    fn hashable_signature_tree(&mut self, node: Noun, space: &NounSpace) -> Noun {
        if unsafe { node.raw_equals(&D(0)) } {
            return self.hashable_leaf(node);
        }
        let Ok([entry, left, right]) = node.uncell(space) else {
            panic!("signature node not a cell");
        };
        let Ok([key_noun, sig_noun]) = entry.uncell(space) else {
            panic!("signature entry not a pair");
        };
        let pubkey = SchnorrPubkey::from_noun(&key_noun, space).expect("decode pubkey");
        let sig = SchnorrSignature::from_noun(&sig_noun, space).expect("decode signature");
        let pk_hash = self.hash_schnorr_pubkey(&pubkey);
        let pk_hashable = self.hashable_hash(&pk_hash);
        let sig_noun = sig.to_noun(&mut self.stack);
        let sig_hashable = self.hashable_leaf(sig_noun);
        let entry_hashable = T(&mut self.stack, &[pk_hashable, sig_hashable]);
        let left_hashable = self.hashable_signature_tree(left, space);
        let right_hashable = self.hashable_signature_tree(right, space);
        T(
            &mut self.stack,
            &[entry_hashable, left_hashable, right_hashable],
        )
    }

    fn hashable_spend(&mut self, spend: &Spend) -> Noun {
        let signature_hashable = match &spend.signature {
            None => self.hashable_leaf(SIG),
            Some(signature) => {
                let marker = self.hashable_leaf(SIG);
                let sig_hashable = self.hashable_signature_map(signature);
                T(&mut self.stack, &[marker, sig_hashable])
            }
        };
        let mut slab: NounSlab = NounSlab::new();
        let seeds_noun = spend.seeds.to_noun(&mut slab);
        slab.set_root(seeds_noun);
        let seeds_hashable = self.hashable_seeds_tree(seeds_noun, &slab.noun_space());
        let fee = self.hashable_leaf(D(spend.fee.0 as u64));
        T(&mut self.stack, &[signature_hashable, seeds_hashable, fee])
    }

    fn hashable_nnote(&mut self, note: &NoteV0) -> Noun {
        let timelock_hash = self.hash_timelock(&note.head.timelock);
        let name_hash = self.hash_nname(&note.tail.name);
        let sig_hash = self.hash_sig(&note.tail.lock);
        let source_hash = self.hash_source(&note.tail.source);
        let version_noun = note.head.version.to_noun(&mut self.stack);
        let version_leaf = self.hashable_leaf(version_noun);
        let origin_noun = note.head.origin_page.to_noun(&mut self.stack);
        let origin_leaf = self.hashable_leaf(origin_noun);
        let timelock_hashable = self.hashable_hash(&timelock_hash);
        let head = T(
            &mut self.stack,
            &[version_leaf, origin_leaf, timelock_hashable],
        );
        let name_hashable = self.hashable_hash(&name_hash);
        let sig_hashable = self.hashable_hash(&sig_hash);
        let source_hashable = self.hashable_hash(&source_hash);
        let assets_leaf = self.hashable_leaf(D(note.tail.assets.0 as u64));
        let tail = T(
            &mut self.stack,
            &[name_hashable, sig_hashable, source_hashable, assets_leaf],
        );
        T(&mut self.stack, &[head, tail])
    }

    fn hash_nnote(&mut self, note: &NoteV0) -> Hash {
        let hashable = self.hashable_nnote(note);
        self.hash_hashable(hashable, &self.stack.noun_space())
    }

    fn hashable_input(&mut self, input: &Input) -> Noun {
        let note_hashable = self.hashable_nnote(&input.note);
        let spend_hashable = self.hashable_spend(&input.spend);
        T(&mut self.stack, &[note_hashable, spend_hashable])
    }

    fn hashable_inputs(&mut self, inputs: &Inputs) -> Noun {
        let mut slab: NounSlab = NounSlab::new();
        let inputs_noun = inputs.to_noun(&mut slab);
        slab.set_root(inputs_noun);
        self.hashable_inputs_tree(inputs_noun, &slab.noun_space())
    }

    fn hashable_inputs_tree(&mut self, node: Noun, space: &NounSpace) -> Noun {
        if unsafe { node.raw_equals(&D(0)) } {
            return self.hashable_leaf(node);
        }
        let Ok([entry, left, right]) = node.uncell(space) else {
            panic!("inputs node not a cell");
        };
        let Ok([key_noun, value_noun]) = entry.uncell(space) else {
            panic!("inputs entry not a pair");
        };
        let name = Name::from_noun(&key_noun, space).expect("decode name");
        let input = Input::from_noun(&value_noun, space).expect("decode input");
        let name_hashable = self.hashable_nname(&name);
        let input_hashable = self.hashable_input(&input);
        let entry_hashable = T(&mut self.stack, &[name_hashable, input_hashable]);
        let left_hashable = self.hashable_inputs_tree(left, space);
        let right_hashable = self.hashable_inputs_tree(right, space);
        T(
            &mut self.stack,
            &[entry_hashable, left_hashable, right_hashable],
        )
    }

    fn hash_raw_tx_id(
        &mut self,
        inputs: &Inputs,
        timelock_range: &TimelockRangeAbsolute,
        total_fees: &Nicks,
    ) -> Hash {
        let inputs_hashable = self.hashable_inputs(inputs);
        let timelock_hashable = self.hashable_timelock_range_abs(timelock_range);
        let fees = self.hashable_leaf(D(total_fees.0 as u64));
        let hashable = T(&mut self.stack, &[inputs_hashable, timelock_hashable, fees]);
        self.hash_hashable(hashable, &self.stack.noun_space())
    }

    fn hash_seeds(&mut self, seeds: &Seeds) -> Hash {
        let mut slab: NounSlab = NounSlab::new();
        let seeds_noun = seeds.to_noun(&mut slab);
        slab.set_root(seeds_noun);
        let hashable = self.hashable_seeds_tree(seeds_noun, &slab.noun_space());
        self.hash_hashable(hashable, &self.stack.noun_space())
    }

    fn sig_hash(&mut self, seeds: &Seeds, fee: &Nicks) -> Hash {
        let mut slab: NounSlab = NounSlab::new();
        let seeds_noun = seeds.to_noun(&mut slab);
        slab.set_root(seeds_noun);
        let seeds_hashable = self.sig_hashable_seeds_tree(seeds_noun, &slab.noun_space());
        let fee_leaf = self.hashable_leaf(D(fee.0 as u64));
        let hashable = T(&mut self.stack, &[seeds_hashable, fee_leaf]);
        self.hash_hashable(hashable, &self.stack.noun_space())
    }
}

#[test]
#[ignore = "long-running; set NOCKCHAIN_PMA_PAGING_* to shrink the workload"]
#[cfg_attr(miri, ignore = "mincore/madvise unsupported in Miri")]
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), ignore)]
fn pma_paging_kernel_workload() {
    let _ = std::env::set_var("NOCKAPP_DISABLE_METRICS", "1");
    let _ = std::env::set_var("GNORT_DISABLE", "1");

    let target_blocks = env_usize("NOCKCHAIN_PMA_PAGING_BLOCKS", DEFAULT_TARGET_BLOCKS);
    let target_bytes = env_u64("NOCKCHAIN_PMA_PAGING_BYTES", DEFAULT_TARGET_BYTES);
    let outputs_per_tx = env_usize("NOCKCHAIN_PMA_PAGING_OUTPUTS", DEFAULT_OUTPUTS_PER_TX);
    let pubkeys_per_output = env_usize("NOCKCHAIN_PMA_PAGING_PUBKEYS", DEFAULT_PUBKEYS_PER_OUTPUT);
    let extra_gift = env_u64("NOCKCHAIN_PMA_PAGING_GIFT", DEFAULT_EXTRA_GIFT);
    let bytes_check_interval = env_usize(
        "NOCKCHAIN_PMA_PAGING_BYTES_INTERVAL", DEFAULT_BYTES_CHECK_INTERVAL,
    )
    .max(1);
    let txs_per_block =
        env_usize("NOCKCHAIN_PMA_PAGING_TXS_PER_BLOCK", DEFAULT_TXS_PER_BLOCK).max(1);
    let note_pool_target =
        env_usize("NOCKCHAIN_PMA_PAGING_NOTE_POOL", DEFAULT_NOTE_POOL).max(txs_per_block);
    let skip_mining = env_bool("NOCKCHAIN_PMA_PAGING_SKIP_MINING", true);
    let skip_txs = env_bool("NOCKCHAIN_PMA_PAGING_SKIP_TXS", false);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    runtime
        .block_on(async {
            let (temp_dir, mut app, data_dir) = build_nockapp("pma-paging").await?;
            let pma_path = find_pma_path(&data_dir)?;

            let mut rng = StdRng::seed_from_u64(1);
            let key = generate_key(&mut rng)?;
            let mut tx_hasher = TxHasher::new();
            let mining_pkh = tx_hasher.hash_schnorr_pubkey(&key.pk).to_base58();
            let lock_pool = vec![owned_lock(pubkeys_per_output, &key.pk, &mut rng)?];

            let genesis_bytes = setup::FAKENET_GENESIS_BLOCK.to_vec();
            let genesis_id = genesis_block_id(&genesis_bytes)?;
            let gossip_wire = Libp2pWire::Gossip(PeerId::random()).to_wire();

            let mut constants = fakenet_constants();
            if skip_mining {
                constants.check_pow_flag = false;
            }
            constants.max_block_size = u64::MAX;
            constants.base_fee = 0;

            let mut init_pokes =
                build_init_pokes(&constants, &genesis_bytes, &key.pk_b58, &mining_pkh)?;

            let mut spendable_notes: VecDeque<(Name, NoteV0)> = VecDeque::new();
            let mut seen_notes: HashSet<(Hash, Hash)> = HashSet::new();
            let mut pending_txs: Vec<PendingTx> = Vec::new();
            let mut blocks_mined = 0usize;
            let mut used_bytes = 0u64;
            let mut pending_candidate: Option<MiningCandidate> = None;
            let mut saw_candidate = false;
            let mut last_heaviest_block_id: Option<String> = None;
            let mut tx_effects = TxEffectStats::default();
            let mut logged_invalid = false;

            while blocks_mined < target_blocks && used_bytes < target_bytes {
                let Some(poke) = init_pokes.pop_front() else {
                    if let Some(candidate) = pending_candidate.as_ref() {
                        let pow_poke = create_pow_poke(&candidate, &random_nonce(&mut rng));
                        init_pokes.push_back(Poke {
                            wire: MiningWire::Mined.to_wire(),
                            noun: pow_poke,
                            tx_id: None,
                        });
                        continue;
                    }
                    if saw_candidate {
                        return Err("No pending pokes while target not reached; mining stalled"
                            .to_string()
                            .into());
                    }
                    return Err("Mining never started (no %mine effect observed)"
                        .to_string()
                        .into());
                };
                let effects = app.poke(poke.wire, poke.noun).await?;
                let mut tx_seen = false;
                let mut tx_accepted = false;
                let mut tx_liar = false;
                let record_tx_effects = poke.tx_id.is_some();
                if record_tx_effects {
                    tx_effects.sent += 1;
                }
                let mut stop = false;
                let mut queued_tx = false;
                for effect in effects {
                    if let Some(candidate_effect) = parse_mine_effect(&effect)? {
                        pending_candidate = Some(candidate_effect);
                        saw_candidate = true;
                        continue;
                    }

                    if record_tx_effects {
                        if let Some(cause) = parse_liar_effect(&effect)? {
                            tx_liar = true;
                            tx_effects.record_liar(cause);
                        }

                        if let Some(seen_tx_id) = parse_seen_tx_id(&effect)? {
                            if poke.tx_id.as_ref() == Some(&seen_tx_id) {
                                tx_seen = true;
                            }
                        }
                    }

                    if let Some(mut gossip) = extract_gossip_data(&effect)? {
                        if record_tx_effects {
                            if let Some(tx_id) = heard_tx_id(&mut gossip)? {
                                if poke.tx_id.as_ref() == Some(&tx_id) {
                                    tx_accepted = true;
                                }
                            }
                        }

                        if let Some((block_id, page)) = heard_block_page(&mut gossip)? {
                            if block_id == genesis_id {
                                continue;
                            }
                            if last_heaviest_block_id.as_deref() == Some(block_id.as_str()) {
                                continue;
                            }
                            last_heaviest_block_id = Some(block_id);
                            blocks_mined += 1;

                            if !skip_txs {
                                if !pending_txs.is_empty() {
                                    let mut next_pending = Vec::with_capacity(pending_txs.len());
                                    for pending in pending_txs.drain(..) {
                                        if page_contains_tx_id(&mut gossip, page, &pending.id)? {
                                            let height = page_height(page, &gossip.noun_space())?;
                                            let mut note = pending.refund_note;
                                            note.head.origin_page = BlockHeight(Belt(height));
                                            let key = note_key(&pending.refund_name);
                                            if seen_notes.insert(key) {
                                                spendable_notes
                                                    .push_back((pending.refund_name, note));
                                            }
                                        } else {
                                            next_pending.push(pending);
                                        }
                                    }
                                    pending_txs = next_pending;
                                }

                                if spendable_notes.len() < note_pool_target {
                                    refresh_note_pool(
                                        &mut app, &key.pk_b58, &mut spendable_notes,
                                        &mut seen_notes,
                                    )
                                    .await?;
                                }
                            }

                            if blocks_mined % bytes_check_interval == 0 {
                                used_bytes = pma_used_bytes(&pma_path)?;
                                info!(
                                    "paging: blocks={} used_bytes={} target_bytes={}",
                                    blocks_mined, used_bytes, target_bytes
                                );
                            }

                            if blocks_mined >= target_blocks || used_bytes >= target_bytes {
                                stop = true;
                                break;
                            }

                            if !skip_txs {
                                let available_slots =
                                    txs_per_block.saturating_sub(pending_txs.len());
                                for _ in 0..available_slots {
                                    let Some((name, note)) = spendable_notes.pop_front() else {
                                        break;
                                    };
                                    let next_height = (blocks_mined + 1) as u64;
                                    let tx = build_tx_plan(
                                        &mut tx_hasher, &key, &name, &note, next_height,
                                        outputs_per_tx, extra_gift, &lock_pool,
                                    )?;
                                    if !logged_invalid {
                                        let mut check_hasher = TxHasher::new();
                                        if let Err(reason) =
                                            validate_tx_plan(&tx.raw_tx, &mut check_hasher)
                                        {
                                            warn!("pma_paging_kernel: tx-plan invalid: {}", reason);
                                            logged_invalid = true;
                                        }
                                    }
                                    let heard_tx = make_heard_tx_poke(&tx.raw_tx)?;
                                    init_pokes.push_back(Poke {
                                        wire: gossip_wire.clone(),
                                        noun: heard_tx,
                                        tx_id: Some(tx.raw_tx.id.clone()),
                                    });
                                    pending_txs.push(PendingTx {
                                        id: tx.raw_tx.id.clone(),
                                        refund_name: tx.refund_name,
                                        refund_note: tx.refund_note,
                                    });
                                    queued_tx = true;
                                }
                            }
                        }
                    }
                }

                if stop {
                    break;
                }
                if queued_tx {
                    continue;
                }
                if poke.tx_id.is_some() {
                    if tx_accepted {
                        tx_effects.accepted += 1;
                    }
                    if tx_seen {
                        tx_effects.seen += 1;
                    }
                    if !tx_accepted && !tx_seen && !tx_liar {
                        tx_effects.dropped += 1;
                    }
                }
            }

            used_bytes = pma_used_bytes(&pma_path)?;
            info!(
                "paging: finished blocks={} used_bytes={} target_bytes={}",
                blocks_mined, used_bytes, target_bytes
            );
            info!(
                "paging: tx-effects sent={} accepted={} seen={} dropped={}",
                tx_effects.sent, tx_effects.accepted, tx_effects.seen, tx_effects.dropped
            );
            if !tx_effects.liar_causes.is_empty() {
                let mut causes: Vec<(String, u64)> = tx_effects.liar_causes.into_iter().collect();
                causes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                for (cause, count) in causes {
                    info!("paging: tx-effects liar cause {} = {}", cause, count);
                }
            }

            validate_paging_with_heaviest_peek(&mut app, &pma_path, used_bytes).await?;
            drop(app);
            validate_paging(&pma_path, used_bytes)?;
            drop(temp_dir);
            Ok::<(), Box<dyn Error>>(())
        })
        .expect("paging test failed");
}

fn build_tx_plan(
    hasher: &mut TxHasher,
    key: &KeyMaterial,
    name: &Name,
    note: &NoteV0,
    next_height: u64,
    outputs_per_tx: usize,
    extra_gift: u64,
    lock_pool: &[Lock],
) -> Result<TxPlan, Box<dyn Error>> {
    hasher.reset();
    let lock = lock_pool
        .first()
        .expect("lock pool must not be empty")
        .clone();
    hasher.cache_lock(&lock);
    let parent_hash = hasher.hash_nnote(note);

    let input_assets = note.tail.assets.0 as u64;
    let max_outputs_by_assets = if extra_gift == 0 {
        outputs_per_tx
    } else {
        (input_assets / extra_gift) as usize
    };
    let extra_outputs = outputs_per_tx.min(max_outputs_by_assets);
    let extra_gift_total = extra_gift.saturating_mul(extra_outputs as u64);
    let refund_gift = input_assets.saturating_sub(extra_gift_total);

    let refund_seed = Seed {
        output_source: None,
        recipient: note.tail.lock.clone(),
        timelock_intent: None,
        gift: Nicks(refund_gift as usize),
        parent_hash: parent_hash.clone(),
    };

    let mut seeds: Vec<Seed> = Vec::with_capacity(extra_outputs + 1);
    seeds.push(refund_seed.clone());
    for idx in 0..extra_outputs {
        let timelock_intent = Some(TimelockIntent {
            absolute: TimelockRangeAbsolute::none(),
            relative: TimelockRangeRelative::new(
                Some(BlockHeightDelta(Belt((idx + 1) as u64))),
                None,
            ),
        });
        seeds.push(Seed {
            output_source: None,
            recipient: lock.clone(),
            timelock_intent,
            gift: Nicks(extra_gift as usize),
            parent_hash: parent_hash.clone(),
        });
    }

    let seeds_struct = Seeds {
        seeds: seeds.clone(),
    };
    let fee = Nicks(0);

    let sig_hash = hasher.sig_hash(&seeds_struct, &fee);
    let signature = sign_schnorr(&key.sk, &sig_hash)?;
    let signature = Signature(vec![(key.pk.clone(), signature)]);

    let spend = Spend {
        signature: Some(signature),
        seeds: seeds_struct.clone(),
        fee,
    };

    let input = Input {
        note: note.clone(),
        spend,
    };
    let inputs = Inputs(vec![(name.clone(), input)]);
    let timelock_range = timelock_range_for_note(note);
    let total_fees = Nicks(0);

    let id = hasher.hash_raw_tx_id(&inputs, &timelock_range, &total_fees);
    let raw_tx = RawTx {
        id,
        inputs,
        timelock_range,
        total_fees,
    };

    let refund_seeds = Seeds {
        seeds: vec![refund_seed.clone()],
    };
    let source_hash = hasher.hash_seeds(&refund_seeds);
    let source = Source {
        hash: source_hash,
        is_coinbase: false,
    };
    let timelock = Timelock(None);
    let refund_name = compute_nname(hasher, &note.tail.lock, &source, &timelock);
    let refund_note = NoteV0 {
        head: NoteHead {
            version: Version::V0,
            origin_page: BlockHeight(Belt(next_height)),
            timelock,
        },
        tail: NoteTail {
            name: refund_name.clone(),
            lock: note.tail.lock.clone(),
            source,
            assets: Nicks(refund_gift as usize),
        },
    };

    Ok(TxPlan {
        raw_tx,
        refund_note,
        refund_name,
    })
}

fn compute_nname(hasher: &mut TxHasher, lock: &Lock, source: &Source, timelock: &Timelock) -> Name {
    let has_timelock = timelock.0.is_some();
    let sig_hash = hasher.hash_sig(lock);
    let first = {
        let leaf_true = hasher.hashable_leaf(YES);
        let leaf_has_timelock = hasher.hashable_leaf(if has_timelock { YES } else { NO });
        let sig_hashable = hasher.hashable_hash(&sig_hash);
        let leaf_null = hasher.hashable_leaf(SIG);
        let hashable = T(
            &mut hasher.stack,
            &[leaf_true, leaf_has_timelock, sig_hashable, leaf_null],
        );
        hasher.hash_hashable(hashable, &hasher.stack.noun_space())
    };

    let last = {
        let leaf_true = hasher.hashable_leaf(YES);
        let source_hashable = hasher.hashable_source(source);
        let timelock_hash = hasher.hash_timelock(timelock);
        let timelock_hashable = hasher.hashable_hash(&timelock_hash);
        let leaf_null = hasher.hashable_leaf(SIG);
        let hashable = T(
            &mut hasher.stack,
            &[leaf_true, source_hashable, timelock_hashable, leaf_null],
        );
        hasher.hash_hashable(hashable, &hasher.stack.noun_space())
    };

    Name::new(first, last)
}

fn sign_schnorr(sk: &UBig, msg: &Hash) -> Result<SchnorrSignature, Box<dyn Error>> {
    let sk_limbs = ubig_to_limbs(sk);
    let pubkey = ch_scal_big(sk, &A_GEN)?;
    let mut transcript = Vec::with_capacity(6 + 6 + 5 + 8);
    transcript.extend_from_slice(&pubkey.x.0);
    transcript.extend_from_slice(&pubkey.y.0);
    transcript.extend_from_slice(&msg.0);
    transcript.extend(sk_limbs.iter().map(|v| Belt(*v as u64)));

    let nonce = trunc_g_order(&hash_varlen(&mut transcript));
    let scalar = ch_scal_big(&nonce, &A_GEN)?;

    let mut pre_image = Vec::with_capacity(6 + 6 + 6 + 6 + 5);
    pre_image.extend_from_slice(&scalar.x.0);
    pre_image.extend_from_slice(&scalar.y.0);
    pre_image.extend_from_slice(&pubkey.x.0);
    pre_image.extend_from_slice(&pubkey.y.0);
    pre_image.extend_from_slice(&msg.0);

    let chal = trunc_g_order(&hash_varlen(&mut pre_image));
    let sig = (nonce + (&chal * sk)) % &*G_ORDER;

    Ok(SchnorrSignature {
        chal: limbs_to_t8(&chal),
        sig: limbs_to_t8(&sig),
    })
}

fn verify_schnorr(pubkey: &SchnorrPubkey, msg: &Hash, sig: &SchnorrSignature) -> bool {
    let chal = t8_to_ubig(&sig.chal);
    let sig_val = t8_to_ubig(&sig.sig);
    if chal == UBig::from(0u8) || chal >= *G_ORDER {
        return false;
    }
    if sig_val == UBig::from(0u8) || sig_val >= *G_ORDER {
        return false;
    }
    let sig_point = match ch_scal_big(&sig_val, &A_GEN) {
        Ok(point) => point,
        Err(_) => return false,
    };
    let chal_point = match ch_scal_big(&chal, &pubkey.0) {
        Ok(point) => point,
        Err(_) => return false,
    };
    let scalar = match ch_add(&sig_point, &ch_neg(&chal_point)) {
        Ok(point) => point,
        Err(_) => return false,
    };
    if scalar == A_ID {
        return false;
    }
    let mut pre_image = Vec::with_capacity(6 + 6 + 6 + 6 + 5);
    pre_image.extend_from_slice(&scalar.x.0);
    pre_image.extend_from_slice(&scalar.y.0);
    pre_image.extend_from_slice(&pubkey.0.x.0);
    pre_image.extend_from_slice(&pubkey.0.y.0);
    pre_image.extend_from_slice(&msg.0);
    trunc_g_order(&hash_varlen(&mut pre_image)) == chal
}

fn ubig_to_limbs(value: &UBig) -> [u32; 8] {
    let mut bytes = value.to_le_bytes();
    bytes.resize(32, 0);
    let mut limbs = [0u32; 8];
    for i in 0..8 {
        let mut limb_bytes = [0u8; 4];
        limb_bytes.copy_from_slice(&bytes[i * 4..(i + 1) * 4]);
        limbs[i] = u32::from_le_bytes(limb_bytes);
    }
    limbs
}

fn limbs_to_t8(value: &UBig) -> [Belt; 8] {
    let limbs = ubig_to_limbs(value);
    let mut out = [Belt(0); 8];
    for (idx, limb) in limbs.iter().enumerate() {
        out[idx] = Belt(*limb as u64);
    }
    out
}

fn t8_to_ubig(value: &[Belt; 8]) -> UBig {
    let base = UBig::from(1u64 << 32);
    let mut factor = UBig::from(1u8);
    let mut result = UBig::from(0u8);
    for limb in value.iter() {
        result += UBig::from(limb.0) * &factor;
        factor *= &base;
    }
    result
}

#[test]
fn verify_sample_raw_tx_signatures() -> Result<(), Box<dyn Error>> {
    const RAW_TX_JAM: &[u8] = include_bytes!("../../nockchain-types/jams/v0/raw-tx.jam");

    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from_static(RAW_TX_JAM))?;
    let space = slab.noun_space();
    let raw_tx = RawTx::from_noun(&noun, &space)?;

    let mut hasher = TxHasher::new();
    let expected_id =
        hasher.hash_raw_tx_id(&raw_tx.inputs, &raw_tx.timelock_range, &raw_tx.total_fees);
    assert_eq!(expected_id, raw_tx.id, "raw-tx id mismatch");

    let mut expected_range = TimelockRangeAbsolute::none();
    for (_, input) in &raw_tx.inputs.0 {
        let range = timelock_range_for_note(&input.note);
        expected_range = merge_timelock_range(&expected_range, &range);
    }
    assert_eq!(
        expected_range, raw_tx.timelock_range,
        "raw-tx timelock range mismatch",
    );

    for (_, input) in &raw_tx.inputs.0 {
        let sig_hash = hasher.sig_hash(&input.spend.seeds, &input.spend.fee);
        let signature = input
            .spend
            .signature
            .as_ref()
            .ok_or("raw-tx input missing signature")?;
        for (pk, sig) in &signature.0 {
            assert!(
                verify_schnorr(pk, &sig_hash, sig),
                "raw-tx signature failed verification",
            );
        }
    }

    Ok(())
}

fn validate_tx_plan(raw_tx: &RawTx, hasher: &mut TxHasher) -> Result<(), String> {
    let Some((name, input)) = raw_tx.inputs.0.first() else {
        return Err("missing input".to_string());
    };
    if input.note.tail.assets.0 as u64 >= PRIME {
        return Err("input assets not based".to_string());
    }
    if *name != input.note.tail.name {
        return Err("input name mismatch".to_string());
    }
    if raw_tx.total_fees.0 as u64 >= PRIME {
        return Err("total_fees not based".to_string());
    }
    if raw_tx.total_fees != input.spend.fee {
        return Err("total_fees mismatch".to_string());
    }
    let expected_timelock = timelock_range_for_note(&input.note);
    if raw_tx.timelock_range != expected_timelock {
        return Err("timelock_range mismatch".to_string());
    }
    let mut gifts = 0u64;
    for seed in &input.spend.seeds.seeds {
        if seed.gift.0 as u64 >= PRIME {
            return Err("seed gift not based".to_string());
        }
        gifts = gifts.saturating_add(seed.gift.0 as u64);
    }
    let total = gifts.saturating_add(input.spend.fee.0 as u64);
    if total != input.note.tail.assets.0 as u64 {
        return Err("gifts+fee mismatch".to_string());
    }
    let note_hash = hasher.hash_nnote(&input.note);
    for seed in &input.spend.seeds.seeds {
        if seed.parent_hash != note_hash {
            return Err("parent_hash mismatch".to_string());
        }
    }
    let signature = input
        .spend
        .signature
        .as_ref()
        .ok_or_else(|| "missing signature".to_string())?;
    let required = input.note.tail.lock.keys_required as usize;
    if signature.0.len() < required {
        return Err("insufficient signatures".to_string());
    }
    for (pk, sig) in &signature.0 {
        if !input.note.tail.lock.pubkeys.iter().any(|k| k == pk) {
            return Err("signature key not in lock".to_string());
        }
        let sig_hash = hasher.sig_hash(&input.spend.seeds, &input.spend.fee);
        if !verify_schnorr(pk, &sig_hash, sig) {
            return Err("signature verify failed".to_string());
        }
    }
    let expected_id =
        hasher.hash_raw_tx_id(&raw_tx.inputs, &raw_tx.timelock_range, &raw_tx.total_fees);
    if raw_tx.id != expected_id {
        return Err("raw_tx id mismatch".to_string());
    }
    Ok(())
}

fn owned_lock(
    pubkeys_per_output: usize,
    owner: &SchnorrPubkey,
    rng: &mut StdRng,
) -> Result<Lock, Box<dyn Error>> {
    let count = pubkeys_per_output.max(1);
    let mut pubkeys = Vec::with_capacity(count);
    pubkeys.push(owner.clone());
    for _ in 1..count {
        let sk = random_scalar(rng);
        let pk = SchnorrPubkey(ch_scal_big(&sk, &A_GEN)?);
        pubkeys.push(pk);
    }
    Ok(Lock {
        keys_required: 1,
        pubkeys,
    })
}

fn random_scalar(rng: &mut StdRng) -> UBig {
    loop {
        let mut bytes = [0u8; 32];
        rng.fill_bytes(&mut bytes);
        let candidate = UBig::from_be_bytes(&bytes);
        if candidate > UBig::from(0u8) && candidate < *G_ORDER {
            return candidate;
        }
    }
}

fn generate_key(rng: &mut StdRng) -> Result<KeyMaterial, Box<dyn Error>> {
    let sk = random_scalar(rng);
    let pk = SchnorrPubkey(ch_scal_big(&sk, &A_GEN)?);
    let pk_b58 = pk.0.into_base58()?;
    Ok(KeyMaterial { sk, pk, pk_b58 })
}

async fn build_nockapp(name: &str) -> Result<(TempDir, NockApp, PathBuf), Box<dyn Error>> {
    let temp_dir = TempDir::new()?;
    let data_dir = temp_dir.path().join(name);
    let hot_state = produce_prover_hot_state();
    let cli = boot::Cli {
        new: true,
        trace_opts: TraceOpts::default(),
        gc_interval: None,
        rotating_snapshot_interval_event_time: None,
        color: clap::ColorChoice::Auto,
        state_jam: None,
        bootstrap_from_chkjam: None,
        export_state_jam: None,
        stack_size: NockStackSize::Medium,
        data_dir: None,
        event_log_path: None,
        disable_fsync: false,
    };
    boot::init_default_tracing(&cli);
    let app = boot::setup(
        NOCKCHAIN_KERNEL,
        cli,
        hot_state.as_slice(),
        name,
        Some(temp_dir.path().to_path_buf()),
    )
    .await?;
    Ok((temp_dir, app, data_dir))
}

fn fakenet_constants() -> BlockchainConstants {
    setup::fakenet_blockchain_constants(2, 1)
        .with_update_candidate_timestamp_interval(Seconds(0))
        .with_pow_len(2)
        .with_v1_phase(u64::MAX)
        .with_first_month_coinbase_min(0)
        .with_coinbase_timelock_min(0)
}

fn build_init_pokes(
    constants: &BlockchainConstants,
    genesis_bytes: &[u8],
    v0_pubkey: &str,
    mining_pkh: &str,
) -> Result<VecDeque<Poke>, Box<dyn Error>> {
    let mut pokes = VecDeque::new();
    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: make_set_constants_poke(constants),
        tx_id: None,
    });
    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: make_set_genesis_seal_poke(setup::FAKENET_GENESIS_MESSAGE),
        tx_id: None,
    });
    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: make_set_btc_data_poke(),
        tx_id: None,
    });
    pokes.push_back(Poke {
        wire: MiningWire::SetPubKey.to_wire(),
        noun: make_set_mining_key_poke(v0_pubkey, mining_pkh),
        tx_id: None,
    });
    pokes.push_back(Poke {
        wire: MiningWire::Enable.to_wire(),
        noun: make_enable_mining_poke(true),
        tx_id: None,
    });
    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: make_born_poke(),
        tx_id: None,
    });
    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: setup::heard_fake_genesis_block(Some(genesis_bytes.to_vec()))?,
        tx_id: None,
    });
    Ok(pokes)
}

fn make_set_constants_poke(constants: &BlockchainConstants) -> NounSlab {
    let mut poke_slab = NounSlab::new();
    let tag = make_tas(&mut poke_slab, "set-constants").as_noun();
    let constants_noun = constants.to_noun(&mut poke_slab);
    let poke_noun = T(&mut poke_slab, &[D(tas!(b"command")), tag, constants_noun]);
    poke_slab.set_root(poke_noun);
    poke_slab
}

fn make_set_genesis_seal_poke(seal: &str) -> NounSlab {
    let mut poke_slab = NounSlab::new();
    let block_height_noun = Atom::new(&mut poke_slab, DEFAULT_GENESIS_BLOCK_HEIGHT).as_noun();
    let seal_byts = Bytes::from(seal.to_string().into_bytes());
    let seal_noun = Atom::from_bytes(&mut poke_slab, &seal_byts).as_noun();
    let tag = Bytes::from(b"set-genesis-seal".to_vec());
    let set_genesis_seal = Atom::from_bytes(&mut poke_slab, &tag).as_noun();
    let poke_noun = T(
        &mut poke_slab,
        &[D(tas!(b"command")), set_genesis_seal, block_height_noun, seal_noun],
    );
    poke_slab.set_root(poke_noun);
    poke_slab
}

fn make_set_btc_data_poke() -> NounSlab {
    let mut poke_slab = NounSlab::new();
    let poke_noun = T(
        &mut poke_slab,
        &[D(tas!(b"command")), D(tas!(b"btc-data")), D(0)],
    );
    poke_slab.set_root(poke_noun);
    poke_slab
}

fn make_born_poke() -> NounSlab {
    let mut poke_slab = NounSlab::new();
    let born = T(
        &mut poke_slab,
        &[D(tas!(b"command")), D(tas!(b"born")), D(0)],
    );
    poke_slab.set_root(born);
    poke_slab
}

fn make_enable_mining_poke(enable: bool) -> NounSlab {
    let mut slab = NounSlab::new();
    let enable_mining =
        Atom::from_value(&mut slab, "enable-mining").expect("Failed to create enable-mining atom");
    let enable_mining_poke = T(
        &mut slab,
        &[D(tas!(b"command")), enable_mining.as_noun(), if enable { YES } else { NO }],
    );
    slab.set_root(enable_mining_poke);
    slab
}

fn make_set_mining_key_poke(v0_pubkey: &str, pkh: &str) -> NounSlab {
    let mut slab = NounSlab::new();
    let set_mining_key_adv = Atom::from_value(&mut slab, "set-mining-key-advanced")
        .expect("Failed to create set-mining-key-advanced atom");

    let mut configs_list = D(0);
    let mut keys_noun = D(0);
    let key_atom = Atom::from_value(&mut slab, v0_pubkey)
        .expect("Failed to create key atom")
        .as_noun();
    keys_noun = T(&mut slab, &[key_atom, keys_noun]);
    let config_tuple = T(&mut slab, &[D(1), D(1), keys_noun]);
    configs_list = T(&mut slab, &[config_tuple, configs_list]);

    let mut pkh_configs_list = D(0);
    let pkh_noun = Atom::from_value(&mut slab, pkh)
        .expect("Failed to create pkh atom")
        .as_noun();
    let pkh_tuple = T(&mut slab, &[D(1), pkh_noun]);
    pkh_configs_list = T(&mut slab, &[pkh_tuple, pkh_configs_list]);

    let set_mining_key_poke = T(
        &mut slab,
        &[
            D(tas!(b"command")),
            set_mining_key_adv.as_noun(),
            configs_list,
            pkh_configs_list,
        ],
    );
    slab.set_root(set_mining_key_poke);
    slab
}

fn make_heard_tx_poke(raw_tx: &RawTx) -> Result<NounSlab, NockAppError> {
    let mut slab = NounSlab::new();
    let tx_noun = raw_tx.to_noun(&mut slab);
    let tag = make_tas(&mut slab, "heard-tx").as_noun();
    let poke_noun = T(&mut slab, &[D(tas!(b"fact")), D(0), tag, tx_noun]);
    slab.set_root(poke_noun);
    Ok(slab)
}

fn parse_mine_effect(effect: &NounSlab) -> Result<Option<MiningCandidate>, NockAppError> {
    let Ok(effect_cell) = (unsafe { effect.root().as_cell() }) else {
        return Ok(None);
    };
    let space = effect.noun_space();
    let effect_cell = effect_cell.in_space(&space);
    if !effect_cell.head().eq_bytes("mine") {
        return Ok(None);
    }
    let Ok([version, commit, target, pow_len_noun]) = effect_cell.tail().noun().uncell(&space)
    else {
        return Err(NockAppError::OtherError(
            "Expected four elements in %mine effect".to_string(),
        ));
    };
    let pow_len = pow_len_noun
        .in_space(&space)
        .as_atom()?
        .as_u64()
        .map_err(|_| NockAppError::OtherError("pow-len was not a u64".to_string()))?;

    let mut version_slab = NounSlab::new();
    version_slab.copy_into(version, &space);
    let mut header_slab = NounSlab::new();
    header_slab.copy_into(commit, &space);
    let mut target_slab = NounSlab::new();
    target_slab.copy_into(target, &space);

    Ok(Some(MiningCandidate {
        version: version_slab,
        header: header_slab,
        _target: target_slab,
        _pow_len: pow_len,
    }))
}

fn extract_gossip_data(effect: &NounSlab) -> Result<Option<NounSlab>, NockAppError> {
    let Ok(effect_cell) = (unsafe { effect.root().as_cell() }) else {
        return Ok(None);
    };
    let space = effect.noun_space();
    let effect_cell = effect_cell.in_space(&space);
    if !effect_cell.head().eq_bytes("gossip") {
        return Ok(None);
    }
    let gossip_cell = effect_cell.tail().noun();
    let data = gossip_cell.in_space(&space).as_cell()?.tail().noun();
    let mut data_slab = NounSlab::new();
    data_slab.copy_into(data, &space);
    Ok(Some(data_slab))
}

fn heard_block_page(gossip: &mut NounSlab) -> Result<Option<(String, Noun)>, NockAppError> {
    let noun = unsafe { gossip.root() };
    let space = gossip.noun_space();
    let head = noun.in_space(&space).as_cell()?.head();
    if !head.eq_bytes(b"heard-block") {
        return Ok(None);
    }

    let page = noun.in_space(&space).as_cell()?.tail().noun();
    let block_id = block_id_from_page(page, &space)?;
    let block_id_str = tip5_hash_to_base58_stack(gossip, block_id, &space)?;
    Ok(Some((block_id_str, page)))
}

fn heard_tx_id(gossip: &mut NounSlab) -> Result<Option<Hash>, NockAppError> {
    let noun = unsafe { gossip.root() };
    let space = gossip.noun_space();
    let head = noun.in_space(&space).as_cell()?.head();
    if !head.eq_bytes(b"heard-tx") {
        return Ok(None);
    }
    let raw_tx = noun.in_space(&space).as_cell()?.tail().noun();
    let raw_tx = RawTx::from_noun(&raw_tx, &space)
        .map_err(|_| NockAppError::OtherError("heard-tx raw-tx decode failed".to_string()))?;
    Ok(Some(raw_tx.id))
}

fn parse_liar_effect(effect: &NounSlab) -> Result<Option<String>, NockAppError> {
    let Ok(effect_cell) = (unsafe { effect.root().as_cell() }) else {
        return Ok(None);
    };
    let space = effect.noun_space();
    let effect_cell = effect_cell.in_space(&space);
    let head = effect_cell.head();
    if !head.eq_bytes("liar-peer") && !head.eq_bytes("liar-block-id") {
        return Ok(None);
    }
    let cause = effect_cell
        .tail()
        .noun()
        .in_space(&space)
        .as_cell()?
        .tail()
        .noun()
        .in_space(&space)
        .as_atom()?
        .into_string()
        .unwrap_or_else(|_| "<non-utf8>".to_string());
    Ok(Some(cause))
}

fn parse_seen_tx_id(effect: &NounSlab) -> Result<Option<Hash>, NockAppError> {
    let Ok(effect_cell) = (unsafe { effect.root().as_cell() }) else {
        return Ok(None);
    };
    let space = effect.noun_space();
    let effect_cell = effect_cell.in_space(&space);
    if !effect_cell.head().eq_bytes("seen") {
        return Ok(None);
    }
    let seen_tail = effect_cell.tail().noun().in_space(&space).as_cell()?;
    if !seen_tail.head().eq_bytes("tx") {
        return Ok(None);
    }
    let id_noun = seen_tail.tail().noun();
    let id = Hash::from_noun(&id_noun, &space)
        .map_err(|_| NockAppError::OtherError("seen tx id decode failed".to_string()))?;
    Ok(Some(id))
}

fn block_id_from_page(page: Noun, space: &NounSpace) -> Result<Noun, NockAppError> {
    let page_cell = page.in_space(space).as_cell()?;
    match page_cell.head().as_atom() {
        Ok(version_atom) => {
            let version = version_atom.as_u64()?;
            if version == 1 {
                Ok(page_cell.tail().as_cell()?.head().noun())
            } else {
                Err(NockAppError::OtherError(format!(
                    "Unsupported page version {}",
                    version
                )))
            }
        }
        Err(_) => Ok(page_cell.head().noun()),
    }
}

fn page_field(page: Noun, space: &NounSpace, index: usize) -> Result<Noun, NockAppError> {
    let page_cell = page.in_space(space).as_cell()?;
    let mut cell = match page_cell.head().as_atom() {
        Ok(version_atom) => {
            let version = version_atom.as_u64()?;
            if version == 1 {
                page_cell.tail().as_cell()?
            } else {
                return Err(NockAppError::OtherError(format!(
                    "Unsupported page version {}",
                    version
                )));
            }
        }
        Err(_) => page_cell,
    };

    for _ in 0..index {
        cell = cell.tail().as_cell()?;
    }
    Ok(cell.head().noun())
}

fn page_tx_ids(page: Noun, space: &NounSpace) -> Result<Noun, NockAppError> {
    page_field(page, space, 3)
}

fn page_height(page: Noun, space: &NounSpace) -> Result<u64, NockAppError> {
    let height_noun = page_field(page, space, 9)?;
    height_noun
        .in_space(space)
        .as_atom()?
        .as_u64()
        .map_err(|_| NockAppError::OtherError("page height was not a u64".to_string()))
}

fn page_contains_tx_id(
    gossip: &mut NounSlab,
    page: Noun,
    tx_id: &Hash,
) -> Result<bool, NockAppError> {
    let tx_id_noun = tx_id.to_noun(gossip);
    let space = gossip.noun_space();
    let tx_ids = page_tx_ids(page, &space)?;
    z_set_contains(tx_ids, &space, tx_id_noun)
}

fn z_set_contains(node: Noun, space: &NounSpace, target: Noun) -> Result<bool, NockAppError> {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if unsafe { node.raw_equals(&D(0)) } {
            continue;
        }
        let Ok([entry, left, right]) = node.uncell(space) else {
            return Err(NockAppError::OtherError(
                "tx-ids node not a cell".to_string(),
            ));
        };
        if noun_equality(entry.in_space(space), target.in_space(space)) {
            return Ok(true);
        }
        stack.push(left);
        stack.push(right);
    }
    Ok(false)
}

fn genesis_block_id(genesis_bytes: &[u8]) -> Result<String, Box<dyn Error>> {
    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from(genesis_bytes.to_vec()))?;
    let space = slab.noun_space();
    let block_id = block_id_from_page(noun, &space)?;
    Ok(tip5_hash_to_base58_stack(&mut slab, block_id, &space)?)
}

fn create_pow_poke(candidate: &MiningCandidate, nonce: &NounSlab) -> NounSlab {
    let mut slab = NounSlab::new();
    let version_space = candidate.version.noun_space();
    let header_space = candidate.header.noun_space();
    let nonce_space = nonce.noun_space();
    let version = slab.copy_into(unsafe { *candidate.version.root() }, &version_space);
    let header = slab.copy_into(unsafe { *candidate.header.root() }, &header_space);
    let nonce = slab.copy_into(unsafe { *nonce.root() }, &nonce_space);
    let proof = T(&mut slab, &[version, D(0), D(0), D(0)]);
    let poke_noun = T(
        &mut slab,
        &[D(tas!(b"command")), D(tas!(b"pow")), proof, D(0), header, nonce],
    );
    slab.set_root(poke_noun);
    slab
}

fn random_nonce(rng: &mut StdRng) -> NounSlab {
    let mut nonce_slab = NounSlab::new();
    let mut nonce_cell = Atom::from_value(&mut nonce_slab, rng.random::<u64>() % PRIME)
        .expect("nonce atom")
        .as_noun();
    for _ in 1..5 {
        let nonce_atom = Atom::from_value(&mut nonce_slab, rng.random::<u64>() % PRIME)
            .expect("nonce atom")
            .as_noun();
        nonce_cell = T(&mut nonce_slab, &[nonce_atom, nonce_cell]);
    }
    nonce_slab.set_root(nonce_cell);
    nonce_slab
}

fn note_key(name: &Name) -> (Hash, Hash) {
    (name.first.clone(), name.last.clone())
}

fn note_is_spendable(note: &NoteV0, now: u64) -> bool {
    let since = (note.head.origin_page.0).0;
    let Some(intent) = note.head.timelock.0.as_ref() else {
        return true;
    };
    let rel_min = intent.relative.min.as_ref().map(|height| (height.0).0);
    let rel_max = intent.relative.max.as_ref().map(|height| (height.0).0);
    let abs_min = intent.absolute.min.as_ref().map(|height| (height.0).0);
    let abs_max = intent.absolute.max.as_ref().map(|height| (height.0).0);
    let rmin_ok = rel_min.map_or(true, |min| now >= since.saturating_add(min));
    let rmax_ok = rel_max.map_or(true, |max| now <= since.saturating_add(max));
    let amin_ok = abs_min.map_or(true, |min| now >= min);
    let amax_ok = abs_max.map_or(true, |max| now <= max);
    rmin_ok && rmax_ok && amin_ok && amax_ok
}

fn merge_timelock_range(
    a: &TimelockRangeAbsolute,
    b: &TimelockRangeAbsolute,
) -> TimelockRangeAbsolute {
    if a.min.is_none() && a.max.is_none() {
        return b.clone();
    }
    if b.min.is_none() && b.max.is_none() {
        return a.clone();
    }
    let min = match (&a.min, &b.min) {
        (Some(a_min), Some(b_min)) => {
            if (a_min.0).0 >= (b_min.0).0 {
                Some(a_min.clone())
            } else {
                Some(b_min.clone())
            }
        }
        (Some(a_min), None) => Some(a_min.clone()),
        (None, Some(b_min)) => Some(b_min.clone()),
        (None, None) => None,
    };
    let max = match (&a.max, &b.max) {
        (Some(a_max), Some(b_max)) => {
            if (a_max.0).0 <= (b_max.0).0 {
                Some(a_max.clone())
            } else {
                Some(b_max.clone())
            }
        }
        (Some(a_max), None) => Some(a_max.clone()),
        (None, Some(b_max)) => Some(b_max.clone()),
        (None, None) => None,
    };
    TimelockRangeAbsolute::new(min, max)
}

fn add_block_height(origin: &BlockHeight, delta: &BlockHeightDelta) -> BlockHeight {
    BlockHeight(origin.0 + delta.0)
}

fn timelock_range_for_note(note: &NoteV0) -> TimelockRangeAbsolute {
    let Some(intent) = note.head.timelock.0.as_ref() else {
        return TimelockRangeAbsolute::none();
    };
    let origin = &note.head.origin_page;
    let relative = &intent.relative;
    let absolutification = if relative.min.is_none() && relative.max.is_none() {
        TimelockRangeAbsolute::none()
    } else {
        let min = relative
            .min
            .as_ref()
            .map(|height| add_block_height(origin, height));
        let max = relative
            .max
            .as_ref()
            .map(|height| add_block_height(origin, height));
        TimelockRangeAbsolute::new(min, max)
    };
    merge_timelock_range(&absolutification, &intent.absolute)
}

async fn refresh_note_pool(
    app: &mut NockApp,
    pubkey_b58: &str,
    spendable_notes: &mut VecDeque<(Name, NoteV0)>,
    seen_notes: &mut HashSet<(Hash, Hash)>,
) -> Result<(), Box<dyn Error>> {
    let mut path_slab = NounSlab::new();
    let path_noun =
        vec!["balance-by-pubkey".to_string(), pubkey_b58.to_string()].to_noun(&mut path_slab);
    path_slab.set_root(path_noun);
    let result_slab = app.peek(path_slab).await?;
    let result_noun = unsafe { result_slab.root() };
    let space = result_slab.noun_space();
    let balance_opt = Option::<Option<nockchain_types::tx_engine::v0::BalanceUpdate>>::from_noun(
        &result_noun, &space,
    )?;
    let update = balance_opt
        .and_then(|inner| inner)
        .ok_or("missing balance update")?;
    let now = (update.height.0).0;
    for (name, note) in update.notes.0 {
        if note.tail.assets.0 == 0 {
            continue;
        }
        if !note_is_spendable(&note, now) {
            continue;
        }
        if note.tail.lock.keys_required > 1 {
            continue;
        }
        if !note
            .tail
            .lock
            .pubkeys
            .iter()
            .any(|pk| pk.to_base58().ok().as_deref() == Some(pubkey_b58))
        {
            continue;
        }
        let key = note_key(&name);
        if seen_notes.insert(key) {
            spendable_notes.push_back((name, note));
        }
    }
    Ok(())
}

fn find_pma_path(data_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let pma_dir = data_dir.join("pma");
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&pma_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("pma") {
            entries.push(path);
        }
    }
    if entries.is_empty() {
        return Err(format!("no pma slab file found in {:?}", pma_dir).into());
    }
    if entries.len() > 1 {
        warn!("multiple pma files found, using first: {:?}", entries[0]);
    }
    Ok(entries.remove(0))
}

fn pma_used_bytes(path: &Path) -> Result<u64, Box<dyn Error>> {
    let trailer = read_pma_trailer(path)?;
    if trailer.magic != PMA_MAGIC {
        return Err(format!("unexpected PMA magic {:#x}", trailer.magic).into());
    }
    if trailer.version != PMA_VERSION {
        return Err(format!("unexpected PMA version {}", trailer.version).into());
    }
    Ok(trailer.alloc_offset.saturating_mul(8))
}

fn read_pma_trailer(path: &Path) -> Result<PmaTrailer, Box<dyn Error>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len < PMA_TRAILER_BYTES as u64 {
        return Err(format!("PMA file too small: {}", len).into());
    }
    file.seek(SeekFrom::End(-(PMA_TRAILER_BYTES as i64)))?;
    let mut buf = [0u8; PMA_TRAILER_BYTES];
    file.read_exact(&mut buf)?;
    Ok(PmaTrailer::from_bytes(buf))
}

fn validate_paging(path: &Path, used_bytes: u64) -> Result<(), Box<dyn Error>> {
    let page = page_size();
    let len = (used_bytes as usize / page) * page;
    if len == 0 {
        return Err("PMA has no allocated bytes".into());
    }

    let file = File::open(path)?;
    let mmap = unsafe { MmapOptions::new().len(len).map(&file)? };
    let base = mmap.as_ptr() as *mut u8;

    touch_entire_region(base, len, page);
    let resident_bitmap = mincore_bitmap(base, len);
    let initial_ratio = residency_ratio(&resident_bitmap);
    info!("[pma-paging] initial residency ratio {:.3}", initial_ratio);
    if resident_bitmap.iter().all(|b| b & 1 == 1) {
        drop_all_pages(base, len);
    } else {
        warn!("initial residency not full; paging check may be noisy");
    }

    let after_drop = mincore_bitmap(base, len);
    let post_drop_ratio = residency_ratio(&after_drop);
    info!(
        "[pma-paging] post-drop residency ratio {:.3}",
        post_drop_ratio
    );
    if post_drop_ratio > 0.9 {
        warn!(
            "[pma-paging] paging did not drop pages; skipping remainder (ratio={:.3})",
            post_drop_ratio
        );
        return Ok(());
    }

    let touched_pages = fault_sparse(base, len, page, 128);
    let post_fault = mincore_bitmap(base, len);
    let post_fault_ratio = residency_ratio(&post_fault);
    let total_pages = len / page;
    let expected_ratio = touched_pages as f64 / total_pages.max(1) as f64;
    info!(
        "[pma-paging] post-fault residency ratio {:.4} (expected {:.4}, touched {} pages)",
        post_fault_ratio, expected_ratio, touched_pages
    );

    Ok(())
}

async fn validate_paging_with_heaviest_peek(
    app: &mut NockApp,
    path: &Path,
    used_bytes: u64,
) -> Result<(), Box<dyn Error>> {
    let page = page_size();
    let len = (used_bytes as usize / page) * page;
    if len == 0 {
        return Err("PMA has no allocated bytes".into());
    }

    let file = File::open(path)?;
    let mmap = unsafe { MmapOptions::new().len(len).map(&file)? };
    let base = mmap.as_ptr() as *mut u8;

    touch_entire_region(base, len, page);
    let resident_bitmap = mincore_bitmap(base, len);
    let touched_ratio = residency_ratio(&resident_bitmap);
    info!("[pma-paging] pre-peek residency ratio {:.3}", touched_ratio);
    if resident_bitmap.iter().all(|b| b & 1 == 1) {
        drop_all_pages(base, len);
    } else {
        warn!("[pma-paging] pre-peek residency not full; continuing anyway");
    }

    let after_drop = mincore_bitmap(base, len);
    let post_drop_ratio = residency_ratio(&after_drop);
    info!(
        "[pma-paging] pre-peek post-drop residency ratio {:.3}",
        post_drop_ratio
    );

    if peek_heaviest_block(app).await?.is_none() {
        warn!("[pma-paging] heaviest-block peek returned no data; skipping post-peek check");
        return Ok(());
    }

    let post_peek = mincore_bitmap(base, len);
    let post_peek_ratio = residency_ratio(&post_peek);
    info!(
        "[pma-paging] post-peek residency ratio {:.4}",
        post_peek_ratio
    );

    Ok(())
}

async fn peek_heaviest_block(app: &mut NockApp) -> Result<Option<NounSlab>, Box<dyn Error>> {
    let mut path_slab = NounSlab::new();
    let tag = make_tas(&mut path_slab, "heaviest-block").as_noun();
    let path = T(&mut path_slab, &[tag, D(0)]);
    path_slab.set_root(path);
    Ok(app.peek_handle(path_slab).await?)
}

fn touch_entire_region(ptr: *mut u8, len: usize, page: usize) {
    for offset in (0..len).step_by(page) {
        unsafe {
            std::ptr::read_volatile(ptr.add(offset));
        }
    }
}

fn fault_sparse(ptr: *mut u8, len: usize, page: usize, desired_pages: usize) -> usize {
    let total_pages = len / page;
    if total_pages == 0 {
        return 0;
    }
    let touches = desired_pages.min(total_pages.max(1));
    let stride = (total_pages / touches).max(1);
    let mut touched = 0;
    let mut page_idx = 0;
    while touched < touches && page_idx < total_pages {
        unsafe {
            std::ptr::read_volatile(ptr.add(page_idx * page));
        }
        touched += 1;
        page_idx = page_idx.saturating_add(stride);
    }
    touched
}

fn drop_all_pages(ptr: *mut u8, len: usize) {
    #[cfg(target_os = "linux")]
    {
        let ret = unsafe { libc::madvise(ptr as *mut libc::c_void, len, libc::MADV_PAGEOUT) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINVAL) | Some(libc::ENOSYS) => {
                    let fallback = unsafe {
                        libc::madvise(ptr as *mut libc::c_void, len, libc::MADV_DONTNEED)
                    };
                    if fallback != 0 {
                        panic!(
                            "madvise fallback failed: {}",
                            std::io::Error::last_os_error()
                        );
                    }
                }
                _ => panic!("madvise(MADV_PAGEOUT) failed: {err}"),
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let ret = unsafe { libc::madvise(ptr as *mut libc::c_void, len, libc::MADV_DONTNEED) };
        if ret != 0 {
            panic!(
                "madvise(MADV_DONTNEED) failed: {}",
                std::io::Error::last_os_error()
            );
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
}

fn mincore_bitmap(ptr: *mut u8, len: usize) -> Vec<u8> {
    let page = page_size();
    assert_eq!(
        len % page,
        0,
        "mincore requires len to be page sized, len={len}, page={page}"
    );
    let pages = len / page;
    let mut vec = vec![0u8; pages];
    let ret = unsafe { libc::mincore(ptr as *mut libc::c_void, len, vec.as_mut_ptr().cast()) };
    if ret != 0 {
        panic!("mincore failed: {}", std::io::Error::last_os_error());
    }
    vec
}

fn residency_ratio(bitmap: &[u8]) -> f64 {
    if bitmap.is_empty() {
        return 0.0;
    }
    let resident = bitmap.iter().filter(|b| **b & 1 == 1).count();
    resident as f64 / bitmap.len() as f64
}

fn page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .and_then(|value| {
            let value = value.trim().to_ascii_lowercase();
            match value.as_str() {
                "1" | "true" | "yes" | "on" => Some(true),
                "0" | "false" | "no" | "off" => Some(false),
                _ => None,
            }
        })
        .unwrap_or(default)
}
