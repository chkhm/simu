# Test Strategy

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `master` and on every pull
request (Ubuntu):

- **Rust job** — `cargo build`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test`, each exercised for both the default build and the
  `--features monte-carlo` (rayon) backend.
- **SimPy parity job** — builds the release `compare` example, installs
  `compare/requirements.txt`, and runs `compare/harness/run_correctness.py`. It
  fails only on a genuine regression; accepted divergences on the
  `KNOWN_EXCEPTIONS` allowlist are surfaced but do not fail the run.

`rustfmt` is intentionally **not** enforced — parts of the codebase use manual
column alignment (e.g. constant blocks in the examples) that `cargo fmt` would
rewrite.

## Running the tests

```bash
cargo test                   # all tests
cargo test --test timeout    # timeout tests only
cargo test --test event      # event tests only
cargo test --test resource   # resource tests only
cargo test --test preemptive_resource  # preemptive-resource tests only
cargo test --test system     # system tests only
```

## Running the benchmarks

```bash
cargo bench                                    # all benchmark groups
cargo bench -- timeout_throughput              # one group only
cargo bench -- --save-baseline main            # save current results as baseline
cargo bench -- --baseline main                 # compare against saved baseline
cargo bench -- --output-format bencher         # machine-readable output for CI
```

HTML reports are written to `target/criterion/`.

## Checking test coverage

Coverage requires `cargo-llvm-cov` and the `llvm-tools` rustup component:

```bash
# One-time setup
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov

# Summary (one line per file)
cargo llvm-cov --summary-only

# Annotated source (shows hit counts per line)
cargo llvm-cov --text

# HTML report (opens in browser)
cargo llvm-cov --open
```

The suite currently runs **~140 tests** (≈107 integration + ≈27 inline unit) plus
19 doc-tests (including the 5 `tutorial` chapters — the tutorial is compile-checked
on every test run), all green on both the default and `--features monte-carlo` builds.
Line coverage has historically sat around ~98%; rather than pin per-file numbers
that go stale, regenerate them on demand:

```bash
cargo llvm-cov --summary-only        # headline + per-file table
```

The only intentionally-uncovered lines are the noop-waker vtable callbacks
(`clone`/`wake`/`drop`) inside `executor/queue.rs`'s `#[cfg(test)]` block — test
infrastructure that is never actually invoked.

## Structure

All tests live in `tests/` and use Rust's native integration test harness.
No mocking — every test drives real `SimEnv` instances.

Shared idiom for capturing process output:

```rust
type Log = Rc<RefCell<Vec<String>>>;
fn new_log() -> Log { Rc::new(RefCell::new(Vec::new())) }
```

All tests use `SimEnv::with_seed(0)` (or another fixed seed) for reproducibility.

## Test files

### tests/timeout.rs — 10 tests

| Test | What it verifies |
|---|---|
| `single_timeout_advances_time` | `env.now()` advances correctly after a timeout |
| `multiple_timeouts_fire_in_time_order` | Events fire in time order, not spawn order |
| `run_until_stops_at_boundary` | `run_until(t)` stops at `t`, pending events beyond `t` do not fire |
| `zero_delay_timeout` | `timeout(0.0)` still enters the queue; process completes with `env.now() == 0.0` |
| `deterministic_tie_breaking` | Two processes with equal delay complete in spawn order (via `seq_counter`) |
| `simenv_new_creates_valid_env` | `SimEnv::new()` (entropy-seeded) produces a usable environment |
| `simenv_timeout_method` | `timeout()` called directly on `SimEnv` (not via `EnvHandle`) |
| `rng_guard_covers_all_rngcore_methods` | `next_u32`, `fill_bytes`, `try_fill_bytes` all delegate correctly |
| `timeout_repoll_before_deadline_returns_pending` | Regression: `all_of!` re-polls of a scheduled-but-unfired `Timeout` return `Pending` |
| `any_of_first_pass_all_pending` | `any_of!` first-poll pass does not falsely resolve unfired timeouts |

### tests/event.rs — 5 tests

