//! Benchmarking setup for pallet-quantum-compute-mempool.

use super::*;

#[allow(unused)]
use crate::Pallet as QuantumComputeMempool;
use alloc::{vec, vec::Vec};
use frame_benchmarking::v2::*;
use frame_support::{
    pallet_prelude::ConstU32,
    traits::{Currency, Get, Hooks, ReservableCurrency},
    BoundedVec,
};
use frame_system::RawOrigin;
use sp_runtime::traits::{Hash as _, One, SaturatedConversion, Saturating, Zero};

fn bounded<T, S>(items: Vec<T>) -> BoundedVec<T, S>
where
    S: frame_support::traits::Get<u32>,
{
    items
        .try_into()
        .ok()
        .expect("benchmark input fits within bounds")
}

fn benchmark_reward<T: Config>() -> BalanceOf<T> {
    T::MinReward::get()
}

fn benchmark_funding<T: Config>() -> BalanceOf<T> {
    benchmark_reward::<T>().saturating_mul(10u32.saturated_into())
}

fn fund_account<T: Config>(who: &T::AccountId, amount: BalanceOf<T>) {
    let _ = T::Currency::make_free_balance_be(who, amount);
}

fn sample_spec<T: Config>() -> (
    BoundedVec<u8, ConstU32<128>>,
    types::Formulation,
    Option<T::Hash>,
    Option<T::Hash>,
) {
    (
        bounded(b"max-cut".to_vec()),
        types::Formulation::Ising,
        None,
        None,
    )
}

fn sample_spec_id<T: Config>(
    name: &BoundedVec<u8, ConstU32<128>>,
    formulation: types::Formulation,
    validation_program: Option<T::Hash>,
    transform_program: Option<T::Hash>,
) -> T::Hash {
    T::Hashing::hash_of(&(
        name.clone(),
        formulation,
        validation_program,
        transform_program,
    ))
}

fn sample_params<T: Config>() -> IsingParamsOf<T> {
    types::IsingParams {
        nodes: bounded(vec![0, 1]),
        edges: bounded(vec![(0, 1)]),
        h_values: bounded(vec![0, 0]),
        j_values: bounded(vec![-1_000]),
        max_energy_milli: None,
        min_diversity_milli: None,
        min_solutions: None,
    }
}

fn sample_solution<T: Config>() -> SolutionsOf<T> {
    bounded(vec![bounded(vec![1, 1])])
}

/// A deterministic xorshift64 (W1-010's property-test generator).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn shuffle<X>(&mut self, items: &mut [X]) {
        for index in (1..items.len()).rev() {
            items.swap(index, self.below(index as u64 + 1) as usize);
        }
    }
}

/// The worst topology a proposer can choose for `TopologyIndex::new` (A13):
/// `node_count` scattered u32 ids in shuffled order, and `edge_count`
/// distinct edges -- a shuffled spanning path, then random pairs -- in
/// shuffled order. Dense ids `0..n` with ring edges cost ~1.42-1.57x less
/// natively, all of it in the index's id map. Past the `n(n-1)/2` distinct
/// pairs the edges repeat (every pair, shuffled, cycled). Seeded from the
/// components, so a benchmark point is reproducible.
fn scattered_topology(node_count: u32, edge_count: u32) -> (Vec<u32>, Vec<(u32, u32)>) {
    let mut rng =
        Rng(0x5EED_A13A_0000_0000 ^ (u64::from(node_count) << 32) ^ u64::from(edge_count) | 1);
    let mut ids = alloc::collections::BTreeSet::new();
    while ids.len() < node_count as usize {
        ids.insert((rng.next() >> 16) as u32);
    }
    let mut nodes: Vec<u32> = ids.into_iter().collect();
    rng.shuffle(&mut nodes);

    let n = u64::from(node_count);
    let pairs = n * (n - 1) / 2;
    let wanted = u64::from(edge_count);
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(edge_count as usize);
    if wanted >= pairs {
        // Every distinct pair, shuffled, then cycled to `edge_count`.
        let mut all = Vec::with_capacity(pairs as usize);
        for a in 0..nodes.len() {
            for b in a + 1..nodes.len() {
                all.push((nodes[a], nodes[b]));
            }
        }
        rng.shuffle(&mut all);
        edges.extend(all.iter().cycle().take(edge_count as usize).copied());
    } else {
        let mut seen = alloc::collections::BTreeSet::new();
        for pair in nodes.windows(2) {
            if edges.len() == edge_count as usize {
                break;
            }
            seen.insert((pair[0].min(pair[1]), pair[0].max(pair[1])));
            edges.push((pair[0], pair[1]));
        }
        while edges.len() < edge_count as usize {
            let a = nodes[rng.below(n) as usize];
            let b = nodes[rng.below(n) as usize];
            if a != b && seen.insert((a.min(b), a.max(b))) {
                edges.push((a, b));
            }
        }
        rng.shuffle(&mut edges);
    }
    (nodes, edges)
}

fn params_with_dimensions<T: Config>(
    node_count: u32,
    edge_count: u32,
    min_solutions: Option<u32>,
) -> IsingParamsOf<T> {
    assert!(node_count >= 2);
    let (nodes, edges) = scattered_topology(node_count, edge_count);

    types::IsingParams {
        nodes: bounded(nodes),
        edges: bounded(edges),
        // A small non-zero field on every node: with fewer edges than nodes, a
        // node outside every edge and with no field would be isolated, and
        // rows differing only there are one answer (W1-004 keys by the W1-012
        // canonical form). Non-zero fields keep `distinct_rows` pairwise
        // distinct answers, the dedupe's worst case, for every `e`. 1 milli,
        // so a single field placed on spin 0 can still dominate the energy
        // (`setup_solution_replacement`).
        h_values: bounded(vec![1; node_count as usize]),
        j_values: bounded(vec![-1_000; edge_count as usize]),
        max_energy_milli: None,
        min_diversity_milli: None,
        min_solutions,
    }
}

/// `solution_count` rows of `node_count` spins that are pairwise distinct
/// answers (W1-004) whenever enough spins exist: spin 0 is `first_spin` in every
/// row, so no row is a global flip of another, and spin `i >= 1` is negated when
/// bit `i - 1` of the row index is set. The sets for `first_spin = 1` and `-1`
/// are exact negations of each other, row by row.
fn distinct_rows<T: Config>(
    node_count: u32,
    solution_count: u32,
    first_spin: i8,
) -> SolutionsOf<T> {
    bounded(
        (0..solution_count)
            .map(|solution_index| distinct_row(node_count, solution_index, first_spin))
            .collect(),
    )
}

