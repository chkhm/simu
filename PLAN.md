# Implementation Plan

Each step adds a self-contained increment of functionality and extends `examples/hospital.rs` to demonstrate it.

---

## Step 1: SimEnv + Timeout + Executor ✅

The foundational heartbeat of the library.

**Files:**
- `src/executor/queue.rs` — min-heap event queue with tie-breaking sequence numbers
- `src/executor/waker.rs` — custom `Waker` implementation
- `src/executor/mod.rs` — event loop that drives future polling
- `src/env.rs` — `SimEnv`: `spawn()`, `run()`, `run_until()`, `now()`, `timeout()`; `EnvHandle` for use inside processes
- `src/timeout.rs` — `Timeout` future

**Hospital example:** a handful of patients arrive at fixed intervals, each sleeping for a fixed treatment duration, printing their discharge time.

---

## Step 2: Manual Event ✅

Adds inter-process signalling.

**Files:**
- `src/event.rs` — `EventTrigger` / `EventAwaitable`

**Design details:**
- Multiple processes can await the same `EventAwaitable` (multi-waiter via `Vec<Waker>`).
- `EventTrigger::fire()` drains all registered wakers in one pass.
- `fired` latch: if `fire()` is called before any process awaits, the awaitable resolves immediately on the first poll.

**Hospital example:** a triage nurse process signals a patient process when assessment is complete.

---

## Step 3: Resource ✅

Adds shared, capacity-limited resources with RAII acquisition.

**Files:**
- `src/resource/mod.rs` — `Resource`, `ResourceRequest` (future), `ResourceGuard` (RAII)

**Design details:**
- `Resource` wraps `Rc<RefCell<ResourceState>>` and is `Clone`.
- Waiters are served FIFO via `VecDeque<Waker>`.
- `ResourceGuard::drop` pops and wakes exactly one waiter.
- `Resource::new(0)` panics with `"capacity must be at least 1"`.

**Hospital example:** patients contend for a limited pool of beds and doctors.

---

## Step 4: Seeded RNG ✅

Makes simulations stochastic and reproducible.

**Changes:**
- `SimEnv::with_seed(seed: u64)` constructor.
- `SimEnv::new()` seeds from OS entropy.
- `EnvHandle::rng()` returns a `RngGuard` newtype wrapping `RefMut<StdRng>`.
- `RngGuard` implements `RngCore` by delegation — safe to use with `rand_distr` distributions.
- Sampling must happen before `async move` blocks (guard cannot cross `.await`).

**Hospital example:** Poisson inter-arrival times and exponential service durations. Same seed → identical output.

---

## Step 5: Monte Carlo ✅

Adds parallel multi-run support.

**Files:**
- `src/monte_carlo.rs` — `run(seeds, f) -> Vec<R>` using `Arc<F>` + `std::thread::spawn`

**Design details:**
- `SimEnv` is `!Send`, so it is created *inside* each closure — never crosses thread boundaries.
- Results are returned in seed order (one element per seed).

**Hospital example:** runs 10 simulations with different seeds; each run writes to `target/sim-logs/run_<N>.log`; `main()` prints a summary table of patient throughput and resource wait times.

---

## Step 6: Test Suite ✅

Comprehensive integration and unit tests, plus code coverage analysis.

**Files:**
- `tests/timeout.rs` — 8 tests covering the executor and `Timeout` future
- `tests/event.rs` — 5 tests covering `EventTrigger` / `EventAwaitable`
- `tests/resource.rs` — 6 tests covering `Resource`, `ResourceGuard`, FIFO ordering
- `tests/system.rs` — 3 system tests (Job Shop with Quality Gate, determinism, Monte Carlo)
- `src/executor/queue.rs` — 3 inline unit tests for `ScheduledWaker::PartialEq` (type is `pub(crate)`)
- `TESTING.md` — test strategy, coverage commands, coverage table, benchmark guide

**Coverage:** 98.6% lines / 97.7% regions across all library source files.

---

## Step 7: Performance Benchmarks ✅

Criterion-based benchmark suite for regression detection and throughput measurement.

**Files:**
- `benches/simulation.rs` — five parameterized benchmark groups
- `Cargo.toml` — added `criterion = "0.5"` dev-dependency and `[[bench]]` entry

