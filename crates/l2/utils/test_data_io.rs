#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]

use ethrex_blockchain::Blockchain;
use ethrex_common::types::{Block, Genesis, ELASTICITY_MULTIPLIER};
use ethrex_rlp::{decode::RLPDecode, encode::RLPEncode};
use ethrex_storage::{EngineType, Store};
use tracing::info;
use zkvm_interface::io::ProgramInput;

use std::{
    fs::File,
    io::{BufReader, Read as _, Write},
    path::PathBuf,
};

use super::{error::ProverInputError, prover::db::to_prover_db};

// From cmd/ethrex
pub fn read_chain_file(chain_rlp_path: &str) -> Vec<Block> {
    let chain_file = File::open(chain_rlp_path).expect("Failed to open chain rlp file");
    _chain_file(chain_file).expect("Failed to decode chain rlp file")
}

// From cmd/ethrex
pub fn read_genesis_file(genesis_file_path: &str) -> Genesis {
    let genesis_file = std::fs::File::open(genesis_file_path).expect("Failed to open genesis file");
    _genesis_file(genesis_file).expect("Failed to decode genesis file")
}

/// Generates a `test.rlp` file for use by the prover during testing.
/// Place this in the `proposer/mod.rs` file,
/// specifically in the `start` function,
/// before calling `send_commitment()` to send the block commitment.
pub async fn generate_rlp(
    up_to_block_number: u64,
    store: &Store,
) -> Result<(), Box<dyn std::error::Error>> {
    if store.get_latest_block_number().await? == up_to_block_number {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let file_name = "l2-test.rlp";

        path.push(file_name);

        let mut file = std::fs::File::create(path.to_str().unwrap())?;
        for i in 1..up_to_block_number {
            let body = store.get_block_body(i).await?.unwrap();
            let header = store.get_block_header(i)?.unwrap();

            let block = Block::new(header, body);
            let vec = block.encode_to_vec();
            file.write_all(&vec)?;
        }

        info!("TEST RLP GENERATED AT: {path:?}");
    }
    Ok(())
}

// Unused. Generates the program input for a batch of only one block.
pub async fn generate_program_input(
    genesis: Genesis,
    chain: Vec<Block>,
    block_number: usize,
) -> Result<ProgramInput, ProverInputError> {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let block = chain
        .get(block_number)
        .ok_or(ProverInputError::InvalidBlockNumber(block_number))?
        .clone();

    // create store
    let store = Store::new("memory", EngineType::InMemory)?;
    rt.block_on(store.add_initial_state(genesis))?;
    // create blockchain
    let blockchain = Blockchain::default_with_store(store.clone());
    for block in chain {
        rt.block_on(blockchain.add_block(&block))?;
    }

    let parent_hash = block.header.parent_hash;
    let parent_block_header = store
        .get_block_header_by_hash(block.header.parent_hash)?
        .ok_or(ProverInputError::InvalidParentBlock(parent_hash))?;
    let elasticity_multiplier = ELASTICITY_MULTIPLIER;
    let blocks = vec![block];
    let db = to_prover_db(&store, &blocks).await?;

    Ok(ProgramInput {
        db,
        blocks,
        parent_block_header,
        elasticity_multiplier,
    })
}

// From cmd/ethrex/decode.rs
fn _chain_file(file: File) -> Result<Vec<Block>, Box<dyn std::error::Error>> {
    let mut chain_rlp_reader = BufReader::new(file);
    let mut buf = vec![];
    chain_rlp_reader.read_to_end(&mut buf)?;
    let mut blocks = Vec::new();
    while !buf.is_empty() {
        let (item, rest) = Block::decode_unfinished(&buf)?;
        blocks.push(item);
        buf = rest.to_vec();
    }
    Ok(blocks)
}

// From cmd/ethrex/decode.rs
fn _genesis_file(file: File) -> Result<Genesis, serde_json::Error> {
    let genesis_reader = BufReader::new(file);
    serde_json::from_reader(genesis_reader)
}