/// Row `index` of `distinct_rows`.
fn distinct_row<S: Get<u32>>(node_count: u32, index: u32, first_spin: i8) -> BoundedVec<i8, S> {
    bounded(
        (0..node_count)
            .map(|node_index| {
                let flipped =
                    node_index >= 1 && node_index <= 32 && (index >> (node_index - 1)) & 1 == 1;
                if flipped {
                    -first_spin
                } else {
                    first_spin
                }
            })
            .collect(),
    )
}

/// The worst case for the W1-004 dedupe: every row a distinct answer.
fn solutions_with_dimensions<T: Config>(node_count: u32, solution_count: u32) -> SolutionsOf<T> {
    distinct_rows::<T>(node_count, solution_count, 1)
}

/// A placeholder answer for pre-seeded rankings, payloads and the answer
/// index. It is not the hash of any row, so it cannot collide with a
/// submitted answer.
fn synthetic_answer(index: u32) -> sp_core::H256 {
    let mut bytes = [0xA5u8; 32];
    bytes[28..].copy_from_slice(&index.to_be_bytes());
    sp_core::H256(bytes)
}

/// Fill the order's answer index (W1-012) with every other solver's entry, so
/// the duplicate read, and on improvement the remove and insert, run against a
/// full per-order index.
fn seed_answer_index<T: Config>(order_id: u64) {
    for index in 0..types::MAX_SOLVERS_PER_OPEN_ORDER.saturating_sub(1) {
        OrderAnswers::<T>::insert(
            order_id,
            synthetic_answer(index),
            types::AnswerOwner {
                solver: account::<T::AccountId>("indexed-solver", index, 0),
                best_energy_milli: 0,
                effective_at: Zero::zero(),
                seq: index,
            },
        );
    }
}

/// One row: the `index`-th row of `distinct_rows(node_count, _, 1)`, so
/// different indices are different answers (W1-012) on a connected topology.
fn nth_distinct_solution<T: Config>(node_count: u32, index: u32) -> SolutionsOf<T> {
    bounded(vec![distinct_row(node_count, index, 1)])
}

/// How many distinct answers `distinct_rows` can hold: `node_count - 1` free
/// spins give at most `2^(node_count - 1)` flip classes.
fn achievable_distinct(node_count: u32, solution_count: u32) -> u32 {
    let free = node_count.saturating_sub(1);
    if free >= 32 {
        solution_count
    } else {
        solution_count.min(1u32 << free)
    }
}

fn register_spec_for<T: Config>(builder: &T::AccountId) -> T::Hash {
    let (name, formulation, validation_program, transform_program) = sample_spec::<T>();
    let spec_id = sample_spec_id::<T>(&name, formulation, validation_program, transform_program);
    assert!(QuantumComputeMempool::<T>::register_job_spec(
        RawOrigin::Root.into(),
        builder.clone(),
        name,
        formulation,
        validation_program,
        transform_program,
    )
    .is_ok());
    spec_id
}

fn register_solver_for<T: Config>(solver: &T::AccountId) {
    fund_account::<T>(
        solver,
        T::Currency::minimum_balance().saturating_mul(2u32.into()),
    );
    assert!(QuantumComputeMempool::<T>::register_solver(
        RawOrigin::Signed(solver.clone()).into(),
        types::MinerType::Cpu,
    )
    .is_ok());
}

fn propose_open_order_for<T: Config>(
    proposer: &T::AccountId,
    reward: BalanceOf<T>,
    resolution: types::RewardResolution,
    deadline_blocks: BlockNumberOf<T>,
    block_wait: BlockNumberOf<T>,
    delivery: types::ResultDelivery,
) -> u64 {
    fund_account::<T>(proposer, reward.saturating_mul(10u32.saturated_into()));
    let spec_id = register_spec_for::<T>(proposer);
    let order_id = NextOrderId::<T>::get();
    assert!(QuantumComputeMempool::<T>::propose_job(
        RawOrigin::Signed(proposer.clone()).into(),
        spec_id,
        sample_params::<T>(),
        reward,
        types::JobMode::Open,
        resolution,
        deadline_blocks,
        block_wait,
        delivery,
    )
    .is_ok());
    order_id
}

fn propose_order_for<T: Config>(
    proposer: &T::AccountId,
    params: IsingParamsOf<T>,
    reward: BalanceOf<T>,
    mode: JobModeOf<T>,
    resolution: types::RewardResolution,
    delivery: types::ResultDelivery,
) -> u64 {
    fund_account::<T>(proposer, reward.saturating_mul(10u32.saturated_into()));
    let spec_id = register_spec_for::<T>(proposer);
    let order_id = NextOrderId::<T>::get();
    assert!(QuantumComputeMempool::<T>::propose_job(
        RawOrigin::Signed(proposer.clone()).into(),
        spec_id,
        params,
        reward,
        mode,
        resolution,
        10u32.into(),
        5u32.into(),
        delivery,
    )
    .is_ok());
    order_id
}

// The bounded closure scan must see a full index with the closing id last.
fn fill_proposer_index<T: Config>(proposer: &T::AccountId, order_id: u64) {
    let mut ids: Vec<u64> = (1..T::MaxOrdersPerProposer::get())
        .map(|i| order_id + u64::from(i))
        .collect();
    ids.push(order_id);
    ProposerOrders::<T>::insert(proposer, bounded::<_, T::MaxOrdersPerProposer>(ids));
}

/// A full `ProposerOrders` index for `proposer` that contains `order_id`; the
/// fillers count down from `u64::MAX`, so they never name a real order. A slot
/// release then scans the whole bound.
fn full_index_containing<T: Config>(proposer: &T::AccountId, order_id: u64) {
    let mut ids = vec![order_id];
    let mut filler = u64::MAX;
    while ids.len() < T::MaxOrdersPerProposer::get() as usize {
        ids.push(filler);
        filler = filler.saturating_sub(1);
    }
    ProposerOrders::<T>::insert(proposer, bounded::<_, T::MaxOrdersPerProposer>(ids));
}

