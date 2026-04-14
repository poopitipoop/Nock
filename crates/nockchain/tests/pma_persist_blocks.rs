use std::collections::{HashSet, VecDeque};
use std::error::Error;
use std::sync::Once;

use clap::ColorChoice;
use kernels_open_dumb::KERNEL as NOCKCHAIN_KERNEL;
use libp2p::PeerId;
use nockapp::kernel::boot::{self, NockStackSize, TraceOpts};
use nockapp::noun::slab::{slab_equality, NounSlab};
use nockapp::utils::make_tas;
use nockapp::wire::{SystemWire, Wire, WireRepr};
use nockapp::{AtomExt, Bytes, NockApp, NockAppError};
use nockchain::mining::MiningWire;
use nockchain::setup::{self, fakenet_blockchain_constants, DEFAULT_GENESIS_BLOCK_HEIGHT};
use nockchain_libp2p_io::driver::Libp2pWire;
use nockchain_libp2p_io::tip5_util::tip5_hash_to_base58_stack;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace, D, NO, T, YES};
use nockvm_macros::tas;
use noun_serde::NounEncode;
use rand::Rng;
use tempfile::TempDir;
use zkvm_jetpack::form::noun_ext::NounMathExt;
use zkvm_jetpack::hot::produce_prover_hot_state;

const DEFAULT_BLOCKS: usize = 25;
const DEFAULT_POW_LEN: u64 = 2;
const DEFAULT_LOG_DIFFICULTY: u64 = 1;
const DEFAULT_MINING_PKH: &str = "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV";
const DEFAULT_V0_PUBKEY: &str = "2cPnE4Z9RevhTv9is9Hmc1amFubEFbUxzCV2Fxb9GxevJstV5VG92oYt6Sai3d3NjLFcsuVXSLx9hikMbD1agv9M267TVw3hV9MCpMfEnGo5LYtjJ7jPyHg8SERPjJRCWTgZ";

struct Poke {
    wire: WireRepr,
    noun: NounSlab,
}

#[derive(Clone)]
struct MiningCandidate {
    version: NounSlab,
    header: NounSlab,
    _target: NounSlab,
    _pow_len: u64,
}

struct MinedBlock {
    id: String,
    page: NounSlab,
}

struct MiningOutput {
    gossips: Vec<NounSlab>,
    blocks: Vec<MinedBlock>,
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
}

#[test]
#[ignore = "Takes a long time, spurious failure"]
fn pma_persist_blocks() {
    let _ = std::env::set_var("NOCKAPP_DISABLE_METRICS", "1");
    let _ = std::env::set_var("GNORT_DISABLE", "1");

    let target_blocks = env_usize("NOCKCHAIN_PMA_PERSIST_BLOCKS", DEFAULT_BLOCKS);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    runtime
        .block_on(async {
            let (_peer1_dir, mut peer1) = build_nockapp("pma-persist-peer1").await?;
            let (_peer2_dir, mut peer2) = build_nockapp("pma-persist-peer2").await?;

            let genesis_bytes = setup::FAKENET_GENESIS_BLOCK.to_vec();
            let genesis_id = genesis_block_id(&genesis_bytes)?;

            let mut constants =
                fakenet_blockchain_constants(DEFAULT_POW_LEN, DEFAULT_LOG_DIFFICULTY);
            constants.check_pow_flag = false;

            let mut peer1_init = build_init_pokes(&constants, &genesis_bytes, true)?;
            let mut peer2_init = build_init_pokes(&constants, &genesis_bytes, false)?;

            apply_init_pokes(&mut peer2, &mut peer2_init).await?;

            let mining_output =
                run_mining_peer(&mut peer1, &mut peer1_init, target_blocks, &genesis_id).await?;

            run_catchup_peer(&mut peer2, &mining_output.gossips, PeerId::random()).await?;

            if mining_output.blocks.len() != target_blocks {
                return Err(format!(
                    "Expected {} blocks, got {}",
                    target_blocks,
                    mining_output.blocks.len()
                )
                .into());
            }

            for block in &mining_output.blocks {
                assert_block_persisted("peer1", &mut peer1, block).await?;
                assert_block_persisted("peer2", &mut peer2, block).await?;
            }

            Ok::<(), Box<dyn Error>>(())
        })
        .expect("pma persist blocks");
}

async fn build_nockapp(name: &str) -> Result<(TempDir, NockApp), Box<dyn Error>> {
    static TRACING_INIT: Once = Once::new();
    let temp_dir = TempDir::new()?;
    let hot_state = produce_prover_hot_state();
    let cli = boot::Cli {
        new: true,
        trace_opts: TraceOpts::default(),
        gc_interval: None,
        rotating_snapshot_interval_event_time: None,
        color: ColorChoice::Auto,
        state_jam: None,
        bootstrap_from_chkjam: None,
        export_state_jam: None,
        stack_size: NockStackSize::Medium,
        data_dir: None,
        event_log_path: None,
        disable_fsync: false,
    };
    TRACING_INIT.call_once(|| boot::init_default_tracing(&cli));
    let app = boot::setup(
        NOCKCHAIN_KERNEL,
        cli,
        hot_state.as_slice(),
        name,
        Some(temp_dir.path().to_path_buf()),
    )
    .await?;
    Ok((temp_dir, app))
}

