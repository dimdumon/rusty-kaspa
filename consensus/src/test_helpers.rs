use crate::constants::BLOCK_VERSION;
use consensus_core::{block::Block, header::Header};
use hashes::Hash;

pub fn header_from_precomputed_hash(hash: Hash, parents: Vec<Hash>) -> Header {
    Header {
        version: BLOCK_VERSION,
        hash,
        parents_by_level: vec![parents],
        hash_merkle_root: Default::default(),
        accepted_id_merkle_root: Default::default(),
        utxo_commitment: Default::default(),
        nonce: 0,
        timestamp: 0,
        daa_score: 0,
        bits: 0,
        blue_work: 0.into(),
        blue_score: 0,
        pruning_point: Default::default(),
    }
}

pub fn block_from_precomputed_hash(hash: Hash, parents: Vec<Hash>) -> Block {
    Block::from_header(header_from_precomputed_hash(hash, parents))
}