**Benchmark groups:**

| Group | Scenario | N values |
|---|---|---|
| `timeout_throughput` | N processes each `timeout(i).await` — raw executor baseline | 1K / 10K / 100K |
| `resource_contention` | N processes queue for 1-capacity resource, hold 1 tick each | 100 / 1K / 10K |
| `event_broadcast` | N processes await one event; fired simultaneously | 100 / 1K / 10K |
| `mixed_workload` | N patients: nurse (cap 1) → hold 5t → beds (cap 3) → hold 20t | 100 / 1K / 10K |
| `monte_carlo_scaling` | K threads, each running `mixed_workload(100)` | 1 / 2 / 4 / 8 |

HTML reports: `target/criterion/`. Baseline workflow: `--save-baseline main` then `--baseline main`.

---

## Step 8: PriorityResource ✅

Priority-ordered resource pool: lowest priority number served first; FIFO tie-breaking within the same priority level.

**Files:**
- `src/resource/priority.rs` — `PriorityResource`, `PriorityResourceRequest`, `PriorityResourceGuard`
- `src/resource/mod.rs` — added `pub use priority::...`
- `src/lib.rs` — re-exported `PriorityResource`, `PriorityResourceGuard`, `PriorityResourceRequest`
- `tests/priority_resource.rs` — 6 tests
- `examples/hospital.rs` — nurse changed to `PriorityResource`; patients get triage levels (critical=0, standard=1)

**Design:**
- `BinaryHeap<PriorityWaiter>` with reversed `Ord` so `pop()` yields the lowest `(priority, seq)` pair
- `next_seq` counter in state for FIFO tie-breaking within the same priority level
- `registered` + `seq` on the future — same double-registration guard as `ResourceRequest`
- API: `r.request(priority: u32) -> PriorityResourceRequest` — priority always explicit at call site

---

## Step 9: AnyOf / AllOf Combinators ✅

Race and barrier combinators: `AnyOf` resolves when the first sub-future fires;
`AllOf` resolves when all sub-futures have fired.

**Files:**
- `src/combinator.rs` — `AnyOf`, `AllOf`, `any_of!` and `all_of!` macros
- `src/timeout.rs` — bug fix: `poll` now checks `scheduled && now() >= deadline`
  instead of just `scheduled`; needed so `AllOf` can re-poll timeouts without
  false positives
- `src/lib.rs` — added `pub mod combinator`; re-exported `AnyOf`, `AllOf`
- `tests/combinator.rs` — 8 tests
- `examples/hospital.rs` — added bed-pressure monitor + `any_of!` in patient
  treatment; added `early_discharged` stat and summary column

**Design:**
- `AnyOf::poll`: iterates sub-futures, returns `Ready` on first that resolves
- `AllOf::poll`: uses `Vec::retain_mut` to drop completed futures; returns `Ready`
  when list is empty
- Both types are `Unpin` (only contain `Vec` and `Pin<Box<...>>`), so `get_mut()`
  is safe in `poll`
- Macros auto-`Box::pin` each expression, eliminating call-site verbosity

---

---

## Step 10: Container ✅

Continuous-quantity resource: a reservoir with `level: f64` and `capacity: f64`.
Both `put(amount)` and `get(amount)` are suspendable futures (FIFO queues).
Level changes are committed eagerly by the wake cascade via an `Rc<Cell<bool>>`
done-flag shared between the future and its waiter entry.

**Files:**
- `src/resource/container.rs` — `Container`, `ContainerPutRequest`, `ContainerGetRequest`,
  `ContainerState`, `GetWaiter`/`PutWaiter`, `wake_get_waiters`, `wake_put_waiters`, `trigger_cascade`
- `src/resource/mod.rs` — added `pub mod container` + re-exports
- `src/lib.rs` — re-exported `Container`, `ContainerGetRequest`, `ContainerPutRequest`
- `tests/container.rs` — 11 integration tests
- `examples/hospital.rs` — blood bank sub-scenario (Container with restocking process)

