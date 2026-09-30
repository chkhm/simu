# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `EventAwaitable::is_fired()` reports whether an event has fired without
  awaiting it, e.g. to learn which arm of an `any_of!` race won.

### Fixed

- `SimEnv::run_until(t)` no longer runs an event due exactly at `t`. Its
  documentation (and SimPy's `run(until=t)`) already said so, but the loop
  stopped only on events strictly after `t`. A model that relied on the old
  behaviour can run until the next representable time, `t.next_up()` (Rust 1.86+).

## [0.1.0] - 2026-08-23

Initial release, published as [`simu-des`](https://crates.io/crates/simu-des)
(imported as `use simu::…`).

### Added

- **Custom single-threaded async executor** per `SimEnv` — no tokio/async-std;
  futures are polled on simulated time with deterministic `(time, seq)`
  tie-breaking, so equal seeds give identical runs.
- `SimEnv` / `EnvHandle` split: the env owns the event loop (`run` /
  `run_until`); the cloneable handle gives processes `now`, `timeout`, `event`,
  `spawn`, and `rng`.
- `Timeout` future for simulated delays.
- `EventTrigger` / `EventAwaitable` — manual inter-process signalling with
  multi-waiter support and a fire-before-await latch.
- Resources (all `Rc`-cloneable, no `Arc`/`Mutex` needed):
  - `Resource` — FIFO-queued, capacity-limited pool with RAII `ResourceGuard`
    release and direct-handoff semantics (a freed unit can never be stolen by a
    same-tick fresh request).
  - `PriorityResource` — priority-scheduled pool (lower number = higher
    priority; FIFO within a level).
  - `PreemptiveResource` — priority pool with cooperative-at-yield preemption
    (`guard.preempted()` / `is_preempted()`).
  - `Container` — reservoir of continuous quantity with strict head-of-line
    FIFO `put` / `get`.
- `ProcessHandle<T>` — observable spawn (tokio-`JoinHandle`-style): await the
  value or drop to detach.
- `AnyOf` / `AllOf` combinators with `any_of!` / `all_of!` macros.
- Seeded, pluggable randomness: `RandomSource` trait (`SimEnv::with_source`),
  default `StdRng`, portable `SplitMix64` feed, and shared closed-form
  `sample::*` transforms.
- `monte_carlo::run` — parallel replications on `std::thread` by default, or
  rayon's bounded pool via the `monte-carlo` feature.
- Examples: `hospital` (ER patient flow), `brewery` (process automation),
  `warehouse` (forklift preemption, with browser visualization).
- SimPy-parity harness (`compare/`, repo only — excluded from the crate):
  queue models match SimPy seed-by-seed to ~1e-15 in exact mode.
- Criterion benchmark suite; 126 tests across unit + integration suites
  (145 including doc-tests).
- **Documentation**: `simu::tutorial` — a "simu in 10 minutes" module modeled on
  SimPy's intro (5 chapters, every snippet a doc-test) with four sub-60-line
  `intro_*` examples; doc-examples on all core types (19 doc-tests total);
  `llms.txt` + `docs/simu-for-agents.md` for AI coding assistants, including a
  SimPy → simu translation table.

[Unreleased]: https://github.com/chkhm/simu/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/chkhm/simu/releases/tag/v0.1.0
