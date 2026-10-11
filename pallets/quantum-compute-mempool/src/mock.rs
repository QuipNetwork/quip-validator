use crate as pallet_quantum_compute_mempool;
use frame_support::{
    derive_impl, parameter_types,
    traits::{ConstU128, ConstU32, ConstU64},
};
use sp_runtime::BuildStorage;
use std::cell::RefCell;

type Block = frame_system::mocking::MockBlock<Test>;
type Balance = u128;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MockVmState {
    pub validate_programs_calls: u32,
    pub transform_solutions_calls: u32,
    pub validate_result_calls: u32,
    pub fail_validate_programs: bool,
    pub fail_transform_solutions: bool,
    pub fail_validate_result: bool,
    pub transformed_solutions: Option<Vec<Vec<i8>>>,
    pub last_result_winner_count: Option<usize>,
}

std::thread_local! {
    static MOCK_VM_STATE: RefCell<MockVmState> = RefCell::new(MockVmState::default());
}

pub struct TestVm;

impl<AccountId, Hash> pallet_quantum_compute_mempool::xqvm::QuantumVm<AccountId, Balance, Hash>
    for TestVm
{
    fn validate_programs(
        _validation_program: Option<&Hash>,
        _transform_program: Option<&Hash>,
    ) -> sp_runtime::DispatchResult {
        MOCK_VM_STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.validate_programs_calls = state.validate_programs_calls.saturating_add(1);
            if state.fail_validate_programs {
                return Err(sp_runtime::DispatchError::Other("vm-validate-programs"));
            }
            Ok(())
        })
    }

    fn transform_solutions(
        _spec_id: &Hash,
        _validation_program: Option<&Hash>,
        _transform_program: Option<&Hash>,
        _solver: &AccountId,
        solutions: Vec<Vec<i8>>,
    ) -> Result<Vec<Vec<i8>>, sp_runtime::DispatchError> {
        MOCK_VM_STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.transform_solutions_calls = state.transform_solutions_calls.saturating_add(1);
            if state.fail_transform_solutions {
                return Err(sp_runtime::DispatchError::Other("vm-transform-solutions"));
            }
            Ok(state.transformed_solutions.clone().unwrap_or(solutions))
        })
    }

    fn validate_result(
        _spec_id: &Hash,
        _validation_program: Option<&Hash>,
        _transform_program: Option<&Hash>,
        winners: &[crate::types::WinnerSummary<AccountId, Balance>],
    ) -> sp_runtime::DispatchResult {
        MOCK_VM_STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.validate_result_calls = state.validate_result_calls.saturating_add(1);
            state.last_result_winner_count = Some(winners.len());
            if state.fail_validate_result {
                return Err(sp_runtime::DispatchError::Other("vm-validate-result"));
            }
            Ok(())
        })
    }
}

pub fn reset_vm_state() {
    MOCK_VM_STATE.with(|state| *state.borrow_mut() = MockVmState::default());
}

pub fn mock_vm_state() -> MockVmState {
    MOCK_VM_STATE.with(|state| state.borrow().clone())
}

pub fn set_vm_transformed_solutions(solutions: Vec<Vec<i8>>) {
    MOCK_VM_STATE.with(|state| state.borrow_mut().transformed_solutions = Some(solutions));
}

pub fn set_vm_fail_validate_programs(should_fail: bool) {
    MOCK_VM_STATE.with(|state| state.borrow_mut().fail_validate_programs = should_fail);
}

pub fn set_vm_fail_transform_solutions(should_fail: bool) {
    MOCK_VM_STATE.with(|state| state.borrow_mut().fail_transform_solutions = should_fail);
}

pub fn set_vm_fail_validate_result(should_fail: bool) {
    MOCK_VM_STATE.with(|state| state.borrow_mut().fail_validate_result = should_fail);
}