| Test | What it verifies |
|---|---|
| `basic_fire_and_wake` | Waiter wakes at the correct time when trigger fires |
| `fire_before_await_resolves_immediately` | `fired` latch: awaitable resolves without suspending if already fired |
| `multi_waiter_all_wake` | All waiters are woken simultaneously when trigger fires (`waiters.drain(..)`) |
| `clone_shares_underlying_event` | Cloned `EventAwaitable` shares the same `Rc<RefCell<EventState>>` |
| `envhandle_event_method` | `event()` called directly on `EnvHandle` (not via `SimEnv`) |

### tests/resource.rs — 6 tests

| Test | What it verifies |
|---|---|
| `acquire_when_capacity_available` | Process acquires without suspending when capacity is free |
| `block_and_wake_on_drop` | Waiter unblocks exactly when the guard is dropped |
| `fifo_ordering_three_waiters` | Waiters are served in spawn order (`WaitQueue<()>` heap, FIFO by insertion `seq`) |
| `guard_drop_releases_exactly_one` | Dropping a guard wakes exactly one waiter, not all |
| `in_use_and_capacity_counters` | `in_use()` and `capacity()` return correct values throughout the lifecycle |
| `zero_capacity_panics` | `Resource::new(0)` panics with the expected message |

### tests/priority_resource.rs — 7 tests

| Test | What it verifies |
|---|---|
| `acquire_immediately` | Free capacity → resolves without suspending |
| `higher_priority_served_first` | Priority 0 waiter woken before priority 1, even if priority 1 spawned first |
| `fifo_within_same_priority` | Three waiters at the same priority level served in spawn order (`seq` counter) |
| `guard_drop_releases_exactly_one` | `BinaryHeap::pop()` wakes only the top-priority waiter, not all |
| `in_use_and_capacity_counters` | `in_use()` and `capacity()` track correctly throughout the lifecycle |
| `multi_capacity_mixed_priorities` | Capacity=2 with four interleaved priorities; on simultaneous release both priority-0 waiters acquire ahead of priority-1 |
| `zero_capacity_panics` | `PriorityResource::new(0)` panics with the expected message |

### tests/preemptive_resource.rs — 11 tests

Covers `PreemptiveResource`: cooperative-at-yield preemption, victim selection,
and capacity accounting under eviction.