**Design:**
- `put` blocks when `level + amount > capacity`; `get` blocks when `level < amount`
- FIFO `VecDeque` for both get and put waiter queues
- `Rc<Cell<bool>>` done-flag: cascade commits level change and sets flag before waking,
  so re-polled future returns `Ready` without rechecking level (preserves FIFO)
- `trigger_cascade` loops `wake_get_waiters`/`wake_put_waiters` until stable (handles
  cascading chains where a put immediately enables a get enables another put, etc.)

---

## Step 11: ProcessHandle ✅

`spawn` now returns `ProcessHandle<F::Output>` — a tokio-`JoinHandle`-style
future resolving to the process's return value. Enables return values from
processes, process-level `any_of!`/`all_of!` composition, and observable
completion.

**Files:**
- `src/process.rs` — `ProcessHandle<T>`, `ProcessSlot<T>`, `spawn_with_handle()`
  factory, `discard()` adapter for combinators
- `src/env.rs` — generalized `spawn` signatures (both `SimEnv` and `EnvHandle`)
- `src/lib.rs` — `pub mod process;` and `pub use ProcessHandle`
- `tests/process_handle.rs` — 9 integration tests
- `examples/hospital.rs` — `arrivals` collects patient handles; on shift end,
  `AllOf` joins them (with a hard `any_of` deadline) to log "ED clear" time

**Design:**
- Non-`Clone`, single-await — broadcast patterns use `EventTrigger`.
- Completion via `async move` wrapper that stores result in
  `Rc<RefCell<ProcessSlot<T>>>` and wakes a registered awaiter. No executor
  changes — the process table stays `HashMap<_, Pin<Box<dyn Future<Output=()>>>>`.
- Waker dedup via `Waker::will_wake` (same pattern as `EventAwaitable`).
- Panic semantics unchanged: process panic crashes the sim.
- Breaking change to `spawn` signature was source-compatible for every
  existing callsite in tests, benches, and examples (the returned
  `ProcessHandle<()>` is dropped at the statement boundary).

---

## Step 12: Resource with a Changing Capacity

A pool whose number of servers changes while the simulation runs: servers who come
on shift and go off, a lane that opens and closes, machines that break down and come
back. Today `Resource`'s capacity is fixed at `new`, so a model must build its own
pool on events to vary it.

**Files:**
- `src/resource/mod.rs` — `Resource::set_capacity(n)`, and `capacity()` reporting the current value
- `src/resource/wait_queue.rs` — grant waiters when capacity grows; hold grants while `in_use > capacity`
- `tests/resource_capacity.rs` — integration tests
- `examples/hospital.rs` — doctors that come on and off shift

**Design details:**
- `set_capacity(n)` takes effect at the call, in the current instant. `n = 0` is allowed on a
  running pool (a closed lane); `Resource::new(0)` still panics.
- Growing: as many head-of-queue waiters as there are new free units are granted at once, in
  FIFO order, by the same direct handoff as a release.
- Shrinking below `in_use`: current holders keep their units; no new grant is made until
  `in_use` falls below the new capacity. A guard dropped meanwhile frees no unit to the queue.
- A waiter withdrawn (its request dropped) while capacity is zero leaves no stuck entry.
- Ordering at one instant: a `set_capacity` and a guard drop in the same instant grant in the
  order they happen, each by the queue's FIFO; no reordering by priority of call site.

**Tests:** growing wakes exactly the new number of waiters, FIFO; shrinking below `in_use`
blocks grants until enough holders leave; capacity 0 then back to 1 serves the head;
`request_now` requests made while closed keep their order; a dropped request while closed;
`in_use`/`queue_len` after each change.

**Docs:** SPEC §4.5 (the pool's capacity is no longer constant; the invariants of §4.7's
`WaitQueue` direct handoff), API.md, CHANGELOG (Added), TESTING.md (new file and counts),
`llms.txt`.