#[frame_support::runtime]
mod runtime {
    #[runtime::runtime]
    #[runtime::derive(
        RuntimeCall,
        RuntimeEvent,
        RuntimeError,
        RuntimeOrigin,
        RuntimeFreezeReason,
        RuntimeHoldReason,
        RuntimeSlashReason,
        RuntimeLockId,
        RuntimeTask,
        RuntimeViewFunction
    )]
    pub struct Test;

    #[runtime::pallet_index(0)]
    pub type System = frame_system::Pallet<Test>;

    #[runtime::pallet_index(1)]
    pub type Balances = pallet_balances::Pallet<Test>;

    #[runtime::pallet_index(2)]
    pub type QuantumComputeMempool = pallet_quantum_compute_mempool::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountData = pallet_balances::AccountData<Balance>;
}

impl pallet_balances::Config for Test {
    type MaxLocks = ConstU32<50>;
    type MaxReserves = ConstU32<8>;
    type ReserveIdentifier = [u8; 8];
    type Balance = Balance;
    type RuntimeEvent = RuntimeEvent;
    type DustRemoval = ();
    type ExistentialDeposit = ConstU128<1>;
    type AccountStore = System;
    type WeightInfo = ();
    type FreezeIdentifier = RuntimeFreezeReason;
    type MaxFreezes = ConstU32<0>;
    type RuntimeHoldReason = RuntimeHoldReason;
    type RuntimeFreezeReason = RuntimeFreezeReason;
    type DoneSlashHandler = ();
}

parameter_types! {
    pub const MaxNodes: u32 = 16;
    pub const MaxEdges: u32 = 32;
    pub const MaxSolutions: u32 = 8;
    pub const MaxBidMiners: u32 = 4;
    pub const MaxOrdersPerProposer: u32 = 8;
    pub const MaxDeadlineBlocks: u64 = 100;
    pub const MaxBlockWait: u64 = 20;
    pub const MinReward: Balance = 10;
    pub const ResultTtlBlocks: u64 = 10;
}

impl pallet_quantum_compute_mempool::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type MaxNodes = MaxNodes;
    type MaxEdges = MaxEdges;
    type MaxSolutions = MaxSolutions;
    type MaxBidMiners = MaxBidMiners;
    type MaxOrdersPerProposer = MaxOrdersPerProposer;
    type MaxDeadlineBlocks = MaxDeadlineBlocks;
    type MaxBlockWait = MaxBlockWait;
    type MinReward = MinReward;
    type ResultTtlBlocks = ResultTtlBlocks;
    type DefaultIsingSpecId = pallet_quantum_compute_mempool::CanonicalDefaultIsingSpecId<Test>;
    type DefaultJobSpecBuilder = ConstU64<1>;
    type VM = TestVm;
    type WeightInfo = ();
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    new_test_ext_with_default_ising_spec(false)
}

pub fn new_test_ext_with_default_ising_spec(
    seed_default_ising_spec: bool,
) -> sp_io::TestExternalities {
    let mut storage = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Test> {
        balances: vec![
            (1, 1_000_000),
            (2, 1_000_000),
            (3, 1_000_000),
            (4, 1_000_000),
        ],
        dev_accounts: None,
    }
    .assimilate_storage(&mut storage)
    .unwrap();

    pallet_quantum_compute_mempool::GenesisConfig::<Test> {
        default_ising_spec_builder: seed_default_ising_spec.then_some(1),
    }
    .assimilate_storage(&mut storage)
    .unwrap();

    let mut ext: sp_io::TestExternalities = storage.into();
    ext.execute_with(|| {
        reset_vm_state();
        System::set_block_number(1);
    });
    ext
}

/// Advance to block `n`, running this pallet's `on_initialize` for every
/// block after the current one. Mirrors `Executive`: hooks run before any
/// extrinsic in the same block.
pub fn run_to_block(n: u64) {
    use frame_support::traits::Hooks;

    while System::block_number() < n {
        let next = System::block_number() + 1;
        System::set_block_number(next);
        QuantumComputeMempool::on_initialize(next);
    }
}
