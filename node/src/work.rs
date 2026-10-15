//! Work-weighted fork choice for concurrent topology mining.
//!
//! Each qblock's runtime deposits its work (`topology_weight × quality`,
//! summed over the winner and its uncles) as a `qpow` consensus digest.
//! Execution re-derives the digest, so an imported block's work is verified.
//!
//! - [`WorkBlockImport`] sits between BABE and GRANDPA's block import. It
//!   records each block's cumulative work in aux storage and makes the
//!   heaviest chain the best chain. BABE still decides who may author.
//! - [`HeaviestWorkChain`] is the `SelectChain` for BABE and GRANDPA. Its
//!   `finality_target` is the heaviest leaf descending from the vote base, so
//!   honest GRANDPA voters finalize the heaviest-work fork BABE produced.
//!
//! Ties: more work, then the higher block, then the lower hash. Never a
//! timestamp. A block without the digest (non-qblocks, pre-119) adds 0, so
//! with no work anywhere this is longest-chain.

use codec::{Decode, Encode};
use sc_client_api::{backend, AuxStore};
use sc_consensus::{
    BlockCheckParams, BlockImport, BlockImportParams, ForkChoiceStrategy, ImportResult,
};
use sp_blockchain::{Backend as _, HeaderBackend};
use sp_consensus::{Error as ConsensusError, SelectChain};
use sp_runtime::{
    generic::OpaqueDigestItemId,
    traits::{Block as BlockT, Header as HeaderT, NumberFor},
};
use std::{marker::PhantomData, sync::Arc};

/// Engine id of the runtime's work digest; must match
/// `pallet_quantum_pow::QPOW_ENGINE_ID`.
pub const QPOW_ENGINE_ID: [u8; 4] = *b"qpow";
const AUX_PREFIX: &[u8] = b"qpow_cumulative_work";

fn aux_key<H: Encode>(hash: &H) -> Vec<u8> {
    (AUX_PREFIX, hash).encode()
}

/// Work a header's own `qpow` digest declares; `0` without one.
pub fn block_work<H: HeaderT>(header: &H) -> u128 {
    header
        .digest()
        .convert_first(|item| item.try_to::<u128>(OpaqueDigestItemId::Consensus(&QPOW_ENGINE_ID)))
        .unwrap_or(0)
}

/// Cumulative work of `hash` as recorded at import; `0` if never recorded
/// (genesis, warp-synced history).
pub fn cumulative_work<A: AuxStore, H: Encode>(aux: &A, hash: &H) -> u128 {
    aux.get_aux(&aux_key(hash))
        .ok()
        .flatten()
        .and_then(|raw| u128::decode(&mut &raw[..]).ok())
        .unwrap_or(0)
}

/// Fork-choice key: more work, then the higher block, then the lower hash.
type WeightKey<H> = (
    u128,
    <H as HeaderT>::Number,
    std::cmp::Reverse<<H as HeaderT>::Hash>,
);

fn weight_key<H: HeaderT>(work: u128, header: &H) -> WeightKey<H> {
    (work, *header.number(), std::cmp::Reverse(header.hash()))
}

/// Block import that records cumulative work and picks the heaviest chain.
pub struct WorkBlockImport<I, C> {
    inner: I,
    client: Arc<C>,
}

impl<I: Clone, C> Clone for WorkBlockImport<I, C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            client: self.client.clone(),
        }
    }
}

impl<I, C> WorkBlockImport<I, C> {
    pub fn new(inner: I, client: Arc<C>) -> Self {
        Self { inner, client }
    }
}

#[async_trait::async_trait]
impl<B, I, C> BlockImport<B> for WorkBlockImport<I, C>
where
    B: BlockT,
    I: BlockImport<B> + Send + Sync,
    C: AuxStore + HeaderBackend<B> + Send + Sync,
{
    type Error = I::Error;

    async fn check_block(&self, block: BlockCheckParams<B>) -> Result<ImportResult, Self::Error> {
        self.inner.check_block(block).await
    }

    async fn import_block(
        &self,
        mut block: BlockImportParams<B>,
    ) -> Result<ImportResult, Self::Error> {
        let parent = *block.header.parent_hash();
        let work =
            cumulative_work(&*self.client, &parent).saturating_add(block_work(&block.header));
        block
            .auxiliary
            .push((aux_key(&block.post_hash()), Some(work.encode())));

        // Only override a fork choice BABE actually made; keep explicit
        // `LongestChain`/absent choices (e.g. state-sync imports) as they are.
        if matches!(block.fork_choice, Some(ForkChoiceStrategy::Custom(_))) {
            let info = self.client.info();
            let best_work = cumulative_work(&*self.client, &info.best_hash);
            let best_header = self.client.header(info.best_hash).ok().flatten();
            let is_best = match best_header {
                Some(best) => weight_key(work, &block.header) > weight_key(best_work, &best),
                None => true,
            };
            block.fork_choice = Some(ForkChoiceStrategy::Custom(is_best));
        }
        self.inner.import_block(block).await
    }
}