**Size:** small to medium (one method and the queue's grant loop). **Depends on:** nothing.

---

## Step 13: `request_now` for PriorityResource and PreemptiveResource

`Resource::request_now()` queues a request when it is made, as SimPy's `request()` does. The
priority pools still register a request at its first poll, so two requests of equal priority
made in one order and awaited in the other are served in the awaiting order.

**Files:**
- `src/resource/priority.rs` — `PriorityResource::request_now(priority)`, `PriorityResourceRequest::is_granted()`
- `src/resource/preemptive.rs` — `PreemptiveResource::request_now(priority)`, `PreemptiveRequest::is_granted()`
- `src/resource/wait_queue.rs` — reuse the waker slot that starts empty until the first poll
- `tests/priority_resource.rs`, `tests/preemptive_resource.rs` — new cases

**Design details:**
- Same contract as `Resource::request_now`: a free unit is taken at the call, else the entry
  joins the queue at the call, ordered by `(priority, seq)` with `seq` taken at the call.
- A preempting `request_now` preempts at the call, not at its first poll; the victim's
  `preempted()` event fires in that instant.
- Dropping an unpolled or pending request withdraws it; dropping a granted one releases the unit.

**Tests:** equal-priority FIFO by call order when awaited in reverse; a higher priority made
later still goes first; withdrawal on drop before the first poll; `is_granted` before and
after the grant; preemption at the call.

**Docs:** SPEC §4.5, API.md, CHANGELOG (Added), TESTING.md, `llms.txt`.

**Size:** small (the slot exists). **Depends on:** nothing.

---

## Step 14: Events That Carry a Value

SimPy's `event.succeed(value)` hands a value to every waiter (which item arrived, who
answered, what a check found). `EventTrigger::fire()` resolves to `()`, so models keep the
value beside the event in shared state.

**Files:**
- `src/event.rs` — `ValueTrigger<T>` / `ValueAwaitable<T>` (or a generic `EventTrigger<T = ()>`), `fire_with(value)`
- `src/env.rs` — `event_with::<T>()` on `SimEnv` and `EnvHandle`
- `tests/event.rs` — new cases

**Design details:**
- `T: Clone`; each waiter receives a clone. `fire_with` consumes the trigger, as `fire` does.
- A waiter that awaits after the event fired receives the value at once (the `fired` latch keeps it).
- `is_fired()` stays; `value()` returns `Option<T>` without awaiting.
- Keep `EventTrigger`/`EventAwaitable` source-compatible (`T = ()`), so existing code builds unchanged.
- Dropping the trigger unfired: waiters stay pending, as today.

**Tests:** every waiter gets the value; a late waiter gets it immediately; `value()` before and
after; `any_of!` with a valued event; `T = ()` behaves exactly as before.

**Docs:** SPEC §4.4, API.md, CHANGELOG (Added), TESTING.md, `llms.txt`, the tutorial.

**Size:** medium (generic plumbing through the combinators). **Depends on:** nothing; Step 15 builds on it.

---

## Step 15: `any_of!` Reports Which Arm Won

SimPy's `AnyOf` returns the events that fired. `any_of!` returns `()`, so a process must ask
each arm afterwards, and only events can answer (`is_fired()`); a timeout or a resource
request cannot.

**Files:**
- `src/combinator.rs` — `AnyOf` resolves to the index of the first arm that completed; `any_of!` returns it
- `src/timeout.rs` — `Timeout::is_elapsed()` (deadline reached and scheduled)
- `tests/combinator.rs` — new cases

**Design details:**
- The winner is the first arm, in argument order, found ready during the poll that completes
  the `AnyOf`. If several arms are ready in one instant, the lowest index wins; document it.
- Option for typed results: `any_of!` over valued futures returning an enum or `(index, value)`
  once Step 14 lands; the index form comes first.
- Breaking only for callers that bind the `()` result; plain `.await;` keeps compiling.

**Tests:** index of an event vs a timeout; two arms ready in one instant → lowest index;
a resource request arm; `Timeout::is_elapsed` before, at and after its deadline.

**Docs:** SPEC §4.4, API.md, CHANGELOG (Changed and Added), TESTING.md, `llms.txt`.

**Size:** small to medium. **Depends on:** Step 14 for the typed form only.

---

## Step 16: Cancelling a Lost Timeout, Releasing Stale Wakers

When a `Timeout` loses an `any_of!` race it is dropped, but its entry stays in the event
queue until its deadline and then wakes the process spuriously; the process's next future
returns `Pending` (pinned by `tests/scheduling_order.rs`). An `EventAwaitable` dropped while
waiting leaves its waker in the event's list until the event fires. Neither changes a result,
but both cost memory and polls in long runs with many races.

