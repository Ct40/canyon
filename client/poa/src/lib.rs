// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Canyon.
//
// Copyright (c) 2021 Canyon Labs.
//
// Canyon is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published
// by the Free Software Foundation, either version 3 of the License,
// or (at your option) any later version.
//
// Canyon is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with Canyon. If not, see <http://www.gnu.org/licenses/>.

//! This crate implements the Proof of Access consensus.

mod chunk;

use std::marker::PhantomData;
use std::sync::Arc;

use chunk::ChunkProof;
use codec::{Decode, Encode};
use thiserror::Error;

use sp_blockchain::HeaderBackend;
use sp_runtime::{
    generic::BlockId,
    traits::{Block as BlockT, DigestItemFor, Extrinsic, Header as HeaderT, SaturatedConversion},
};

use sc_client_api::BlockBackend;

use cp_permastore::{DEFAULT_CHUNK_SIZE, POA_ENGINE_ID};

type TxProof = Vec<Vec<u8>>;

/// The maximum depth of attempting to generate a valid PoA.
///
/// TODO: make it configurable in Runtime?
pub const MAX_DEPTH: u16 = 100;

type Depth = u16;

type Randomness = Vec<u8>;

type BlockNumber = u64;

type DataIndex = u64;

type ExtrinsicIndex = usize;