fn build_init_pokes(
    constants: &setup::BlockchainConstants,
    genesis_bytes: &[u8],
    enable_mining: bool,
) -> Result<VecDeque<Poke>, Box<dyn Error>> {
    let mut pokes = VecDeque::new();

    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: make_set_constants_poke(constants),
    });
    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: make_set_genesis_seal_poke(setup::FAKENET_GENESIS_MESSAGE),
    });
    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: make_set_btc_data_poke(),
    });
    if enable_mining {
        pokes.push_back(Poke {
            wire: MiningWire::SetPubKey.to_wire(),
            noun: make_set_mining_key_poke(DEFAULT_V0_PUBKEY, DEFAULT_MINING_PKH),
        });
        pokes.push_back(Poke {
            wire: MiningWire::Enable.to_wire(),
            noun: make_enable_mining_poke(true),
        });
    }
    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: make_born_poke(),
    });
    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: setup::heard_fake_genesis_block(Some(genesis_bytes.to_vec()))?,
    });

    Ok(pokes)
}

async fn apply_init_pokes(
    nockapp: &mut NockApp,
    pokes: &mut VecDeque<Poke>,
) -> Result<(), Box<dyn Error>> {
    while let Some(poke) = pokes.pop_front() {
        let _ = nockapp.poke(poke.wire, poke.noun).await?;
    }
    Ok(())
}

async fn run_mining_peer(
    nockapp: &mut NockApp,
    pending: &mut VecDeque<Poke>,
    target_blocks: usize,
    genesis_id: &str,
) -> Result<MiningOutput, Box<dyn Error>> {
    let mut gossips = Vec::new();
    let mut blocks = Vec::new();
    let mut seen_blocks: HashSet<String> = HashSet::new();
    let mut mining_started = false;

    while let Some(poke) = pending.pop_front() {
        let effects = nockapp.poke(poke.wire, poke.noun).await?;

        for effect in effects {
            if let Some(candidate) = parse_mine_effect(&effect)? {
                mining_started = true;
                let mined_poke = create_pow_poke(&candidate, &random_nonce());
                pending.push_back(Poke {
                    wire: MiningWire::Mined.to_wire(),
                    noun: mined_poke,
                });
                continue;
            }

            if let Some(mut gossip) = extract_gossip_data(&effect)? {
                if let Some((block, fact_poke)) = extract_block_from_gossip(&mut gossip)? {
                    if block.id == genesis_id {
                        continue;
                    }
                    if seen_blocks.insert(block.id.clone()) {
                        blocks.push(block);
                        gossips.push(fact_poke);
                        if blocks.len() >= target_blocks {
                            break;
                        }
                    }
                }
            }
        }

        if blocks.len() >= target_blocks {
            break;
        }

        if pending.is_empty() {
            return Err("No pending pokes while target not reached; mining stalled"
                .to_string()
                .into());
        }
    }

    if !mining_started {
        return Err("Mining never started (no %mine effect observed)"
            .to_string()
            .into());
    }
    if blocks.len() < target_blocks {
        return Err(format!(
            "Mined {} blocks but target is {}",
            blocks.len(),
            target_blocks
        )
        .into());
    }

    Ok(MiningOutput { gossips, blocks })
}

async fn run_catchup_peer(
    nockapp: &mut NockApp,
    gossips: &[NounSlab],
    peer_id: PeerId,
) -> Result<(), Box<dyn Error>> {
    for gossip in gossips {
        let _ = nockapp
            .poke(Libp2pWire::Gossip(peer_id).to_wire(), gossip.clone())
            .await?;
    }
    Ok(())
}

async fn assert_block_persisted(
    label: &str,
    nockapp: &mut NockApp,
    block: &MinedBlock,
) -> Result<(), Box<dyn Error>> {
    let mut path_slab = NounSlab::new();
    let tag = make_tas(&mut path_slab, "block").as_noun();
    let id = Atom::from_value(&mut path_slab, block.id.as_str())
        .expect("block id atom")
        .as_noun();
    let path = T(&mut path_slab, &[tag, id, D(0)]);
    path_slab.set_root(path);

    let Some(result) = nockapp.peek_handle(path_slab).await? else {
        return Err(format!("{label}: missing block {}", block.id).into());
    };

    if !slab_equality(&block.page, &result) {
        return Err(format!("{label}: block page mismatch {}", block.id).into());
    }
    Ok(())
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

fn extract_block_from_gossip(
    gossip: &mut NounSlab,
) -> Result<Option<(MinedBlock, NounSlab)>, NockAppError> {
    let noun = unsafe { gossip.root() };
    let space = gossip.noun_space();
    let Ok(cell) = noun.in_space(&space).as_cell() else {
        return Ok(None);
    };
    if !cell.head().eq_bytes(b"heard-block") {
        return Ok(None);
    }

    let page = cell.tail().noun();
    let block_id = block_id_from_page(page, &space)?;
    let block_id_str = tip5_hash_to_base58_stack(gossip, block_id, &space)?;

    let mut page_slab = NounSlab::new();
    let page_noun = page_slab.copy_into(page, &space);
    page_slab.set_root(page_noun);

    let mut fact_poke = NounSlab::new();
    fact_poke.copy_from_slab(gossip);
    fact_poke.modify(|response_noun| vec![D(tas!(b"fact")), D(0), response_noun]);

    Ok(Some((
        MinedBlock {
            id: block_id_str,
            page: page_slab,
        },
        fact_poke,
    )))
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

fn random_nonce() -> NounSlab {
    let mut rng = rand::rng();
    let mut nonce_slab = NounSlab::new();
    let mut nonce_cell = Atom::from_value(&mut nonce_slab, rng.random::<u64>())
        .expect("nonce atom")
        .as_noun();
    for _ in 1..5 {
        let nonce_atom = Atom::from_value(&mut nonce_slab, rng.random::<u64>())
            .expect("nonce atom")
            .as_noun();
        nonce_cell = T(&mut nonce_slab, &[nonce_atom, nonce_cell]);
    }
    nonce_slab.set_root(nonce_cell);
    nonce_slab
}

fn make_set_constants_poke(constants: &setup::BlockchainConstants) -> NounSlab {
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