**Files:**
- `src/timeout.rs` — `Drop` marks the scheduled entry canceled
- `src/executor/queue.rs` — skip (or lazily pop) canceled entries without waking
- `src/event.rs` — `Drop` for a waiting `EventAwaitable` removes its waker
- `tests/scheduling_order.rs` — update the spurious-wake test to the new behaviour; new cases
- `benches/simulation.rs` — a race-heavy benchmark (`timeout_race`)

**Design details:**
- Cancellation must not change the order of anything else at any instant: `(time, seq)` of the
  remaining entries is untouched.
- Lazy deletion (a canceled flag checked on pop) keeps the heap O(log n); document the choice.
- A timeout already fired and awaited again is unaffected; one dropped before its first poll was
  never scheduled and has nothing to cancel.

**Tests:** a process racing an event against a long timeout is not polled at the timeout's
deadline; the queue length falls back after a race; a dropped event waiter is not woken; a
determinism test over a race-heavy model gives identical traces before and after.

**Docs:** SPEC §4.1 and §4.7 (new invariant), CHANGELOG (Changed), TESTING.md, benchmark notes.

**Size:** medium. **Depends on:** nothing.

---

## Step 17: `Interrupt`

SimPy's `process.interrupt(cause)` wakes a process at its current yield with an error it can
handle: breakdowns, recalls, a patient called away. Only `PreemptiveResource` lets one process
affect another today, and only through the victim's own guard.

**Files:**
- `src/process.rs` — `ProcessHandle::interrupt(cause)`, or a separate `Interruptor` handle
- `src/interrupt.rs` — `Interrupted<C>` result type and `EnvHandle::interruptible(fut)` wrapper
- `src/executor/mod.rs` — deliver the interrupt at the next poll of the target
- `tests/interrupt.rs` — integration tests
- `examples/hospital.rs` — a doctor called away mid-treatment

**Design details:**
- Delivery: the interrupt is scheduled at the current time, after what is already due at that
  instant; the target's awaited future resolves to `Err(Interrupted(cause))`.
- The interrupted future is dropped: a pending resource request is withdrawn, a held guard stays
  held (the process decides), a timeout is canceled (Step 16).
- Interrupting a finished process is a no-op returning `false`; two interrupts in one instant are
  delivered in order.
- Rust has no exceptions: the API is opt-in per await (`interruptible(...)`) so plain awaits stay
  `()`; document the difference from SimPy.

**Tests:** interrupt during a timeout, a resource wait, an event wait; cause delivered; guard kept
across an interrupt; interrupt of a finished process; two in one instant; determinism.

**Docs:** SPEC §4.3 and §6, API.md, CHANGELOG (Added), TESTING.md, `llms.txt`, tutorial.

**Size:** large. **Depends on:** Step 16 (cancel the dropped timeout), Step 20 helps.

---

## Step 18: `Store`, `FilterStore` and `PriorityStore`

Discrete-item queues: parts on a conveyor, messages, orders. SimPy's `Store.put(item)` /
`get()`, `FilterStore.get(predicate)` and `PriorityStore` are on the roadmap and absent.

**Files:**
- `src/resource/store.rs` — `Store<T>`, `StorePut`, `StoreGet`
- `src/resource/filter_store.rs` — `FilterStore<T>`, `get(filter)`
- `src/resource/priority_store.rs` — `PriorityStore<T: Ord>`
- `src/lib.rs` — re-exports
- `tests/store.rs` — integration tests
- `examples/` — a producer/consumer example

**Design details:**
- `Store::new(capacity)` with `usize::MAX` for unbounded; `put` blocks when full, `get` when empty.
- FIFO for both put and get waiters, head-of-line as `Container` is (§4.7); a `FilterStore` get
  that does not match lets later gets pass (SimPy semantics) — document it.
- A put that frees a get in the same instant hands the item over directly, with no extra tick.
- Dropping a pending put returns nothing to the store; dropping a pending get withdraws it.