/// `SelectChain` that votes for, and authors on, the heaviest-work chain.
pub struct HeaviestWorkChain<BE, B> {
    backend: Arc<BE>,
    _phantom: PhantomData<B>,
}

impl<BE, B> Clone for HeaviestWorkChain<BE, B> {
    fn clone(&self) -> Self {
        Self {
            backend: self.backend.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<BE, B> HeaviestWorkChain<BE, B>
where
    BE: backend::Backend<B>,
    B: BlockT,
{
    pub fn new(backend: Arc<BE>) -> Self {
        Self {
            backend,
            _phantom: PhantomData,
        }
    }

    fn header(&self, hash: B::Hash) -> sp_blockchain::Result<B::Header> {
        self.backend
            .blockchain()
            .header(hash)?
            .ok_or_else(|| sp_blockchain::Error::MissingHeader(format!("{hash:?}")))
    }

    /// Heaviest leaf descending from `base`, walked back to `max_number`.
    fn heaviest_from(
        &self,
        base: B::Hash,
        max_number: Option<NumberFor<B>>,
    ) -> sp_blockchain::Result<B::Hash> {
        let base_header = self.header(base)?;
        let base_number = *base_header.number();
        if max_number.is_some_and(|max| max < base_number) {
            return Err(sp_blockchain::Error::Application(
                "finality target max number below the base".into(),
            ));
        }
        let mut best: Option<(WeightKey<B::Header>, B::Hash)> = None;
        for leaf in self.backend.blockchain().leaves()? {
            // Walk the leaf back to `max_number`, then to `base_number`, so we
            // both cap the target and check it descends from `base`.
            let mut head = self.header(leaf)?;
            if let Some(max) = max_number {
                while *head.number() > max {
                    head = self.header(*head.parent_hash())?;
                }
            }
            let target = head.clone();
            while *head.number() > base_number {
                head = self.header(*head.parent_hash())?;
            }
            if head.hash() != base {
                continue;
            }
            let key = weight_key(cumulative_work(&*self.backend, &target.hash()), &target);
            if best.as_ref().is_none_or(|(best_key, _)| key > *best_key) {
                best = Some((key, target.hash()));
            }
        }
        Ok(best.map(|(_, hash)| hash).unwrap_or(base))
    }
}

#[async_trait::async_trait]
impl<BE, B> SelectChain<B> for HeaviestWorkChain<BE, B>
where
    BE: backend::Backend<B>,
    B: BlockT,
{
    async fn leaves(&self) -> Result<Vec<B::Hash>, ConsensusError> {
        self.backend
            .blockchain()
            .leaves()
            .map_err(|e| ConsensusError::ChainLookup(e.to_string()))
    }

    /// The client's best block, which `WorkBlockImport` keeps at the heaviest
    /// chain.
    async fn best_chain(&self) -> Result<B::Header, ConsensusError> {
        let best = self.backend.blockchain().info().best_hash;
        self.header(best)
            .map_err(|e| ConsensusError::ChainLookup(e.to_string()))
    }

    async fn finality_target(
        &self,
        base_hash: B::Hash,
        maybe_max_number: Option<NumberFor<B>>,
    ) -> Result<B::Hash, ConsensusError> {
        self.heaviest_from(base_hash, maybe_max_number)
            .map_err(|e| ConsensusError::ChainLookup(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sp_runtime::{generic::DigestItem, testing::Header, Digest};

    fn header(number: u64, logs: Vec<DigestItem>) -> Header {
        Header::new(
            number,
            Default::default(),
            Default::default(),
            Default::default(),
            Digest { logs },
        )
    }

    #[test]
    fn block_work_reads_the_qpow_digest_and_defaults_to_zero() {
        let with = header(
            1,
            vec![DigestItem::Consensus(QPOW_ENGINE_ID, 42u128.encode())],
        );
        assert_eq!(block_work(&with), 42);
        assert_eq!(block_work(&header(1, vec![])), 0);
        // Another engine's digest is not work.
        let other = header(1, vec![DigestItem::Consensus(*b"FRNK", 42u128.encode())]);
        assert_eq!(block_work(&other), 0);
        // The id must match the runtime's.
        assert_eq!(QPOW_ENGINE_ID, pallet_quantum_pow::QPOW_ENGINE_ID);
    }

    #[test]
    fn weight_prefers_work_then_height_then_lower_hash() {
        let short_heavy = header(5, vec![]);
        let long_light = header(9, vec![]);
        assert!(weight_key(10, &short_heavy) > weight_key(9, &long_light));
        // Equal work: the longer chain (zero work everywhere = longest chain).
        assert!(weight_key(0, &long_light) > weight_key(0, &short_heavy));
        // Equal work and height: the lower hash, deterministic and not a
        // timestamp.
        let a = header(5, vec![DigestItem::Other(vec![1])]);
        let b = header(5, vec![DigestItem::Other(vec![2])]);
        let (lo, hi) = if a.hash() < b.hash() { (a, b) } else { (b, a) };
        assert!(weight_key(7, &lo) > weight_key(7, &hi));
    }
}