// Shared maximal settlement setup; lifecycle variants are measured separately.
fn setup_claim_reward<T: Config>() -> (T::AccountId, T::AccountId, Vec<T::AccountId>, u64) {
    let caller: T::AccountId = whitelisted_caller();
    let proposer: T::AccountId = account("proposer", 0, 0);
    fund_account::<T>(&caller, benchmark_funding::<T>());
    let winner_count = types::MAX_REWARD_WINNERS;
    // Keep every weighted payout above the existential deposit, so all
    // 32 transfers and solver-counter writes execute even in the mock.
    let payout_floor = benchmark_reward::<T>().max(T::Currency::minimum_balance());
    let reward = payout_floor.saturating_mul((winner_count * winner_count).saturated_into());
    let winners: Vec<T::AccountId> = (0..winner_count)
        .map(|index| account("winner", index, 0))
        .collect();
    for winner in &winners {
        register_solver_for::<T>(winner);
    }
    let order_id = propose_order_for::<T>(
        &proposer,
        params_with_dimensions::<T>(T::MaxNodes::get(), T::MaxEdges::get(), None),
        reward,
        types::JobMode::Bid {
            miners: Some(bounded(
                winners
                    .iter()
                    .take(T::MaxBidMiners::get() as usize)
                    .cloned()
                    .collect(),
            )),
            miner_types: None,
        },
        types::RewardResolution::TopNWeighted { n: winner_count },
        types::ResultDelivery::CallbackWithPoll {
            endpoint: bounded(vec![b'x'; 256]),
        },
    );
    let ranked: TopSolversOf<T> = bounded(
        winners
            .iter()
            .enumerate()
            .map(|(index, winner)| types::RankedSolver {
                solver: winner.clone(),
                energy_milli: -i64::from(winner_count) + index as i64,
                answer: synthetic_answer(index as u32),
                effective_at: Zero::zero(),
                seq: index as u32,
            })
            .collect(),
    );
    OrderTopSolvers::<T>::insert(order_id, ranked);
    JobOrders::<T>::mutate(order_id, |maybe_order| {
        let order = maybe_order.as_mut().expect("benchmark order exists");
        order.solution_count = winner_count;
        order.first_solution_at = Some(order.created_at);
        assert_eq!(order.status, types::OrderStatus::Opened);
    });

    // Mirror the first-solution reschedule after seeding the maximal ranking.
    let created_at = JobOrders::<T>::get(order_id).unwrap().created_at;
    OrdersExpiringAt::<T>::remove(created_at.saturating_add(10u32.into()), order_id);
    OrdersExpiringAt::<T>::insert(created_at.saturating_add(5u32.into()), order_id, ());
    // Expiry/result publication is hook work, outside the measured settlement.
    // Advance every block: the hook intentionally bounds empty-bucket scanning.
    for offset in 1u32..=5 {
        let now = created_at.saturating_add(offset.into());
        frame_system::Pallet::<T>::set_block_number(now);
        QuantumComputeMempool::<T>::on_initialize(now);
    }
    assert_eq!(
        JobOrders::<T>::get(order_id).unwrap().status,
        types::OrderStatus::Expired
    );
    assert!(!OpenOrders::<T>::contains_key(order_id));
    assert!(OrderResults::<T>::contains_key(order_id));
    assert!(!Solvers::<T>::contains_key(&caller));

    fill_proposer_index::<T>(&proposer, order_id);
    (caller, proposer, winners, order_id)
}

fn verify_claim_reward<T: Config>(order_id: u64, winners: &[T::AccountId]) {
    let winner_count = types::MAX_REWARD_WINNERS;
    let order = JobOrders::<T>::get(order_id).expect("order exists");
    assert_eq!(order.status, types::OrderStatus::Closed);
    // W1-006: settlement empties the order's escrow account.
    assert!(
        T::Currency::total_balance(&QuantumComputeMempool::<T>::escrow_account(order_id)).is_zero()
    );
    assert!(!ProposerOrders::<T>::get(&order.proposer).contains(&order_id));
    assert!(!OpenOrders::<T>::contains_key(order_id));
    for winner in winners {
        assert!(Solvers::<T>::get(winner).unwrap().rewards_earned > 0u32.into());
    }
    assert_eq!(
        OrderResults::<T>::get(order_id)
            .expect("poll result is stored")
            .winners
            .len(),
        winner_count as usize
    );
}

