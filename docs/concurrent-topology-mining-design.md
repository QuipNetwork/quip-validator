# Concurrent topology mining: uncles and work-weighted finality (design)

Status: draft for sign-off (qv-s30.13). Nothing here is implemented.

## Goals

- Any number of topologies are mineable at once.
- Every per-block and per-proof check is O(1) or O(n + e) of the proof's own
  topology. Nothing iterates the whitelist on chain.
- A block's work is its winning proof plus included uncles, each worth
  `topology_weight × proof_quality`.
- GRANDPA votes for the heaviest-work chain among the forks BABE produced.
  Once GRANDPA finalizes, that choice is final.

## Prior art

No proof-of-useful-work chain we found ships uncles, so the template is
Ethereum's proof-of-work ommer rules (Yellow Paper, Homestead to Paris):

| Rule | Ethereum PoW | Proposed here |
| --- | --- | --- |
| Uncles per block | at most 2 | at most `MaxUncles = 2` |
| Uncle age | parent within 6 generations | seeded by one of the last 6 qblock rounds |
| Uncle reward | `(8 - depth)/8 × R` | same, depth in qblock rounds, depth 0 = same block = 7/8 |
| Includer reward | `R/32` per uncle | `R/32` per uncle to the qblock winner |
| Duplicate inclusion | forbidden | forbidden (salt + miner + round key) |
| Uncles in fork choice | no, total difficulty counts canonical blocks only | yes (GHOST), as requested |

Lessons we carry over:

- Uncle rewards weaken the cost of selfish mining (Ritz and Zugenmaier,
  arXiv:1805.08832; arXiv:1904.13330). Counting uncles toward chain weight is
  the GHOST fix for that, and it is what Ethereum never shipped. We count them.
- Uncle Maker (CCS 2023) exploited timestamp-based difficulty and tie-breaks.
  Our tie-break never uses timestamps.

## Work

```text
topology_weight = expected_bound_milli(topology)        // E[Σ|h| + Σ|J|], derived
proof_work      = topology_weight × quality / QUALITY_SCALE   // u128
block_work      = winner_work + Σ uncle_work
```

`quality` is the existing `proof_quality` (depth below the instance bar over
the bar's distance to the instance optimum), already computed in
`submit_proof`. `topology_weight` is computed once at registration and stored
next to `TopologyDims`, so reading it is one 16-byte read.

## Uncles (runtime)

Two sources, both validated by the existing `submit_proof` path and both
priced at the existing `submit_proof` weight:

1. **Same-block runners-up.** Accepted proofs that lose `BlockBestProof` today
   are discarded. Keep the top `MaxUncles` by work in a bounded
   `BlockUncles` value. Depth 0.
2. **Stale proofs.** A proof seeded by one of the last 6 qblock round hashes
   (a ring buffer `RecentRoundSeeds`, 6 × 32 bytes) is accepted as an uncle
   candidate instead of failing `InvalidNonce`. Depth = rounds behind.

`on_finalize`:

- Pays the winner, each uncle `(8 - depth)/8 × R`, and the winner `R/32` per
  uncle.
- Writes uncles to a new `QBlockUncles` map (block → bounded list), not into
  `QBlock`, so the `QBlocks` layout does not change again.
- Records `(round_seed, miner, salt)` of each uncle in a 6-round dedup set so a
  stale proof is paid once.
- Deposits `DigestItem::Consensus(*b"qpow", block_work.encode())`. Executive
  checks that the header digest matches execution, so every importing node
  verifies it.

A block with no winner but with stale uncles still deposits their work.

## Fork choice (node)

- `WorkBlockImport` sits between BABE's block import and GRANDPA's. On import
  it reads the `qpow` digest, adds the parent's cumulative work from aux
  storage, stores the child's cumulative work, and sets
  `ForkChoiceStrategy::Custom(child_work > best_work)`. BABE still decides who
  may author; work decides the best chain.
- `HeaviestWorkChain` replaces `LongestChain` as the `SelectChain` for BABE and
  GRANDPA. `best_chain` is the leaf with the most cumulative work.
  `finality_target(base)` is the heaviest leaf descending from `base`, which is
  what the GRANDPA voter votes for. The default voting rules still apply.
- Tie-break: more work, then greater block number, then lower block hash.
  Never a timestamp.

Enforcement is by honest voters: GRANDPA cannot verify why a voter chose a
target, so this holds under the same two-thirds-honest assumption finality
already rests on. A block without the digest (pre-upgrade) counts 0 work.

## Automatable failover without the whitelist scan

The on-chain failover scan in `retire_tractable_default` is O(whitelist). With
an unbounded whitelist it is replaced by a resumable cursor: `on_initialize`
inspects at most `FailoverScanPerBlock` (e.g. 8) whitelist keys per block from
a stored cursor and promotes the first Heuristic one. Bounded per block,
automatic, and finishes within `whitelist / 8` blocks of the grace window
closing. `add_mineable_topology`'s "one standby" check is removed.

## Changes to existing behaviour

- `MineableTopologyConflict` is never raised (kept for index stability).
- Emission per qblock rises by up to `2 × 7/8 + 2/32 ≈ 1.81 × R`, paid from the
  same subnet pot.
- Miner-visible: stale proofs within 6 rounds are no longer refused.
- New storage: `TopologyWeight`, `BlockUncles`, `QBlockUncles`,
  `RecentRoundSeeds`, `PaidUncles`, `FailoverCursor`. New digest engine ID
  `qpow`. Node release required on every validator before the runtime upgrade.

## Questions for sign-off

1. **One difficulty dial across topologies.** Each topology's bar is in its own
   energy units. A shared dial needs a common unit. Proposal: store one
   position `p` in `[0, 1]` and map it onto each topology's own curve
   (`bar = max + p × (min − max)`); a win anywhere hardens `p`. Today each
   topology has its own `Difficulties` entry.
2. **Emission.** Uncles raise payout per qblock by up to 1.81×. Keep `R`
   fixed and let the pot absorb it, or scale `R` down so expected emission per
   block is unchanged?
3. **Includer reward.** Ethereum pays the block author. Here the BABE author
   is a validator and the qblock winner is a miner. Proposal: the qblock
   winner gets `R/32` per uncle, since uncles are included automatically.
4. **Weight discount.** Proposal: uncles count in full toward block work
   (GHOST) and are discounted only in reward. Alternatively discount weight by
   depth too.