**Tests:** FIFO items; capacity blocking; a filter get skipped by a later matching one; priority
order and ties; cascade put→get in one instant; drop of pending requests; comparison with the
SimPy harness in `compare/`.

**Docs:** SPEC §4.5 and §6, API.md, CHANGELOG (Added), TESTING.md, `llms.txt`, tutorial.

**Size:** large (three types). **Depends on:** nothing.

---

## Step 19: Stepping the Loop: `peek()` and `step()`

SimPy's `env.peek()` gives the next event's time and `env.step()` runs exactly one event. They
let a driver interleave the simulation with other work, test at a precise instant, or pause.

**Files:**
- `src/env.rs` — `SimEnv::peek() -> Option<f64>`, `SimEnv::step() -> bool`
- `tests/timeout.rs` — new cases

**Design details:**
- `peek` returns the time of the next entry that will run (skipping canceled ones, Step 16);
  `None` when nothing is scheduled.
- `step` advances the clock to that time and runs that one entry's wake, then polls the processes
  it made ready; define "one event" as one queue entry and document it.
- `run_until(t)` stays equivalent to `while peek() < Some(t) { step() }`.

**Tests:** `peek` on an empty env; `step` order equals `run` order; `run_until` equivalence;
stepping across processes spawned during a step.

**Docs:** SPEC §4.2, API.md, CHANGELOG (Added), TESTING.md, `llms.txt`.

**Size:** small. **Depends on:** Step 16 for skipping canceled entries.

---

## Step 20: `ProcessHandle::is_alive()`

SimPy's `process.is_alive` tells whether a process is still running. A `ProcessHandle` can be
awaited or dropped, but not asked, so a supervisor cannot check a worker without waiting on it.

**Files:**
- `src/process.rs` — `ProcessHandle::is_alive()` (and `is_finished()`), reading the result slot
- `tests/process_handle.rs` — new cases

**Design details:**
- Alive from `spawn` until the process's future returns; a process that panicked is not alive
  (see Step 21).
- Querying does not consume the handle or register a waker.

**Tests:** alive before and during, not after; after the result is taken; with `discard()`.

**Docs:** SPEC §4.3, API.md, CHANGELOG (Added), TESTING.md.

**Size:** small. **Depends on:** nothing.

---

## Step 21: Per-Process Panic Isolation

A panic in one process unwinds the whole `run()`. For long or Monte Carlo runs, a failing
process should end only itself and be reported, or abort the run, by the model's choice.