// Seed a real accepted maximal payload and a full ranking outside measurement.
fn setup_solution_replacement<T: Config>(n: u32, e: u32, s: u32) -> (T::AccountId, u64, i64) {
    let proposer: T::AccountId = whitelisted_caller();
    let solver: T::AccountId = account("solver", 0, 0);
    register_solver_for::<T>(&solver);
    let mut params = params_with_dimensions::<T>(n, e, Some(achievable_distinct(n, s)));
    // A dominant field on spin 0 makes every first-spin-minus row strictly
    // worse than its negation (the first-spin-plus row) at every dimension,
    // without relying on a fabricated stored score. A row's negation changes
    // its energy by twice the sum of h_i * s_i; the other n - 1 fields are 1
    // milli each, so |h_0| > 2n outweighs them for every row.
    params.h_values[0] = -(2 * i32::try_from(n).expect("n fits i32") + 1_000);
    let order_id = propose_order_for::<T>(
        &proposer,
        params,
        benchmark_reward::<T>(),
        types::JobMode::Open,
        types::RewardResolution::TopNEqual {
            n: types::MAX_REWARD_WINNERS,
        },
        types::ResultDelivery::OnChainOnly,
    );
    let payee = types::WinnerPayee::V1 {
        chain_id: 20033,
        address: sp_core::H160::repeat_byte(1),
    };
    assert!(QuantumComputeMempool::<T>::submit_solution_with_payee(
        RawOrigin::Signed(solver.clone()).into(),
        order_id,
        distinct_rows::<T>(n, s, -1),
        payee,
    )
    .is_ok());
    let old_energy = OrderSolutions::<T>::get(order_id, &solver)
        .unwrap()
        .best_energy_milli;
    let ranked: TopSolversOf<T> = bounded(
        (0..types::MAX_REWARD_WINNERS)
            .map(|index| types::RankedSolver {
                solver: account("ranked-solver", index, 0),
                energy_milli: old_energy + i64::from(index),
                answer: synthetic_answer(index),
                effective_at: Zero::zero(),
                seq: index,
            })
            .collect(),
    );
    OrderTopSolvers::<T>::insert(order_id, ranked);
    seed_answer_index::<T>(order_id);

    (solver, order_id, old_energy)
}

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn register_solver() {
        let caller: T::AccountId = whitelisted_caller();
        fund_account::<T>(
            &caller,
            T::Currency::minimum_balance().saturating_mul(2u32.into()),
        );

        #[extrinsic_call]
        QuantumComputeMempool::register_solver(
            RawOrigin::Signed(caller.clone()),
            types::MinerType::Cpu,
        );

        assert!(Solvers::<T>::contains_key(&caller));
        assert_eq!(
            T::Currency::reserved_balance(&caller),
            T::Currency::minimum_balance()
        );
    }

    #[benchmark]
    fn deregister_solver() {
        let caller: T::AccountId = whitelisted_caller();
        register_solver_for::<T>(&caller);

        #[extrinsic_call]
        QuantumComputeMempool::deregister_solver(RawOrigin::Signed(caller.clone()));

        assert!(!Solvers::<T>::contains_key(&caller));
        assert_eq!(T::Currency::reserved_balance(&caller), 0u32.into());
        assert_eq!(
            T::Currency::free_balance(&caller),
            T::Currency::minimum_balance().saturating_mul(2u32.into())
        );
    }

    #[benchmark]
    fn register_job_spec() {
        let caller: T::AccountId = whitelisted_caller();
        let (name, formulation, validation_program, transform_program) = sample_spec::<T>();
        let spec_id =
            sample_spec_id::<T>(&name, formulation, validation_program, transform_program);

        #[extrinsic_call]
        QuantumComputeMempool::register_job_spec(
            RawOrigin::Root,
            caller.clone(),
            name,
            formulation,
            validation_program,
            transform_program,
        );

        assert!(JobSpecs::<T>::contains_key(spec_id));
    }

    #[benchmark]
    fn propose_job(
        n: Linear<2, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        b: Linear<1, { T::MaxBidMiners::get() }>,
        t: Linear<0, { MinerTypesOf::bound() as u32 }>,
    ) {
        let caller: T::AccountId = whitelisted_caller();
        fund_account::<T>(&caller, benchmark_funding::<T>());
        let spec_id = register_spec_for::<T>(&caller);
        let reward = benchmark_reward::<T>();
        let miners = bounded((0..b).map(|index| account("bid-miner", index, 0)).collect());
        let miner_types = if t == 0 {
            None
        } else {
            Some(bounded(vec![types::MinerType::Cpu; t as usize]))
        };

        #[extrinsic_call]
        QuantumComputeMempool::propose_job(
            RawOrigin::Signed(caller.clone()),
            spec_id,
            params_with_dimensions::<T>(n, e, None),
            reward,
            types::JobMode::Bid {
                miners: Some(miners),
                miner_types,
            },
            types::RewardResolution::SingleBest,
            10u32.into(),
            5u32.into(),
            types::ResultDelivery::OnChainOnly,
        );

        assert!(JobOrders::<T>::contains_key(0));
        // W1-006: the reward moved into the order's escrow account.
        assert_eq!(
            T::Currency::free_balance(&QuantumComputeMempool::<T>::escrow_account(0)),
            JobOrders::<T>::get(0).expect("order stored").reward
        );
    }

    #[benchmark]
    fn submit_solution(
        n: Linear<2, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        s: Linear<1, { T::MaxSolutions::get() }>,
    ) {
        let proposer: T::AccountId = whitelisted_caller();
        let solver: T::AccountId = account("solver", 0, 0);
        register_solver_for::<T>(&solver);
        // Only distinct answers count (W1-004): require every row that n
        // spins can make distinct, as the quantum-pow benchmark does.
        let min_solutions = achievable_distinct(n, s);
        let order_id = propose_order_for::<T>(
            &proposer,
            params_with_dimensions::<T>(n, e, Some(min_solutions)),
            benchmark_reward::<T>(),
            types::JobMode::Open,
            types::RewardResolution::TopNEqual {
                n: types::MAX_REWARD_WINNERS,
            },
            types::ResultDelivery::OnChainOnly,
        );
        let ranked: TopSolversOf<T> = bounded(
            (0..types::MAX_REWARD_WINNERS)
                .map(|index| types::RankedSolver {
                    solver: account("ranked-solver", index, 0),
                    energy_milli: 1_000_000_000_i64.saturating_add(i64::from(index)),
                    answer: synthetic_answer(index),
                    effective_at: Zero::zero(),
                    seq: index,
                })
                .collect(),
        );
        OrderTopSolvers::<T>::insert(order_id, ranked);
        seed_answer_index::<T>(order_id);

        #[extrinsic_call]
        QuantumComputeMempool::submit_solution(
            RawOrigin::Signed(solver.clone()),
            order_id,
            solutions_with_dimensions::<T>(n, s),
        );

        assert!(OrderSolutions::<T>::contains_key(order_id, solver));
    }

    #[benchmark]
    fn propose_job_with_payee_requirement(
        n: Linear<2, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        b: Linear<1, { T::MaxBidMiners::get() }>,
        t: Linear<0, { MinerTypesOf::bound() as u32 }>,
    ) {
        let caller: T::AccountId = whitelisted_caller();
        fund_account::<T>(&caller, benchmark_funding::<T>());
        let spec_id = register_spec_for::<T>(&caller);
        let reward = benchmark_reward::<T>();
        let miners = bounded((0..b).map(|index| account("bid-miner", index, 0)).collect());
        let miner_types = if t == 0 {
            None
        } else {
            Some(bounded(vec![types::MinerType::Cpu; t as usize]))
        };

        #[extrinsic_call]
        QuantumComputeMempool::propose_job_with_payee_requirement(
            RawOrigin::Signed(caller.clone()),
            spec_id,
            params_with_dimensions::<T>(n, e, None),
            reward,
            types::JobMode::Bid {
                miners: Some(miners),
                miner_types,
            },
            types::RewardResolution::SingleBest,
            10u32.into(),
            5u32.into(),
            types::ResultDelivery::OnChainOnly,
            20033,
        );

        assert!(JobOrders::<T>::contains_key(0));
        assert_eq!(OrderPayeeRequirements::<T>::get(0), Some(20033));
    }

    #[benchmark]
    fn submit_solution_with_payee(
        n: Linear<2, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        s: Linear<1, { T::MaxSolutions::get() }>,
    ) {
        let proposer: T::AccountId = whitelisted_caller();
        let solver: T::AccountId = account("solver", 0, 0);
        register_solver_for::<T>(&solver);
        // Only distinct answers count (W1-004): require every row that n
        // spins can make distinct, as the quantum-pow benchmark does.
        let min_solutions = achievable_distinct(n, s);
        let order_id = propose_order_for::<T>(
            &proposer,
            params_with_dimensions::<T>(n, e, Some(min_solutions)),
            benchmark_reward::<T>(),
            types::JobMode::Open,
            types::RewardResolution::TopNEqual {
                n: types::MAX_REWARD_WINNERS,
            },
            types::ResultDelivery::OnChainOnly,
        );
        let ranked: TopSolversOf<T> = bounded(
            (0..types::MAX_REWARD_WINNERS)
                .map(|index| types::RankedSolver {
                    solver: account("ranked-solver", index, 0),
                    energy_milli: 1_000_000_000_i64.saturating_add(i64::from(index)),
                    answer: synthetic_answer(index),
                    effective_at: Zero::zero(),
                    seq: index,
                })
                .collect(),
        );
        OrderTopSolvers::<T>::insert(order_id, ranked);
        seed_answer_index::<T>(order_id);

        OrderPayeeRequirements::<T>::insert(order_id, 20033);
        let payee = types::WinnerPayee::V1 {
            chain_id: 20033,
            address: sp_core::H160::repeat_byte(1),
        };

        #[extrinsic_call]
        QuantumComputeMempool::submit_solution_with_payee(
            RawOrigin::Signed(solver.clone()),
            order_id,
            solutions_with_dimensions::<T>(n, s),
            payee,
        );

        assert!(OrderSolutions::<T>::contains_key(order_id, &solver));
        assert_eq!(OrderPayees::<T>::get(order_id, &solver), Some(payee));
    }

    // Reading/replacing the largest previously accepted payload is a separate
    // timing case. Compare this with first-submission rescheduling before
    // generating the public submission weights.
    #[benchmark(extra)]
    fn submit_solution_replacement(
        n: Linear<2, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        s: Linear<1, { T::MaxSolutions::get() }>,
    ) {
        let (solver, order_id, old_energy) = setup_solution_replacement::<T>(n, e, s);
        OrderPayeeRequirements::<T>::insert(order_id, 20033);
        let payee = types::WinnerPayee::V1 {
            chain_id: 20033,
            address: sp_core::H160::repeat_byte(2),
        };

        #[extrinsic_call]
        QuantumComputeMempool::submit_solution_with_payee(
            RawOrigin::Signed(solver.clone()),
            order_id,
            distinct_rows::<T>(n, s, 1),
            payee,
        );

        assert!(
            OrderSolutions::<T>::get(order_id, &solver)
                .unwrap()
                .best_energy_milli
                < old_energy
        );
        assert_eq!(OrderPayees::<T>::get(order_id, &solver), Some(payee));
    }

    // Legacy call clears an optional destination while replacing the full payload.
    #[benchmark(extra)]
    fn submit_solution_clear_payee(
        n: Linear<2, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        s: Linear<1, { T::MaxSolutions::get() }>,
    ) {
        let (solver, order_id, old_energy) = setup_solution_replacement::<T>(n, e, s);
        assert!(OrderPayees::<T>::contains_key(order_id, &solver));

        #[extrinsic_call]
        QuantumComputeMempool::submit_solution(
            RawOrigin::Signed(solver.clone()),
            order_id,
            distinct_rows::<T>(n, s, 1),
        );

        assert!(
            OrderSolutions::<T>::get(order_id, &solver)
                .unwrap()
                .best_energy_milli
                < old_energy
        );
        assert!(!OrderPayees::<T>::contains_key(order_id, &solver));
        assert!(frame_system::Pallet::<T>::events()
            .iter()
            .any(|record| record.event
                == Event::<T>::SolutionPayeeSet {
                    order_id,
                    solver: solver.clone(),
                    payee: None
                }
                .into()));
    }

    #[benchmark]
    fn claim_reward() {
        let (caller, proposer, winners, order_id) = setup_claim_reward::<T>();

        #[extrinsic_call]
        QuantumComputeMempool::claim_reward(RawOrigin::Signed(caller), order_id);

        verify_claim_reward::<T>(order_id, &winners);
        assert!(T::Currency::free_balance(&proposer) >= T::Currency::minimum_balance());
    }

    // The public claim weight takes the maximum of both measured lifecycle cases.
    // The name is kept so the WeightInfo trait does not change; since W1-006
    // the second case is a proposer that reaped its own account after posting.
    #[benchmark(extra)]
    fn claim_reward_dust() {
        let (caller, proposer, winners, order_id) = setup_claim_reward::<T>();
        // With the reward in the order's escrow the proposer holds no reserve,
        // so it cannot sit at dust: below ED its account is reaped. Reap it
        // outright. Settlement must still pay every winner from the escrow,
        // burn nothing, and leave the proposer either reaped or holding at
        // least ED -- never dust.
        //
        // What this case does NOT bound: a remainder transfer. The weighted
        // split adds its rounding to rank 0, so the shares sum to the whole
        // reward, and every share is >= ED for ANY constants: the reward is
        // 32 * 32 * floor with floor = max(MinReward, ED) (setup_claim_reward),
        // so the smallest share, reward / 528, is 1024 / 528 * floor >= ED.
        // So the remainder is 0 here and neither the transfer back to the
        // proposer nor the rank-0 fallback runs.
        fund_account::<T>(&proposer, Zero::zero());
        assert!(!frame_system::Pallet::<T>::account_exists(&proposer));
        let issuance = T::Currency::total_issuance();

        #[extrinsic_call]
        QuantumComputeMempool::claim_reward(RawOrigin::Signed(caller), order_id);

        verify_claim_reward::<T>(order_id, &winners);
        let left = T::Currency::total_balance(&proposer);
        assert!(left.is_zero() || left >= T::Currency::minimum_balance());
        assert_eq!(T::Currency::total_issuance(), issuance);
    }

    #[benchmark]
    fn reclaim_order() {
        let proposer: T::AccountId = whitelisted_caller();
        let order_id = propose_open_order_for::<T>(
            &proposer,
            benchmark_reward::<T>(),
            types::RewardResolution::SingleBest,
            // B5a2: block_wait at MinBlockWait (2 in the runtimes). Unsolved,
            // so it closes on the hard deadline at C + 1 as before.
            1u32.into(),
            T::MinBlockWait::get(),
            types::ResultDelivery::OnChainOnly,
        );
        JobOrders::<T>::mutate(order_id, |maybe_order| {
            let order = maybe_order.as_mut().expect("benchmark order exists");
            order.ising_params =
                params_with_dimensions::<T>(T::MaxNodes::get(), T::MaxEdges::get(), None);
        });

        fill_proposer_index::<T>(&proposer, order_id);
        let now = frame_system::Pallet::<T>::block_number().saturating_add(1u32.into());
        frame_system::Pallet::<T>::set_block_number(now);
        // W3-006: finalisation now closes an unsolved order itself, so
        // `reclaim_order` settles only one that finalisation left `Expired` (a
        // failed refund). Put the order in that state directly. B4c's lazy path
        // (an `Opened` order past expiry) supersedes this.
        JobOrders::<T>::mutate(order_id, |maybe_order| {
            maybe_order.as_mut().expect("benchmark order exists").status =
                types::OrderStatus::Expired;
        });
        OpenOrders::<T>::remove(order_id);

        #[extrinsic_call]
        QuantumComputeMempool::reclaim_order(RawOrigin::Signed(proposer.clone()), order_id);

        let order = JobOrders::<T>::get(order_id).expect("order exists");
        assert_eq!(order.status, types::OrderStatus::Closed);
        assert!(!ProposerOrders::<T>::get(&proposer).contains(&order_id));
        // W1-006: reclaim empties the order's escrow account.
        assert!(
            T::Currency::total_balance(&QuantumComputeMempool::<T>::escrow_account(order_id))
                .is_zero()
        );
    }

    #[benchmark]
    fn purge_result() {
        let proposer: T::AccountId = whitelisted_caller();
        let solver: T::AccountId = account("solver", 0, 0);
        let cleaner: T::AccountId = account("cleaner", 0, 0);
        register_solver_for::<T>(&solver);
        fund_account::<T>(&cleaner, benchmark_funding::<T>());
        let spec_id = register_spec_for::<T>(&proposer);
        fund_account::<T>(&proposer, benchmark_funding::<T>());
        let order_id = NextOrderId::<T>::get();
        assert!(QuantumComputeMempool::<T>::propose_job(
            RawOrigin::Signed(proposer.clone()).into(),
            spec_id,
            sample_params::<T>(),
            benchmark_reward::<T>(),
            types::JobMode::Bid {
                miners: Some(bounded(vec![solver.clone()])),
                miner_types: None,
            },
            types::RewardResolution::SingleBest,
            // B5a2: block_wait at MinBlockWait (2 in the runtimes) and at least
            // 1. Solved at C, it closes at C + 1 (min(C + 1, C + w)), where
            // `on_initialize(2)` finalises it as before.
            1u32.into(),
            T::MinBlockWait::get().max(One::one()),
            types::ResultDelivery::CallbackWithPoll {
                endpoint: bounded(b"https://solver.example/poll".to_vec()),
            },
        )
        .is_ok());
        assert!(QuantumComputeMempool::<T>::submit_solution(
            RawOrigin::Signed(solver.clone()).into(),
            order_id,
            sample_solution::<T>(),
        )
        .is_ok());
        frame_system::Pallet::<T>::set_block_number(2u32.into());
        QuantumComputeMempool::<T>::on_initialize(2u32.into());
        assert!(OrderResults::<T>::get(order_id).is_some());
        frame_system::Pallet::<T>::set_block_number(
            T::ResultTtlBlocks::get().saturating_add(2u32.into()),
        );

        #[extrinsic_call]
        QuantumComputeMempool::purge_result(RawOrigin::Signed(cleaner), order_id);

        assert!(OrderResults::<T>::get(order_id).is_none());
    }

    /// Measures the worst-case order: maximum nodes and edges, 32 weighted
    /// winners with full-size solution vectors, and `CallbackWithPoll` with
    /// a maximum-length endpoint, including the `OrderResults` write.
    #[benchmark]
    fn finalise_expired_orders(k: Linear<0, { types::MAX_FINALISED_ORDERS_PER_BLOCK }>) {
        let winner_count = types::MAX_REWARD_WINNERS;
        let reward = benchmark_reward::<T>().saturating_mul(winner_count.saturated_into());
        let spec_id = register_spec_for::<T>(&account("builder", 0, 0));
        let winners: Vec<T::AccountId> = (0..winner_count)
            .map(|index| account("winner", index, 0))
            .collect();
        for winner in &winners {
            register_solver_for::<T>(winner);
        }
        let bid_miner = winners.first().expect("winner set is nonempty").clone();

        let mut order_ids = Vec::new();
        for i in 0..k {
            // One funded, non-whitelisted proposer per order (cold account and
            // index), each with a full `ProposerOrders` index.
            let proposer: T::AccountId = account("proposer", i, 0);
            fund_account::<T>(
                &proposer,
                reward
                    .saturating_mul(2u32.saturated_into())
                    .saturating_add(benchmark_funding::<T>()),
            );
            let order_id = NextOrderId::<T>::get();
            assert!(QuantumComputeMempool::<T>::propose_job(
                RawOrigin::Signed(proposer.clone()).into(),
                spec_id,
                params_with_dimensions::<T>(T::MaxNodes::get(), T::MaxEdges::get(), None),
                reward,
                types::JobMode::Bid {
                    miners: Some(bounded(vec![bid_miner.clone()])),
                    miner_types: None,
                },
                types::RewardResolution::TopNWeighted { n: winner_count },
                // B5a2: block_wait at MinBlockWait (2 in the runtimes) and at
                // least 1: every winner submits at C, so the order must stay
                // open after the first. It closes at C + 1 (min(C + 1, C + w)).
                1u32.into(),
                T::MinBlockWait::get().max(One::one()),
                types::ResultDelivery::CallbackWithPoll {
                    endpoint: bounded(vec![b'x'; <ConstU32<256> as Get<u32>>::get() as usize]),
                },
            )
            .is_ok());
            for (winner_index, winner) in (0u32..).zip(&winners) {
                // Rotate the fixture's bid allow-list because the mock's
                // MaxBidMiners is smaller than the 32-winner settlement bound.
                JobOrders::<T>::mutate(order_id, |maybe_order| {
                    let order = maybe_order.as_mut().expect("benchmark order exists");
                    order.mode = types::JobMode::Bid {
                        miners: Some(bounded(vec![winner.clone()])),
                        miner_types: None,
                    };
                });
                assert!(QuantumComputeMempool::<T>::submit_solution(
                    RawOrigin::Signed(winner.clone()).into(),
                    order_id,
                    // One answer per winner (W1-012).
                    nth_distinct_solution::<T>(T::MaxNodes::get(), winner_index),
                )
                .is_ok());
            }
            // Pruning must measure maximum encoded payloads for every slot,
            // not only the one-solution setup needed to establish rankings.
            for winner in &winners {
                OrderSolutions::<T>::mutate(order_id, winner, |entry| {
                    let entry = entry.as_mut().expect("accepted solver payload");
                    entry.solutions =
                        solutions_with_dimensions::<T>(T::MaxNodes::get(), T::MaxSolutions::get());
                    entry.num_valid = T::MaxSolutions::get();
                });
            }
            full_index_containing::<T>(&proposer, order_id);
            order_ids.push(order_id);
        }

        let expiry_block = frame_system::Pallet::<T>::block_number().saturating_add(1u32.into());
        frame_system::Pallet::<T>::set_block_number(expiry_block);
        // Measure one full bucket, not the empty buckets before it.
        NextExpiryBucket::<T>::put(expiry_block);
        OldestExpiryBucket::<T>::put(expiry_block);

        #[block]
        {
            QuantumComputeMempool::<T>::on_initialize(expiry_block);
        }

        for order_id in order_ids {
            let order = JobOrders::<T>::get(order_id).expect("order exists");
            assert_eq!(order.status, types::OrderStatus::Expired);
            assert!(OrderSolutions::<T>::iter_key_prefix(order_id)
                .next()
                .is_none());
            assert!(OrderAnswers::<T>::iter_key_prefix(order_id)
                .next()
                .is_none());
            assert_eq!(
                OrderResults::<T>::get(order_id)
                    .expect("poll result is stored")
                    .winners
                    .len(),
                winner_count as usize
            );
        }
    }

    /// Maximum closed record and all sparse auxiliary prefixes, including
    /// defensive cleanup of 32 maximum-size orphaned solution payloads.
    #[benchmark]
    fn prune_closed_orders(k: Linear<0, { types::MAX_PRUNED_ORDERS_PER_BLOCK }>) {
        let (caller, _, winners, template_id) = setup_claim_reward::<T>();
        assert!(QuantumComputeMempool::<T>::claim_reward(
            RawOrigin::Signed(caller).into(),
            template_id,
        )
        .is_ok());
        let order = JobOrders::<T>::take(template_id).expect("closed template");
        let result = OrderResults::<T>::take(template_id).expect("poll template");
        let ranking = OrderTopSolvers::<T>::take(template_id);
        let now = frame_system::Pallet::<T>::block_number()
            .saturating_add(T::OrderRetentionBlocks::get());
        OrdersPrunableAt::<T>::remove(now, template_id);
        NextPruneBucket::<T>::put(now);
        let first = winners.first().expect("nonempty winners");
        for id in 0..u64::from(k) {
            JobOrders::<T>::insert(id, &order);
            OrderResults::<T>::insert(id, &result);
            OrderTopSolvers::<T>::insert(id, &ranking);
            OrderFrontRunner::<T>::insert(
                id,
                types::FrontRunner {
                    solver: first.clone(),
                    energy_milli: -1,
                    effective_at: Zero::zero(),
                    seq: 0,
                },
            );
            OrderPayeeRequirements::<T>::insert(id, 20033);
            OpenOrders::<T>::insert(id, ());
            OrdersPrunableAt::<T>::insert(now, id, ());
            for (winner_index, winner) in (0u32..).zip(&winners) {
                // Defensive cleanup also covers a full answer index (W1-012).
                OrderAnswers::<T>::insert(
                    id,
                    synthetic_answer(winner_index),
                    types::AnswerOwner {
                        solver: winner.clone(),
                        best_energy_milli: -1,
                        effective_at: order.created_at,
                        seq: winner_index,
                    },
                );
                OrderPayees::<T>::insert(
                    id,
                    winner,
                    types::WinnerPayee::V1 {
                        chain_id: 20033,
                        address: sp_core::H160::repeat_byte(255),
                    },
                );
                OrderSolutions::<T>::insert(
                    id,
                    winner,
                    types::JobSolution {
                        solver: winner.clone(),
                        solver_type: types::MinerType::Cpu,
                        solutions: solutions_with_dimensions::<T>(
                            T::MaxNodes::get(),
                            T::MaxSolutions::get(),
                        ),
                        best_energy_milli: -1,
                        diversity_milli: 0,
                        num_valid: T::MaxSolutions::get(),
                        submitted_at: order.created_at,
                        answer: synthetic_answer(winner_index),
                        effective_at: order.created_at,
                        seq: winner_index,
                    },
                );
            }
        }
        frame_system::Pallet::<T>::set_block_number(now);
        #[block]
        {
            QuantumComputeMempool::<T>::prune_closed_orders(now);
        }
        for id in 0..u64::from(k) {
            assert!(!JobOrders::<T>::contains_key(id));
            assert!(!OrderResults::<T>::contains_key(id));
            assert!(!OrderTopSolvers::<T>::contains_key(id));
            assert!(!OrderFrontRunner::<T>::contains_key(id));
            assert!(!OrderPayeeRequirements::<T>::contains_key(id));
            assert!(!OpenOrders::<T>::contains_key(id));
            assert!(OrderPayees::<T>::iter_key_prefix(id).next().is_none());
            assert!(OrderSolutions::<T>::iter_key_prefix(id).next().is_none());
            assert!(OrderAnswers::<T>::iter_key_prefix(id).next().is_none());
        }
        assert!(!NextPruneBucket::<T>::exists());
    }

    /// W3-006 (design §B4 v2 item 2): `finalise_order`'s auto-close of `k`
    /// unsolved, maximum-size orders -- the escrow refund, the `JobOrders`
    /// re-write, the slot release from a full `ProposerOrders` index and the
    /// pruning schedule. `on_initialize` charges it on top of
    /// `finalise_expired_orders`, which measures solved orders.
    #[benchmark]
    fn auto_close_unsolved(k: Linear<0, { types::MAX_FINALISED_ORDERS_PER_BLOCK }>) {
        let spec_id = register_spec_for::<T>(&account("builder", 0, 0));
        let mut proposers = Vec::new();
        let mut order_ids = Vec::new();
        for i in 0..k {
            // One funded, non-whitelisted proposer per order, so each refund
            // and slot release touches a cold account and index (B4d).
            let proposer: T::AccountId = account("proposer", i, 0);
            // Reward plus a maximum-size order's storage deposit.
            fund_account::<T>(
                &proposer,
                benchmark_funding::<T>().saturating_mul(2u32.saturated_into()),
            );
            let order_id = NextOrderId::<T>::get();
            assert!(QuantumComputeMempool::<T>::propose_job(
                RawOrigin::Signed(proposer.clone()).into(),
                spec_id,
                params_with_dimensions::<T>(T::MaxNodes::get(), T::MaxEdges::get(), None),
                benchmark_reward::<T>(),
                types::JobMode::Open,
                types::RewardResolution::SingleBest,
                // B5a2: block_wait at MinBlockWait (2 in the runtimes). Unsolved,
                // so it closes on the hard deadline at C + 1 as before.
                1u32.into(),
                T::MinBlockWait::get(),
                types::ResultDelivery::OnChainOnly,
            )
            .is_ok());
            full_index_containing::<T>(&proposer, order_id);
            proposers.push(proposer);
            order_ids.push(order_id);
        }
        // The state `finalise_order` leaves before the auto-close.
        let orders: Vec<_> = order_ids
            .iter()
            .map(|&id| {
                OpenOrders::<T>::remove(id);
                JobOrders::<T>::mutate(id, |maybe_order| {
                    let order = maybe_order.as_mut().expect("benchmark order exists");
                    order.status = types::OrderStatus::Expired;
                    (id, order.clone())
                })
            })
            .collect();

        #[block]
        {
            for (id, order) in orders {
                QuantumComputeMempool::<T>::auto_close_unsolved(id, order);
            }
        }

        for (&id, proposer) in order_ids.iter().zip(&proposers) {
            assert_eq!(
                JobOrders::<T>::get(id).expect("order exists").status,
                types::OrderStatus::Closed
            );
            assert!(
                T::Currency::total_balance(&QuantumComputeMempool::<T>::escrow_account(id))
                    .is_zero()
            );
            let index = ProposerOrders::<T>::get(proposer);
            assert!(!index.contains(&id));
            assert_eq!(index.len() as u32, T::MaxOrdersPerProposer::get() - 1);
        }
    }

    /// B5a2 (R8 on B5a): the mock's `MinBlockWait` is 0, so the suite above
    /// would not notice a setup proposing below the runtimes' minimum of 2
    /// (`benchmark-preflight` and `benchmark-weights` run at 2). Run every
    /// registered benchmark again at 2, the `extra` ones included (B5a3,
    /// kimi's R8 on B5a2).
    #[cfg(test)]
    #[test]
    fn every_benchmark_runs_at_the_runtime_min_block_wait() {
        use crate::mock::{new_test_ext, MinBlockWait, Test};
        use frame_benchmarking::Benchmarking;
        let names: Vec<Vec<u8>> = <Pallet<Test> as Benchmarking>::benchmarks(true)
            .into_iter()
            .map(|benchmark| benchmark.name)
            .collect();
        assert!(names.len() > 10, "the benchmarks are listed");
        // The `extra` benchmarks, so the inventory cannot silently shrink.
        for extra in [
            &b"submit_solution_replacement"[..],
            b"submit_solution_clear_payee",
            b"claim_reward_dust",
        ] {
            assert!(
                names.iter().any(|name| name.as_slice() == extra),
                "{} is run",
                String::from_utf8_lossy(extra)
            );
        }
        let mut failed = Vec::new();
        for name in names {
            new_test_ext().execute_with(|| {
                MinBlockWait::set(2);
                if let Err(error) = Pallet::<Test>::test_bench_by_name(&name) {
                    failed.push((String::from_utf8_lossy(&name).into_owned(), error));
                }
            });
        }
        assert!(failed.is_empty(), "at MinBlockWait 2: {failed:?}");
    }

    /// The mock's ED of 1 makes `ED - 1` zero, so a "dust" assertion holds
    /// vacuously there. Run the dust benchmark with an ED above one, as the
    /// runtime has (benchmark-preflight runs it at the production ED).
    #[cfg(test)]
    #[test]
    fn claim_reward_dust_runs_with_an_existential_deposit_above_one() {
        use crate::mock::{new_test_ext, ExistentialDeposit, Test};
        new_test_ext().execute_with(|| {
            ExistentialDeposit::set(10);
            assert_eq!(<Test as Config>::Currency::minimum_balance(), 10);
            Pallet::<Test>::test_benchmark_claim_reward_dust().unwrap();
        });
    }

    impl_benchmark_test_suite!(
        QuantumComputeMempool,
        crate::mock::new_test_ext(),
        crate::mock::Test
    );
}