| Test | What it verifies |
|---|---|
| `acquire_immediately_when_free` | Free capacity → resolves without suspending or preempting |
| `higher_priority_preempts_holder_immediately` | A higher-priority request evicts a holder at its yield point (t=1, not after the holder's 100-unit service) |
| `equal_priority_does_not_preempt` | An equal-priority request does not preempt; it queues and waits |
| `lower_priority_request_waits_for_release` | A lower-priority request cannot preempt; waits for normal release |
| `preempts_lowest_priority_holder_among_many` | With capacity 2, the priority-8 holder is evicted, not the priority-3 one |
| `tie_break_preempts_most_recently_acquired` | Among equal-lowest-priority holders, the most recently acquired is evicted |
| `preemptor_blocks_when_no_victim_available` | Equal-priority request with no valid victim blocks until release |
| `preempted_guard_drop_does_not_double_release` | Dropping an already-preempted guard is a no-op (no `in_use` underflow / spurious free) |
| `release_wakes_blocked_waiter_in_priority_order` | Plain blocked-waiter path still serves in priority order (delegated to `WaitQueue`) |
| `zero_capacity_panics` | `PreemptiveResource::new(0)` panics with the expected message |
| `woken_waiter_losing_same_tick_race_is_not_starved` | Two equal-priority requests race for one freed unit at the same tick; the loser stays queued and is still served later — neither is starved |

### tests/combinator.rs — 8 tests

#### AnyOf

| Test | Scenario | What it verifies |
|---|---|---|
| `any_of_first_timeout_wins` | `any_of![timeout(1.0), timeout(5.0)]` | Resolves at t=1; earlier future wins |
| `any_of_event_beats_timeout` | `any_of![timeout(10.0), signal]`; event fires at t=2 | Resolves at t=2 even though timeout is pending |
| `any_of_already_fired_event` | Event fired before `any_of` is awaited | Resolves at t=0 via the `fired` latch |
| `any_of_empty_panics` | `AnyOf::new(vec![])` | Panics with expected message |

#### AllOf

| Test | Scenario | What it verifies |
|---|---|---|
| `all_of_waits_for_last` | `all_of![timeout(1.0), timeout(2.0), timeout(5.0)]` | Resolves at t=5 (slowest) |
| `all_of_two_events` | `all_of![event_a, event_b]`; A fires at t=3, B fires at t=7 | Resolves at t=7 |
| `all_of_empty_resolves_immediately` | `AllOf::new(vec![])` | Resolves at t=0 without suspending |
| `all_of_mixed_timeout_and_event` | `all_of![timeout(5.0), signal]`; signal fires at t=8 | Resolves at t=8 |

### tests/container.rs — 15 tests

| Test | What it verifies |
|---|---|
| `get_immediate_when_level_sufficient` | `get` resolves without suspending when level ≥ amount |
| `put_immediate_when_space_available` | `put` resolves without suspending when space available |
| `get_blocks_then_wakes_on_put` | `get` suspends; wakes exactly when a `put` makes level sufficient |
| `put_blocks_then_wakes_on_get` | `put` suspends when full; wakes when a `get` frees space |
| `fifo_ordering_for_get_waiters` | Three blocked `get`s served in spawn order as `put`s trickle in |
| `fifo_ordering_for_put_waiters` | Three blocked `put`s (full container) served in spawn order |
| `fresh_small_get_queues_behind_blocked_large_get` | Strict head-of-line FIFO: a fresh fitting `get` does **not** bypass an already-blocked larger `get` (matches SimPy) |
| `fresh_small_put_queues_behind_blocked_large_put` | Strict head-of-line FIFO: a fresh fitting `put` does **not** bypass an already-blocked larger `put` |
| `cascade_satisfies_multiple_gets` | One large `put` wakes multiple pending `get`s in one cascade pass |
| `cascade_chain_get_put_get_put` | `trigger_cascade` loop resolves a 5-step get→put→get→put→get chain in one pass |
| `immediate_get_cascade_wakes_multiple_blocked_puts` | An immediate `get` frees space and wakes multiple blocked `put`s in one cascade pass (put-side mirror) |
| `level_and_capacity_accessors` | `level()`/`capacity()` return correct values before/after operations |
| `zero_capacity_panics` | `Container::new(0.0, 0.0)` panics |
| `negative_capacity_panics` | `Container::new(-1.0, 0.0)` panics |
| `initial_level_exceeds_capacity_panics` | `Container::new(5.0, 6.0)` panics |

### tests/process_handle.rs — 9 tests

Covers the `ProcessHandle<T>` feature: `spawn` returning a handle, awaiting
it, detaching via drop, composition with combinators, and nesting.

| Test | What it verifies |
|---|---|
| `handle_returns_value` | `spawn(async { ...; T })` returns a handle whose `.await` yields `T` |
| `await_before_completion` | Awaiter suspends until the spawned process finishes |
| `await_after_completion` | If the child finished long ago, awaiting resolves in the same poll (no suspension) |
| `handle_dropped_detaches_process` | Dropping the handle at t=0 does not stop the child; its side-effect runs |
| `completed_unawaited_drops_value` | Handle drop after child completion runs `Drop` on the stored `T` (verified with a drop-counter) |
| `all_of_on_handles` | `all_of![h1.discard(), h2.discard()]` waits for the slowest child |
| `any_of_on_handles` | `any_of![h1.discard(), h2.discard()]` resolves when the first child finishes |
| `nested_handle` | Inner process returns `ProcessHandle<u32>`; outer awaiter unwraps twice |
| `handle_awaited_from_different_process` | A handle can be moved across processes and awaited from any of them |

### tests/dropped_awaitable.rs — 6 tests

Verifies that abandoning a suspendable future (e.g., via `any_of!` where a
competing arm wins) does not corrupt the primitive's internal waiter queue.
All primitives use a shared `Rc<Cell<bool>>` canceled flag between the
request future and its queue entry.

| Test | What it verifies |
|---|---|
| `dropped_event_awaitable_does_not_block_others` | Dropping one `EventAwaitable` does not prevent other waiters from being woken on `fire()` |
| `dropped_resource_request_does_not_starve_followup` | Live waiter behind a canceled entry is still served on guard release |
| `dropped_priority_resource_request_does_not_starve_followup` | Same, for `PriorityResource` |
| `dropped_container_get_does_not_leak_level` | Cascade does not deduct level for a canceled `get` entry |
| `dropped_container_put_does_not_add_level` | Cascade does not add level for a canceled `put` entry |
| `dropped_container_get_does_not_starve_followup` | Live `get` waiter behind a canceled one is still served by a later `put` |

### tests/same_tick_races.rs — 5 tests

Regression suite for Finding F1 (`reviews/2026-07-01-implementation-review.md`):
a woken `WaitQueue` waiter must not be stranded — nor jumped in FIFO/priority
order — by a *fresh* request that lands in the **same ready batch** (both woken
by one `EventTrigger::fire()`). Four of the five tests fail against the pre-fix
`release()` (verified by temporary revert); the guard-drop test guards the fixed
protocol's accounting.

| Test | What it verifies |
|---|---|
| `resource_woken_fifo_waiter_beats_same_batch_fresh_request` | `Resource`: the earlier FIFO waiter acquires before a same-batch fresh request and is not stranded when the fresh request holds across a yield |
| `priority_woken_waiter_beats_same_batch_fresh_request` | `PriorityResource`: same, with the higher-priority woken waiter winning |
| `preemptive_woken_waiter_beats_same_batch_fresh_request` | `PreemptiveResource`: the plain blocked-waiter release path (also via `WaitQueue`) is not subject to the steal |
| `resource_granted_then_dropped_passes_unit_to_next_waiter` | Request arm first in the `any_of!`: the grant is consumed into a guard, then the whole arm (guard included) is dropped — the guard drop passes the unit on, no double-release |
| `resource_granted_but_unconsumed_drop_passes_unit_on` | Timeout arm first in the `any_of!`: the request is dropped while `granted && !consumed` — the request's `Drop` hands the unit on to the next waiter (branch coverage confirmed by instrumentation) |

### tests/adversarial_scheduling.rs — 3 tests

Same-ready-batch scheduling probes (Finding T2): a shared `EventAwaitable` +
one `fire()` forces N processes into a single `poll_ready` batch, then stresses
the orderings that broke F1.

| Test | What it verifies |
|---|---|
| `double_release_with_two_waiters_same_batch` | Cap-2 pool: two same-batch releases + two earlier waiters + a fresh requester — waiters served first, fresh one queues behind, accounting balances |
| `preempt_during_batch` | A high-priority `PreemptiveResource` request evicts a holder while unrelated processes share the batch; victim observes preemption |
| `container_mixed_put_get_same_batch` | Three `put`s released in one batch serve three blocked `get`s in FIFO via the cascade, with no level drift |

### tests/system.rs — 5 tests

Scenario: **Job Shop with Quality Gate** — a machine (`Resource`, capacity 1) and a
quality gate (`EventTrigger`/`EventAwaitable`). No job may start until the inspector
fires the gate at t=3. Three jobs then queue for the machine sequentially.

| Test | What it verifies |
|---|---|
| `test_system_all_primitives` | Exact event trace with fixed durations: `["gate:3", "job1_done:5", "job2_done:7", "job3_done:9"]`, `env.now() == 9.0` |
| `test_system_determinism` | Same seed → identical trace; different seed → different trace (RNG-driven durations) |
| `monte_carlo_run` | `monte_carlo::run` spawns one thread per seed, returns results in seed order |
| `dropping_env_reclaims_suspended_processes` | Dropping a `SimEnv` with a still-suspended process breaks the `SimState`↔process `Rc` cycle (no leak across replications) |
| `monte_carlo_propagates_worker_panic` | A worker panic is re-raised on the caller via `resume_unwind` (Finding T4; holds on both backends) |

### tests/external_feed.rs — 5 tests

Scenario: a small M/M/1-style model driven by `SimEnv::with_source(SplitMix64::new(seed))`,
exercising the pluggable external random feed (`src/rng.rs`).

| Test | What it verifies |
|---|---|
| `same_seed_produces_identical_trace` | Two runs with the same seed produce an identical trace (full determinism) |
| `different_seeds_diverge` | Different seeds produce different traces |
| `set_seed_restarts_the_stream` | `SimEnv::set_seed` reseeds the source, restarting its stream |
| `env_feed_matches_standalone_splitmix64` | The env feed and a standalone `SplitMix64` produce the same samples |
| `handles_share_one_feed` | All `EnvHandle` clones draw from one shared feed (draws interleave, not restart) |

### tests/scheduling_order.rs — 5 tests

Ordering guarantees models ported from SimPy lean on, pinned so a change to
them is deliberate.

| Test | What it verifies |
|---|---|
| `timeout_is_queued_at_first_poll_with_its_creation_deadline` | A timeout's deadline is fixed at creation, but it enters the queue at first poll: equal deadlines fire in awaiting order, not creation order |
| `process_spawned_mid_run_starts_in_the_same_instant` | A child spawned by a running process runs in the same instant, before the clock moves |
| `fired_event_resumes_waiters_before_a_same_instant_timeout` | `fire()` resumes waiters in the same ready-queue pass, ahead of a timeout already queued for that instant (SimPy would run the timeout first) |
| `losing_timeout_wakes_the_process_spuriously_and_is_ignored` | The losing timeout of an `any_of!` wakes the process at its deadline; the timeout it awaits by then stays pending |
| `losing_event_wakes_the_process_spuriously_and_is_ignored` | The losing event of an `any_of!` wakes the process when fired later; the event it awaits by then stays pending |

### src/rng.rs — 9 inline unit tests

Inline `#[cfg(test)]` module covering the portable feed. The SplitMix64
known-answer table is the **same** one asserted by `compare/models/test_feed.py`,
guarding against cross-language drift.

| Test | What it verifies |
|---|---|
| `splitmix64_known_answer_vectors` | Canonical SplitMix64 outputs for seeds 0 and 42 |
| `uniform01_and_exponential_known_answer` | `uniform01`/`exponential` produce the exact shared-table values |
| `uniform01_in_unit_interval` | `uniform01` stays in `[0, 1)` |
| `next_u32_is_high_bits_of_next_u64` | `next_u32` takes the high 32 bits (matches the Python feed) |
| `same_seed_same_stream` | Two feeds with the same seed produce the same stream |
| `reseed_restarts_stream` | `reseed`/`set_seed` restart the stream |
| `exponential_mean_is_sane` | Sampled exponential mean converges to the target |
| `normal_consumes_two_draws_and_is_centered` | `normal` consumes exactly two uniforms and is centered |
| `stdrng_reseed_is_deterministic` | `RandomSource::reseed` on `StdRng` is deterministic |

### src/executor/queue.rs — 3 inline unit tests

Inline `#[cfg(test)]` module testing `ScheduledWaker`'s `PartialEq` implementation,
which cannot be reached from integration tests (the type is `pub(crate)`).

| Test | What it verifies |
|---|---|
| `partial_eq_same_time_and_seq` | Equal time and seq → equal |
| `partial_eq_different_seq` | Same time, different seq → not equal |
| `partial_eq_different_time` | Different time → not equal |

### src/resource/wait_queue.rs — 5 inline unit tests

Inline `#[cfg(test)]` module exercising the `pub(crate)` `WaitQueue<K>` helper
directly (it is not reachable from integration tests). Uses a safe
`std::task::Wake` recorder to observe wake order.

| Test | What it verifies |
|---|---|
| `try_acquire_respects_capacity` | `try_acquire`/`release` track `in_use` against capacity (no waiters queued) |
| `release_hands_off_directly_without_freeing_the_unit` | Direct-handoff (F1): `release` transfers the unit to the waiter (`granted` set, woken) with `in_use` pinned at capacity, so a concurrent `try_acquire` cannot steal it |
| `fifo_order_for_unit_key` | `WaitQueue<()>` hands the unit to waiters in pure insertion (FIFO) order; `in_use` stays at 1 until the queue empties |
| `priority_order_then_fifo_within_level` | `WaitQueue<u32>` serves lowest key first, FIFO within a level |
| `release_skips_canceled_waiter` | A canceled top-priority entry is skipped (not woken, not granted) so the next live waiter is handed the unit |
| `release_with_only_canceled_waiters_frees_the_unit` | All-canceled queue wakes/grants no one and genuinely frees the unit (`in_use` drops) |

## Benchmark groups (benches/simulation.rs)

All benchmarks use `SimEnv::with_seed(0)` — fully deterministic, no file I/O.
N values are the parameterized workload sizes passed to `BenchmarkId`.

| Group | What it measures | N values |
|---|---|---|
| `timeout_throughput` | Raw event-queue + executor throughput (BinaryHeap push/pop, RefCell borrows, waker path) | 1 000 / 10 000 / 100 000 |
| `resource_contention` | `ResourceRequest` waker registration, `WaitQueue` heap push/pop, `ResourceGuard::Drop` direct-handoff chain | 100 / 1 000 / 10 000 |
| `event_broadcast` | `EventAwaitable` waker registration and `waiters.drain(..)` dispatch when all N wake simultaneously | 100 / 1 000 / 10 000 |
| `mixed_workload` | End-to-end throughput combining spawn, timeouts, and two resources (nurse cap=1, beds cap=3) | 100 / 1 000 / 10 000 |
| `monte_carlo_scaling` | Parallelism efficiency: K independent copies of `mixed_workload(100)` via `monte_carlo::run` | 1 / 2 / 4 / 8 threads |
| `priority_contention` | `PriorityResource` heap ordering (`BinaryHeap<Entry<u32>>`) across four priority levels + handoff chain | 100 / 1 000 / 10 000 |
| `preemptive_contention` | `PreemptiveResource` eviction path: victim scan + holder registry under alternating hi/lo priorities (cap 2) | 100 / 1 000 / 10 000 |
| `container_throughput` | `Container` head-of-line FIFO cascade (`wake_get`/`wake_put_waiters`) with N producers/consumers | 100 / 1 000 / 10 000 |

## Cross-engine parity (SimPy)

Beyond the `cargo test` suite, the `compare/` harness validates `simu` against
Python's [SimPy](https://simpy.readthedocs.io/) as a reference oracle. Both
engines draw from the **same portable feed** (`SplitMix64` + shared transforms,
re-implemented in `compare/models/_feed.py`), so the order-insensitive queue
models are checked in **exact mode** — per-seed metrics must match within 1e-9
(they land ~1e-15) — while `hospital` stays on the distributional test for its
eviction-handoff ordering exception. Queue models are also checked against
closed-form queueing theory. Performance is measured on two axes: single-thread
engine efficiency, and a **Monte Carlo** benchmark where simu parallelises
independent replications via `monte_carlo::run` (rayon) while SimPy is
GIL-serialised. The harness is what surfaced the `Container` strict-FIFO
divergence (since fixed). Run it with `compare/run_comparison.sh`; see
[`compare/README.md`](compare/README.md) for methodology and
[`compare/REPORT.md`](compare/REPORT.md) for the latest results.

The portable feed's cross-language known-answer test (`compare/models/test_feed.py`)
shares its SplitMix64 vectors with the Rust `rng` unit tests, so the two
implementations cannot silently drift.

Accepted, documented divergences (currently just `hospital.early_discharged`)
are listed in a `KNOWN_EXCEPTIONS` allowlist in `harness/run_correctness.py`:
they are surfaced in the report as `known ⚠` but do not fail the run, so the
harness exits non-zero only on a genuine regression — ready to gate a future CI
job.

## Key implementation notes

- **Sequential process execution within a tick**: the executor runs each process
  to its next suspension point before moving to the next. A guard acquired and
  immediately dropped in one process is gone before the next process runs in the
  same tick. Tests that require two guards to be held simultaneously must yield
  (e.g. via `timeout`) before dropping.
- **Zero-delay timeouts**: `timeout(0.0)` always returns `Pending` on the first
  poll and re-enters the event queue, even though the deadline equals the current
  time. It fires on the next event-loop iteration, not synchronously.
- **RNG sampling before async blocks**: `env.rng()` returns a `RefMut` guard that
  cannot be held across an `.await`. Sample values before the `async move` block
  and move the sampled value in.
- **`Timeout` spurious-wakeup safety**: `Timeout::poll` checks both `scheduled`
  and `env.now() >= deadline` before returning `Ready`. This means `AllOf` (and
  any other combinator) can safely re-poll a `Timeout` sub-future when a *different*
  sub-future fires — the re-polled timeout returns `Pending` until its own deadline
  is reached.