#[derive(Error, Debug)]
pub enum Error<Block: BlockT> {
    #[error("blockchain error")]
    BlockchainError(#[from] sp_blockchain::Error),
    #[error("block {0} not found")]
    BlockNotFound(BlockId<Block>),
    #[error("block header {0} not found")]
    HeaderNotFound(BlockId<Block>),
    #[error("the chunk in recall tx not found")]
    InvalidChunk,
    #[error("failed to decode opaque weave size from digest, error: {0}")]
    DecodeWeaveSizeFailed(codec::Error),
    #[error("weave size not found in the header digests")]
    EmptyWeaveSize,
    #[error("the maximum allowed depth {0} reached")]
    MaxDepthReached(Depth),
    #[error("unknown poa error")]
    Unknown,
}

/// Type for proving the data access.
#[derive(Debug, Clone, Encode, Decode)]
pub struct Poa {
    ///
    pub depth: Depth,
    ///
    pub tx_path: TxProof,
    ///
    pub chunk: ChunkProof,
}

pub struct PoaBuilder<Block: BlockT, Client> {
    /// Substrate client.
    client: Arc<Client>,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> PoaBuilder<Block, Client>
where
    Block: BlockT + 'static,
    Client: BlockBackend<Block> + HeaderBackend<Block> + 'static,
{
    /// Constructs a valid `Poa`.
    pub fn build(self, chain_head: Block::Header) -> Result<Poa, Error<Block>> {
        let weave_size = extract_weave_size::<Block>(&chain_head)?;

        for depth in 1..=MAX_DEPTH {
            let recall_byte = calculate_challenge_byte(chain_head.encode(), weave_size, depth);

            // TODO: Find the recall block.
            let recall_block_number = BlockNumber::default();

            let recall_block_id = BlockId::Number(recall_block_number.saturated_into());

            let (header, extrinsics) = self
                .client
                .block(&recall_block_id)?
                .ok_or_else(|| Error::BlockNotFound(recall_block_id))?
                .block
                .deconstruct();

            let recall_parent_block_id = BlockId::Hash(*header.parent_hash());
            let recall_parent_header = self
                .client
                .header(recall_parent_block_id)?
                .ok_or_else(|| Error::HeaderNotFound(recall_parent_block_id))?;

            let weave_base = extract_weave_size::<Block>(&recall_parent_header)?;

            let mut sized_extrinsics = Vec::with_capacity(extrinsics.len());

            let mut acc = 0u64;
            for (index, extrinsic) in extrinsics.iter().enumerate() {
                let tx_size = extrinsic.data_size();
                if tx_size > 0 {
                    sized_extrinsics.push((index, weave_base + acc + tx_size));
                    acc += tx_size;
                }
            }

            // No data store transactions in this block.
            if sized_extrinsics.is_empty() {
                continue;
            }

            let (recall_extrinsic_index, recall_tx_data_base) =
                find_recall_tx(recall_byte, &sized_extrinsics);

            // Continue if the recall tx has been forgotten as the forgot
            // txs can not participate in the consensus.
            if todo!("recall_tx has been forgotten via runtime api") {
                continue;
            }

            let tx_data: Option<Vec<u8>> =
                todo!("Fetch recall_tx data from DB given the block and extrinsic index");

            if let Some(tx_data) = tx_data {
                let chunk_ids = chunk::as_chunk_ids(tx_data);

                let chunk_offset = recall_byte - recall_tx_data_base;
                let recall_chunk_index = chunk_offset / DEFAULT_CHUNK_SIZE;

                let recall_chunk_id = chunk_ids[recall_chunk_index as usize];

                // Find the chunk

                // Construct PoA proof.

                // If find one solution, return directly.
            }
        }

        Err(Error::MaxDepthReached(MAX_DEPTH))
    }
}

fn extract_weave_size<Block: BlockT>(header: &Block::Header) -> Result<DataIndex, Error<Block>> {
    let opaque_weave_size = header.digest().logs.iter().find_map(|log| {
        if let DigestItemFor::<Block>::Consensus(POA_ENGINE_ID, opaque_data) = log {
            Some(opaque_data)
        } else {
            None
        }
    });

    match opaque_weave_size {
        Some(weave_size) => {
            Decode::decode(&mut weave_size.as_slice()).map_err(Error::DecodeWeaveSizeFailed)
        }
        None => Err(Error::EmptyWeaveSize),
    }
}

pub struct OpaquePoA(Vec<u8>);

/// Type for representing the block number and
/// the associated weave size of that block.
pub struct BlockWeave {
    /// Block number
    pub number: BlockNumber,
    /// Size of entire weave including the tx data of block `number`.
    pub weave_size: DataIndex,
}

/// The full list of BlockWeave in the block history.
///
/// NOTE: If a block has no data stored, it will be excluded from this list.
pub struct GlobalBlockIndex(Vec<(BlockNumber, DataIndex)>);

impl GlobalBlockIndex {
    pub fn find_callenge_block(&self, recall_byte: DataIndex) -> BlockNumber {
        binary_search(recall_byte, &self.0).0
    }
}

fn binary_search<T: Copy>(
    recall_byte: DataIndex,
    ordered_list: &[(T, DataIndex)],
) -> (T, DataIndex) {
    match ordered_list.binary_search_by_key(&recall_byte, |&(_, weave_size)| weave_size) {
        Ok(i) => ordered_list[i],
        Err(i) => ordered_list[i],
    }
}

fn find_recall_tx(
    recall_byte: DataIndex,
    sized_extrinsics: &[(ExtrinsicIndex, DataIndex)],
) -> (ExtrinsicIndex, DataIndex) {
    binary_search(recall_byte, sized_extrinsics)
}

/// Applies the hashing on `seed` for `n` times
fn multihash(seed: Randomness, n: Depth) -> [u8; 32] {
    assert!(n > 0);
    let mut r = sp_io::hashing::blake2_256(&seed);
    for _ in 1..n {
        r = sp_io::hashing::blake2_256(&r);
    }
    r
}

fn make_bytes(h: [u8; 32]) -> [u8; 8] {
    let mut res = [0u8; 8];
    res.copy_from_slice(&h);
    res
}

/// Returns the position of recall byte in the entire weave.
fn calculate_challenge_byte(seed: Randomness, weave_size: DataIndex, depth: Depth) -> DataIndex {
    DataIndex::from_le_bytes(make_bytes(multihash(seed, depth))) % weave_size
}

/// Returns the block number of recall block.
fn find_callenge_block(
    recall_byte: DataIndex,
    global_block_index: GlobalBlockIndex,
) -> BlockNumber {
    global_block_index.find_callenge_block(recall_byte)
}

fn generate_poa<Block, Client>(recall_byte: DataIndex, depth: Depth) -> Option<OpaquePoA>
where
    Block: BlockT + 'static,
    Client: BlockBackend<Block> + HeaderBackend<Block> + 'static,
{
    // TODO: load the global index from DB
    // use aux store?
    let global_index = GlobalBlockIndex(vec![(0, 10), (3, 15), (5, 20), (6, 30), (7, 32)]);

    let challenge_block_number = global_index.find_callenge_block(recall_byte);

    // let challenge_block_header = client.

    // let challenge_header =

    todo!()
}

/// Finds locally avaiable data to generate a PoA.
pub fn generate<Block, Client>(
    seed: Randomness,
    weave_size: DataIndex,
    depth: Depth,
) -> Option<OpaquePoA>
where
    Block: BlockT + 'static,
    Client: BlockBackend<Block> + HeaderBackend<Block> + 'static,
{
    if depth > MAX_DEPTH {
        return None;
    }

    let recall_byte = calculate_challenge_byte(seed.clone(), weave_size, depth);

    match generate_poa::<Block, Client>(recall_byte, depth) {
        Some(poa) => Some(poa),
        None => generate::<Block, Client>(seed, weave_size, depth + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_challenge_block() {
        let global_index = GlobalBlockIndex(vec![(0, 10), (3, 15), (5, 20), (6, 30), (7, 32)]);

        assert_eq!(0, global_index.find_callenge_block(5));
        assert_eq!(3, global_index.find_callenge_block(15));
        assert_eq!(5, global_index.find_callenge_block(16));
        assert_eq!(6, global_index.find_callenge_block(29));
        assert_eq!(7, global_index.find_callenge_block(31));
    }
}