#[cfg(test)]
mod topology_tests {
    use super::*;
    use alloc::collections::BTreeSet;

    /// `scattered_topology` (A13): `n` distinct ids, not in increasing
    /// order (checked where a sorted shuffle is negligible); exactly `e` edges over those ids without self-loops,
    /// `min(e, n(n-1)/2)` of them distinct; the same output for the same
    /// components.
    #[test]
    fn the_scattered_topology_has_the_requested_shape() {
        for (n, e) in [
            (2u32, 1u32),
            (2, 5),
            (3, 3),
            (3, 7),
            (40, 300),
            (500, 4_000),
        ] {
            let (nodes, edges) = scattered_topology(n, e);
            let ids: BTreeSet<u32> = nodes.iter().copied().collect();
            assert_eq!((nodes.len(), ids.len()), (n as usize, n as usize));
            if n >= 40 {
                assert!(nodes.windows(2).any(|pair| pair[0] > pair[1]), "shuffled");
            }
            assert_eq!(edges.len(), e as usize);
            assert!(edges
                .iter()
                .all(|&(a, b)| a != b && ids.contains(&a) && ids.contains(&b)));
            let distinct: BTreeSet<(u32, u32)> =
                edges.iter().map(|&(a, b)| (a.min(b), a.max(b))).collect();
            let pairs = u64::from(n) * u64::from(n - 1) / 2;
            assert_eq!(
                distinct.len() as u64,
                u64::from(e).min(pairs),
                "{n} nodes, {e} edges"
            );
            assert_eq!(scattered_topology(n, e), (nodes, edges));
        }
    }
}
