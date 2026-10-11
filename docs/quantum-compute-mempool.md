# Quantum Compute Mempool

## Canonical Plain Ising Job Spec

`QuantumComputeMempool` seeds a default plain Ising job spec at genesis so SDKs
can call `propose_job` without first registering a spec.

Canonical tuple:

```text
name = "plain-ising-v1"
formulation = Ising
validation_program = None
transform_program = None
```

The `spec_id` is the runtime hash of the SCALE-encoded tuple:

```text
(name, formulation, validation_program, transform_program)
```

Pinned `spec_id`:  
NOTE: This value is verified by CI (default_ising_spec_id_is_pinned and default_ising_spec_id_matches_pinned_hash);
if any tuple field changes, those tests will fail.

```text
0x8f46f3a31321d1d093314fc769c42cbe7a83d71a0b69e6571a0f68e2a04067f0
```

SDKs may hardcode this value or read the `DefaultIsingSpecId` pallet constant
from metadata. Additional job specs are team-controlled: `register_job_spec`
requires root origin and records an explicit builder account supplied by root.

## Coefficient and Solution Domains

`QuantumComputeMempool` stores plain Ising problems. The on-chain solution
domain is spin space:

```text
s_i in {-1, +1}
```

The pallet does not accept binary solution values such as `0` and `1` for
plain Ising jobs. If an SDK accepts a QUBO or binary-domain model from a user,
the SDK should transform it off-chain before calling `propose_job`, and should
map returned spin solutions back to the user's binary domain if needed:

```text
x_i = (s_i + 1) / 2
s_i = 2x_i - 1
```

Coefficients are fixed-point milli values. `h_values` and `j_values` are
`i32` values where `1000` represents `1.0`. For example:

```text
5.0   -> 5000
0.25  -> 250
-1.5  -> -1500
```

The same milli convention is used for `best_energy_milli`,
`max_energy_milli`, `diversity_milli`, and `min_diversity_milli`. Energy
values are signed (`i64`); diversity values are unsigned (`u32`).
`max_energy_milli` is an energy ceiling. `submit_solution` drops every vector
whose energy is above it. Keep any constant offset introduced by a
QUBO-to-Ising transform in the adapter, or adjust user-facing energy
thresholds before submitting the job.

## Accepted Solution Storage

Accepted solver submissions are stored in `OrderSolutions`, a `DoubleMap`
keyed first by `order_id` and second by solver account:

```rust
pub type OrderSolutions<T: Config> = StorageDoubleMap<
    _,
    Blake2_128Concat,
    u64,
    Blake2_128Concat,
    T::AccountId,
    JobSolutionOf<T>,
>;
```

Each stored `JobSolution` contains:

```text
solver
solver_type
solutions
best_energy_milli
diversity_milli
num_valid
submitted_at
```

Because this storage item is exposed through runtime metadata, SDKs can query
it through the standard Substrate storage RPC. No custom RPC is needed unless a
client cannot perform or decode metadata-backed `DoubleMap` prefix queries.

### Replacements and retained vectors (runtime 119 reset)

A solver's first valid submission is accepted. A replacement must have a
**strictly lower** validated minimum energy than that solver's accepted payload,
even when the solver has fallen outside the top-N ranking. Equal or worse
submissions fail with `SolutionNotImproved`; payload, ranking, counters, tie
priority and events remain unchanged. Better submissions still must pass every
energy, diversity and eligibility rule. Atomic dispatch updates the accepted
payload and ranking together. Existing ranking rules keep the incumbent on a
single-best tie and stable ordering in top-N ties.

The stored subset always starts with the first vector attaining the minimum
validated energy, after VM transformation and ceiling filtering. It then greedily
adds the vector with the greatest minimum symmetric Hamming distance to those
already selected, breaking ties by input order. Diversity thresholds apply to
this actual subset. `min_solutions = None` retains all valid vectors; zero or one
retains exactly the best vector; larger values require that many valid vectors.
Positive diversity with an explicit count below two remains invalid.

SDK/coordinator error handling: decode the pallet error by metadata name.
`SolutionNotImproved` is a terminal rejection for that unchanged submission:
stop retrying that payload, retain the accepted submission locally, and retry only
with a genuinely better solution while the order is open. A signed failed call
still consumes its nonce and fee. Reconcile inclusion/nonce before sending again.
A missed `SolutionAccepted` event is not permission to resubmit blindly; query
`OrderSolutions` at a finalised block. This repository contains API documentation,
not the external Python coordinator; its deployed regression (QUI-1676,
`leader_energy_matches_chain=True`) must run after deployment on fresh reset
orders and has not been claimed as a local validation.

### Query with substrate-interface

Install the Python client:

```bash
pip install substrate-interface
```

Connect to a node over websocket:

```python
from substrateinterface import SubstrateInterface

substrate = SubstrateInterface(url="ws://127.0.0.1:9944")
```

Read one solver's accepted submission for an order:

```python
order_id = 0
solver_ss58 = "5..."

solution = substrate.query(
    module="QuantumComputeMempool",
    storage_function="OrderSolutions",
    params=[order_id, solver_ss58],
)

if solution.value is not None:
    print(solution.value["solver"])
    print(solution.value["solutions"])
    print(solution.value["best_energy_milli"])
    print(solution.value["diversity_milli"])
```

Read all accepted submissions for one order by querying the `DoubleMap` with
only the first key:

