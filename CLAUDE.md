# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`simu` is a Rust library for Discrete Event Simulation (DES), inspired by SimPy. The MVP is fully
implemented and tested. The crates.io package is **`simu-des`** (the name `simu` was taken) with
`[lib] name = "simu"`, so all code imports stay `use simu::…`.

Licensing: dual **MIT OR Apache-2.0**, [REUSE 3.3](https://reuse.software/spec-3.3)-compliant —
license texts live in `LICENSES/`, every code file carries an SPDX header, prose/uncommentable
files (`*.md`, `.gitignore`, `Cargo.lock`, `requirements.txt`) are covered by `REUSE.toml`, and CI
enforces it (`reuse lint` job). New files must include the SPDX header (copy from any neighbor) or
match a `REUSE.toml` glob.

Companion docs:

- `SPEC.md` — the design source of truth (architecture, API contracts, invariants, roadmap).
- `API.md` — signature cheat-sheet only; semantics live in rustdoc (docs.rs).
- `PLAN.md` — implementation plan / history.
- `TESTING.md` — test strategy and coverage notes.
- `README.md` — user-facing overview.
- `CONTRIBUTING.md` — contribution checklist (rustfmt/clippy, SPDX headers, DCO sign-off, PR process).
- `llms.txt` + `docs/simu-for-agents.md` — LLM-facing reference and drop-in agent
  context. **Rule: every code pattern in these files must be a verbatim copy of a
  doc-test** (tutorial chapter or type doc) — never hand-write parallel snippets.
- `src/tutorial.rs` — doc-only "simu in 10 minutes" module (5 chapters, all
  doc-tests); each chapter has a runnable `examples/intro_*.rs` twin.

When changing behaviour, keep `SPEC.md` and `API.md` in sync — and when changing
tutorial/type doc-tests, re-sync the copies in `llms.txt` / `docs/simu-for-agents.md`.

## Commands

```bash
cargo build
cargo test                        # 154 passing tests (134 unit + integration, 20 doc-tests)
cargo test <test_name>            # run a single test
cargo run --example hospital      # ER patient-flow simulation
cargo run --example brewery       # brewery / process-automation simulation
cargo run --example warehouse     # distribution-center forklift-preemption simulation
cargo bench                       # Criterion benchmark suite (benches/simulation.rs)
cargo clippy -- -D warnings       # must stay warning-clean
```

## Source layout

```
simu/
├── Cargo.toml
├── src/
│   ├── lib.rs            # public re-exports only
│   ├── env.rs            # SimEnv + EnvHandle; owns run()/run_until() event loop
│   ├── executor/         # pub(crate), not user-facing
│   │   ├── mod.rs        # SimState: event queue, ready queue, process table, seq counter
│   │   ├── queue.rs      # ScheduledWaker, min-heap entry ordered by (time, seq)
│   │   └── waker.rs      # custom Waker that re-queues a process id on wake
│   ├── timeout.rs        # Timeout future
│   ├── event.rs          # EventTrigger / EventAwaitable (multi-waiter, fire-before-await latch)
│   ├── combinator.rs     # AnyOf, AllOf + any_of! / all_of! macros
│   ├── process.rs        # ProcessHandle<T>, spawn_with_handle
│   ├── monte_carlo.rs    # monte_carlo::run — std::thread (default) or rayon (monte-carlo feature)
│   ├── rng.rs            # RandomSource trait, portable SplitMix64 feed, sample:: transforms
│   ├── tutorial.rs       # doc-only "simu in 10 minutes" (5 chapters, all doc-tests)
│   └── resource/
│       ├── mod.rs        # Resource, ResourceRequest, ResourceGuard (FIFO)
│       ├── wait_queue.rs # pub(crate) WaitQueue<K>: shared direct-handoff waiter bookkeeping
│       ├── priority.rs   # PriorityResource (priority heap, FIFO within a level)
│       ├── container.rs  # Container (continuous quantity, FIFO put/get)
│       └── preemptive.rs # PreemptiveResource — priority pool with cooperative-at-yield preemption
├── llms.txt              # LLM-facing single-file reference (patterns copied from doc-tests)
├── docs/
│   └── simu-for-agents.md # drop-in agent-context file for downstream users
├── examples/
│   ├── intro_*.rs        # 4 beginner examples, one per tutorial chapter (<60 lines each)
│   ├── hospital.rs  / hospital.md
│   ├── brewery.rs   / brewery.md
│   ├── warehouse.rs / warehouse.md
│   └── compare.rs        # SimPy-parity harness runner (JSON contract; not a showcase — see compare/)
├── benches/
│   └── simulation.rs     # Criterion benchmarks
└── tests/                # 14 integration files: timeout, event, resource, priority_resource,
                          # container, combinator, process_handle, dropped_awaitable, system,
                          # external_feed, same_tick_races, adversarial_scheduling, preemptive_resource,
                          # scheduling_order
```

`executor/` is private (`mod executor;`) with `pub(crate)` items — not user-facing.

## Architecture

### Core design: single-threaded custom async executor per `SimEnv`

- **No tokio/async-std.** The executor is hand-rolled, polling futures based on simulated time, not
  wall-clock time. The run loop lives in `env.rs` (`run` / `run_until` → `poll_ready`); the shared
  mutable state (`SimState`) lives in `executor/mod.rs`.
- `SimEnv` is `!Send + !Sync` — it lives entirely on one thread.
- Monte Carlo parallelism is achieved by spinning up independent `SimEnv` instances on OS threads
  (`monte_carlo::run`). By default it spawns one `std::thread` per seed; enabling the `monte-carlo`
  feature switches the backend to rayon's bounded thread pool. Because each `SimEnv` is built
  *inside* the per-seed closure, its `!Send` nature is never a problem.
- DES is inherently sequential within a run, so single-threaded execution gives zero sync overhead.

### `SimEnv` / `EnvHandle` split

- **`SimEnv`** owns simulation state and drives the event loop. Not `Clone`; one thread.
- **`EnvHandle`** is a `Clone`able handle (`env.handle()`) passed into spawned processes. It exposes
  `now`, `timeout`, `event`, `spawn`, and `rng`.
- Both share the same `Rc<RefCell<SimState>>` and the same randomness source,
  `Rc<RefCell<Box<dyn RandomSource>>>` (default `StdRng`; pluggable via `SimEnv::with_source`).

### Key types

| Type | Role |
|------|------|
| `SimEnv` | Central coordinator: owns event loop, current time (`f64`), process table, pluggable `RandomSource` (default seeded `StdRng`). Not `Clone`. |
| `EnvHandle` | `Clone`able handle into the env, passed to processes (`now`/`timeout`/`event`/`spawn`/`rng`) |
| `RandomSource` / `SplitMix64` | Pluggable randomness source (`SimEnv::with_source`). `SplitMix64` is a portable feed re-implemented in Python (`compare/models/_feed.py`) for exact cross-engine comparison; `rng::sample::*` are the shared closed-form transforms |
| `Timeout` | Future that resolves after a simulated delay |
| `EventTrigger` / `EventAwaitable` | Manual inter-process signalling. `EventAwaitable` is `Clone` (multi-waiter); fire-before-await latch; `fire()` consumes the trigger |
| `Resource` / `ResourceGuard` | FIFO-queued, capacity-limited pool; RAII release on guard drop |
| `PriorityResource` | Priority-scheduled pool (lower number = higher priority; FIFO within a level) |
| `PreemptiveResource` / `PreemptiveGuard` | Priority pool whose in-use units can be evicted by a higher-priority request; cooperative-at-yield (`guard.preempted()` / `is_preempted()`) |
| `Container` | Reservoir of continuous quantity (`put` / `get`, strict head-of-line FIFO waiters) |
| `ProcessHandle<T>` | Observable spawn (tokio-`JoinHandle`-style): `await` for the value, drop to detach. Not `Clone`. |
| `AnyOf` / `AllOf` | Future combinators; built via the `any_of!` / `all_of!` macros |

### Event queue invariant

Events are ordered by simulated time, then by insertion order (`seq_counter`) for deterministic
tie-breaking, via a `BinaryHeap<Reverse<ScheduledWaker>>`. Given the same seed and process logic, a
simulation must produce identical results.

### Resource ownership (no `Arc`/`Mutex`)

`Resource` (and `PriorityResource`, `Container`) wrap `Rc<RefCell<…State>>` internally and implement
`Clone`. Share a pool across processes by **cloning the handle** — *not* by wrapping in `Arc`. No
`Mutex` is needed because the executor is single-threaded. These types are `!Send + !Sync`, consistent
with `SimEnv`.

`Resource` and `PriorityResource` share one internal `pub(crate)` helper,
`resource::wait_queue::WaitQueue<K>` (`WaitQueue<()>` = FIFO, `WaitQueue<u32>` = priority), which owns
the capacity counters and the ordered waiter heap (`try_acquire`/`register`/`release`). Release uses
**direct handoff**: the unit is transferred to the next live waiter (`granted` flag) and is never
observably free, so a same-tick fresh request cannot steal it (SPEC §4.7). `Container` keeps its own
two-sided amount-based cascade with the same commit-at-wake character.

```rust
let machine = Resource::new(1);
let m = machine.clone();           // cheap Rc clone, same pool
env.spawn(async move { let _g = m.request().await; /* … */ });
```

(One internal exception: `SimState::ready_queue` is `Arc<Mutex<Vec<usize>>>` purely so it can be held
by `Waker` vtables, which require `Send + Sync`. It is never actually contended.)

### Load-bearing internal invariants (see `SPEC.md §4.7`)

- **`registered` flag** on every request-future prevents double-queuing across polls and stops a
  spurious re-poll from stealing capacity ahead of an earlier waiter (preserves FIFO).
- **`canceled` flag** (`Rc<Cell<bool>>`) on waiter entries: a request dropped before it is granted
  (e.g. a losing `any_of!` arm) marks itself canceled; guard-release loops and the `Container` wake
  cascade skip canceled entries — no starvation, and no material leak in `Container`.
- **`RngGuard`**: `EnvHandle::rng()` returns an `impl RngCore + '_` over a `RefMut<Box<dyn RandomSource>>`.
  Being `!Send` and borrowing `self`, it cannot cross an `.await` — deterministic sampling is safe by
  construction. Sample before awaiting, and hold only one guard at a time (a second overlapping
  `env.rng()` panics with `RefCell already borrowed`).
- **`SimEnv::drop` breaks the process cycle**: a suspended process future captures an `EnvHandle`
  holding `Rc<RefCell<SimState>>`, so `SimState → processes → future → EnvHandle → SimState` is a
  reference cycle. A run that ends with processes still suspended (e.g. one blocked forever on a
  resource that never frees, as in the hospital model) would leak the whole `SimState` — and the
  leak accumulates across replications. `SimEnv`'s `Drop` clears the process tables (moving them out
  from under the borrow first, so a future's destructor can re-enter the env safely), breaking the
  cycle so each replication is reclaimed.
- **Pluggable randomness**: the source is boxed (`Box<dyn RandomSource>`) so it can be swapped without
  making `SimEnv` generic — one dyn-dispatch per draw, negligible vs. simulation work. Default is
  `StdRng`; `SimEnv::with_source(SplitMix64::new(seed))` plugs in the portable feed used for exact
  SimPy comparison. `SimEnv::set_seed` reseeds via `RandomSource::reseed`.

### Error strategy

- Programming errors (wrong API use, e.g. zero capacity) → `panic!`
- Simulation errors → `Result`

## Dependencies

| Crate | Purpose |
|-------|---------|
| `rand` + `rand_distr` | Seeded RNG and distributions (required) |
| `rayon` | Optional `monte-carlo` feature: switches `monte_carlo::run` to rayon's bounded thread pool (default backend is `std::thread`) |
| `criterion` | Benchmark harness (dev-dependency) |

`thiserror` will be added when the first public fallible API arrives; until then the surface is
panic-on-misuse only. No async runtime dependency — the executor is self-contained.

## Status

**MVP COMPLETE ✅.** All MVP features in `SPEC.md §5` are implemented, tested (133 passing tests),
and clippy-clean: `SimEnv`/event queue, `Timeout`, manual `Event` (multi-waiter + fire-before-await
latch), `Resource` (FIFO + RAII guard), `PriorityResource`, `Container`, `ProcessHandle<T>`,
`AnyOf`/`AllOf` + macros, `spawn`, `run`/`run_until`, seeded RNG, deterministic tie-breaking,
`monte_carlo::run`, the hospital and brewery examples, integration tests, and Criterion benches.

**Delivered post-MVP:**

- `PreemptiveResource` — priority pool with cooperative-at-yield preemption (`src/resource/preemptive.rs`).
- `warehouse` example — distribution center whose forklift fleet (`PreemptiveResource`) is preempted
  between receiving and shipping; the first example to exercise preemption (`examples/warehouse.rs`).
- External random feed (`src/rng.rs`) — pluggable `RandomSource` via `SimEnv::with_source`, the
  portable `SplitMix64` generator, and shared `sample::*` transforms. Re-implemented byte-for-byte in
  `compare/models/_feed.py`, so the SimPy comparison harness now runs the queue models in **exact
  mode** (per-seed metrics agree to ~1e-15). `hospital.early_discharged` remains the sole exception
  (engine eviction-handoff tie-breaking, not RNG).

**Post-MVP (not yet implemented)** — see `SPEC.md §6`:

- `Interrupt`, `RealtimeEnvironment`, `Store` / `FilterStore`, GPU/CUDA acceleration.
- Per-process panic isolation (a process panic currently unwinds the whole `run()`).