**Files:**
- `src/executor/mod.rs` — wrap each process poll in `catch_unwind`
- `src/env.rs` — `SimEnv::on_panic(policy)`: `Abort` (default, today's behaviour) or `Isolate`
- `src/process.rs` — a panicked process's handle resolves to an error (`ProcessHandle::join() -> Result<T, Panicked>`)
- `tests/panic.rs` — integration tests

**Design details:**
- `Isolate`: the panicking process is dropped (its guards release, its requests withdraw), the
  panic payload is recorded with the time and process id, and the run goes on.
- Types held across the poll must be `UnwindSafe` or wrapped; document what a model must not
  rely on after a caught panic (shared `RefCell` state may be mid-update).
- Default stays `Abort`, so existing models see no change.

**Tests:** `Abort` unwinds `run()` as now; `Isolate` keeps other processes running; resources
released by the dropped process are granted to the next waiter; the handle reports the panic;
determinism of the remaining trace.

**Docs:** SPEC §4.3 and §6, API.md, CHANGELOG (Added), TESTING.md.

**Size:** medium. **Depends on:** Step 20 (the handle's state).

---

## Step 22: No `Mutex` on Every Wake

The ready queue is an `Arc<Mutex<Vec<usize>>>` because `std::task::Waker` must be `Send +
Sync`, though a `SimEnv` is single-threaded and `!Send`. Every wake takes the lock.

**Files:**
- `src/executor/waker.rs` — a `RawWaker` vtable over an `Rc`-based or thread-local ready queue
- `src/executor/mod.rs` — the queue type
- `benches/simulation.rs` — compare `timeout_throughput`, `event_broadcast`, `resource_contention`

**Design details:**
- A waker used on another thread must not corrupt the queue: either detect it (panic with a clear
  message) or fall back to a locked path; document why the unsafe vtable is sound.
- Keep the waker dedup (`will_wake`) working.
- Adopt only if the benchmarks show a clear gain; otherwise record the measurement and close the
  step as "kept, with reasons".

**Tests:** the whole suite unchanged; a test that a waker moved to another thread is refused or
handled; Miri over the waker module if available.

**Docs:** SPEC §4.1 and §4.7, TESTING.md (benchmarks), CHANGELOG (Changed, if adopted).

**Size:** medium, with `unsafe`. **Depends on:** nothing; measure after Step 16.

---

## Step 23: `RealtimeEnvironment`

Synchronise simulated time with the wall clock (training, demos, hardware in the loop), as
SimPy's `RealtimeEnvironment(factor, strict)` does.

**Files:**
- `src/realtime.rs` — `RealtimeEnv` wrapping `SimEnv`, or `SimEnv::run_realtime(factor, strict)`
- `tests/realtime.rs` — tests with a fake clock
- `examples/` — a paced example

**Design details:**
- `factor` wall-clock seconds per simulated unit; before each event, sleep until its wall time.
- `strict`: fail when processing falls behind by more than one step, as SimPy does; otherwise log.
- The clock is injectable (a trait) so tests do not sleep.

**Tests:** event wall times with a fake clock; strict failure when behind; factor 0 equals `run`.

**Docs:** SPEC §4.2 and §6, API.md, CHANGELOG (Added), TESTING.md, `llms.txt`.

**Size:** medium. **Depends on:** Step 19 (`peek`/`step` drive it).

---

## Step 24: GPU/CUDA Acceleration

Batch evaluation of independent sub-simulations on GPU. Applicable only when process logic can be
expressed as data-parallel kernels (e.g. pure queuing networks). Requires further design work;
depends on the maturity of CUDA Rust bindings.

**Files:** to be designed.

**Design details:** a separate feature flag and crate module; Monte Carlo (Step 5) stays the CPU
path. Start with a design note comparing a kernel for an M/M/c queue with `monte_carlo::run`.

**Size:** large, research. **Depends on:** nothing in the core.

---

## Post-MVP Roadmap

Planned as Steps 12–24 above, in priority order (see [SPEC.md §6](SPEC.md) for the summary):

1. **Changing capacity** (Step 12) — `Resource::set_capacity` for servers that come and go.
2. **`request_now` on the priority pools** (Step 13) — requests queued when made, with `is_granted`.
3. **Valued events** (Step 14) — `fire_with(value)`, as SimPy's `succeed(value)`.
4. **`any_of!` names its winner** (Step 15) — and `Timeout::is_elapsed`.
5. **Cancel lost timeouts, release stale wakers** (Step 16) — no spurious wakes after a race.
6. **`Interrupt`** (Step 17) — one process can interrupt another (e.g., emergency preemption).
7. **`Store` / `FilterStore` / `PriorityStore`** (Step 18) — discrete-item queues with optional filter predicate.
8. **`peek()` / `step()`** (Step 19) — drive the loop one event at a time.
9. **`ProcessHandle::is_alive()`** (Step 20).
10. **Per-process panic isolation** (Step 21) — `catch_unwind` per poll so a panic ends only that process.
11. **No `Mutex` on every wake** (Step 22) — measured with the benchmarks, adopted only if it pays.
12. **`RealtimeEnvironment`** (Step 23) — synchronise simulated time to wall-clock time.
13. **GPU/CUDA acceleration** (Step 24) — batch evaluation of independent sub-simulations on GPU.

**Delivered since MVP:**

- **`PreemptiveResource`** — priority pool with cooperative-at-yield preemption
  (`guard.preempted()` / `is_preempted()`), built on the shared `WaitQueue<u32>`. See SPEC §4.5.
- **`EventAwaitable::is_fired()`** — ask whether an event has fired without awaiting it.
- **`Resource::request_now()` / `ResourceRequest::is_granted()`** — a request queued at the call. See SPEC §4.5.
- **`run_until(t)` boundary** — an event due exactly at `t` no longer runs, as documented.
- **Scheduling-order tests** — `tests/scheduling_order.rs` pins the orderings at one instant.