```python
order_id = 0

rows = substrate.query_map(
    module="QuantumComputeMempool",
    storage_function="OrderSolutions",
    params=[order_id],
    page_size=100,
)

for solver_key, solution in rows:
    print("solver key:", solver_key.value)
    print("stored solver:", solution.value["solver"])
    print("solutions:", solution.value["solutions"])
    print("best energy:", solution.value["best_energy_milli"])
    print("diversity:", solution.value["diversity_milli"])
```

Use `block_hash=` on `query` or `query_map` when an adapter needs a historical
or finalized view instead of the current best block.

## Job Lifecycle Events

`QuantumComputeMempool` emits lifecycle events that SDKs can monitor through
`System.Events`. The most useful events for result retrieval are:

```text
JobProposed
SolutionAccepted
FirstSolutionReceived
BlockWaitStarted
FrontRunnerChanged
OrderExpired
OrderClosed
ResultReady
```

`SolutionAccepted` is emitted when a solver submission is accepted and written
to `OrderSolutions`. `ResultReady` is emitted at expiry for callback delivery
modes. Settlement also emits
`RewardClaimed` for each winner payout, and `ResultPurged` is emitted when a
stored poll payload is deleted after its TTL — `CallbackWithPoll` consumers
must fetch results before the purge.

`OrderExpired` is emitted by the pallet's `on_initialize` hook at the order's
effective expiry block, even when no extrinsic touches the order.
An expiry that falls in the current block (a zero `block_wait` after the first
solution, or a zero `deadline_blocks`) is scheduled for the next block,
because that block's hook has already run. The hook finalises at most 8 orders
per block (`MAX_FINALISED_ORDERS_PER_BLOCK`).
When more orders expire in one block, the rest carry over to the following
blocks in order, so an order can read `Opened` for several blocks after its
expiry under heavy load: the backlog drains eight orders per block, and the
hook also scans at most eight expiry buckets per block when it has fallen
behind. `submit_solution` rejects the order from the expiry block regardless.

At the same point, `Callback` and `CallbackWithPoll` orders that received at
least one accepted solution emit `ResultReady` with each winner's allocation,
and `CallbackWithPoll` orders get their `OrderResults` entry with `settled_at`
set to the block in which the hook finalises the order. That is the expiry
block unless the expiry fell in the current block or a backlog carried it
over. `claim_reward` only moves funds and emits `RewardClaimed` and
`OrderClosed`.

The effective expiry is:

```text
min(created_at + timing.deadline_blocks, first_solution_at + timing.block_wait)
```

Before the first solution arrives, `first_solution_at` is null and the expiry
is `created_at + timing.deadline_blocks`. `created_at` and `first_solution_at`
are top-level `JobOrder` fields. `deadline_blocks` and `block_wait` are nested
inside the order's `timing` struct.

`TopNWeighted { n }` splits the reward by rank. With `k` ranked solvers, rank
`r` (0 is best) has weight `k - r`, so two solvers split 2:1 and three split
3:2:1. Energy values only decide the order. Any rounding remainder goes to
rank 0. `TopNEqual { n }` splits evenly with the same remainder rule.

### Read events from a block with substrate-interface

```python
EVENTS = {
    "JobProposed",
    "SolutionAccepted",
    "FirstSolutionReceived",
    "BlockWaitStarted",
    "FrontRunnerChanged",
    "OrderExpired",
    "OrderClosed",
    "ResultReady",
}


def mempool_events_at(block_hash=None):
    for event in substrate.get_events(block_hash=block_hash):
        if (
            event.value["module_id"] == "QuantumComputeMempool"
            and event.value["event_id"] in EVENTS
        ):
            yield event


for event in mempool_events_at():
    print(event.value["event_id"], event.value["attributes"])
```

### Subscribe to event storage with substrate-interface

For live best-block monitoring, subscribe to `System.Events` and filter the
decoded event records:

```python
def event_name_from_storage_record(record):
    event = record.get("event", {})
    module = event.get("module_id") or event.get("module")
    name = event.get("event_id") or event.get("name")
    return module, name


def handle_events(events_obj, update_nr, subscription_id):
    if update_nr == 0:
        return None

    for record in events_obj.value:
        module, name = event_name_from_storage_record(record)
        if module == "QuantumComputeMempool" and name in EVENTS:
            print(name, record)

    return None


substrate.query(
    module="System",
    storage_function="Events",
    subscription_handler=handle_events,
)
```

For finalized-only processing, subscribe to finalized or imported block
headers in the adapter process, then call `get_events(block_hash=...)` for each
block hash before acting on the events.

## Outstanding proposer capacity (runtime 119 reset)

`ProposerOrders` enumerates non-Closed orders, including expired but unpaid orders.
The production limit is 32 outstanding orders, not 32 lifetime proposals. Successful
`claim_reward` or `reclaim_order` removes only that ID, atomically with closure;
historical `JobOrders`, solutions, results and payee evidence are retained.
Expiry alone does not free a slot. History requires events or historical order
queries rather than this bounded capacity index.

An impossible payout retains the slot and reserved funds. The lock/freeze owner
must resolve the restriction before a signed caller retries. An incomplete reserve
returns `RewardReserveMissing` and rolls back partial unreserving; restore the
missing reserve through an authorised recovery before retrying. No cleanup,
forced-close or solved-order refund mechanism is provided, and eventual closure
is not guaranteed. Runtime 119 starts with empty indexes at fresh genesis; no
pre-reset cleanup/import/migration is included.
