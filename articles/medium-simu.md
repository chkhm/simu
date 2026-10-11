# Medium article: skeleton & fact sheet (not the article)

> **Status:** skeleton only. Every section lists *talking points, numbers, and
> snippets* for the author to write up. Nothing here is meant to be published as
> it stands. Numbers come from a fresh run on 2026-10-05 (see the appendix).

**Working title options**
- "SimPy's answers, 18× faster: discrete-event simulation in Rust with simu"
- "I rewrote SimPy's mental model in Rust. Same results, seed for seed, up to 40× faster."
- "Discrete-event simulation in Rust, without the Rust pain"

**Target reader:** SimPy users who hit runtime limits (many replications, big
models), and Rust developers who need simulation and don't want to embed Python.

**Length target:** ~1,800 words, 7–9 min read. Two charts, two or three code blocks, two tables.

**Running example: a coffee shop.** No healthcare or industrial examples. The
coffee shop works on three levels: the DES primer (§2), the showcase
(`examples/coffee_shop.rs`, §5), and the benchmark that scales from 1k to 1M
customers (§7). The EV charging station in §4 is the tutorial's own example;
swap it for a mini coffee shop if you prefer one domain throughout.

---

## 1. Hook

- Pain point: SimPy is the default for DES in Python, and it's pleasant to use.
  Slowness bites when you need many replications (confidence intervals,
  parameter sweeps, optimisation loops).
- Concrete hook numbers (coffee-shop benchmark, `compare/REPORT.md`):
  - **One run with 1 million customers: SimPy 12.5 s, simu 0.68 s.**
  - **1,000 simulated days (2,000 customers each): SimPy 25.1 s, simu 0.59 s
    on 10 cores (42.8×).**
- Promise of the article: same modelling style, same random numbers, same
  answers, 12–18× faster on one core, up to ~43× with parallel runs. Plus: why
  Rust is no longer the hard part.
- *Author's choice:* a personal sentence on why you built it (a model that was
  too slow?). A personal origin story makes the article feel genuine.

## 2. DES in 60 seconds (short primer)

- DES = model a system as **events at points in time**; the clock *jumps* from
  event to event (no fixed time step). Nothing happens between events.
- Building blocks (map each to a simu type later):
  - **Processes**: entities with behaviour (customer, barista, milk delivery) → `async` blocks
  - **Time passing**: "service takes 5 minutes" → `h.timeout(5.0).await`
  - **Resources**: capacity-limited, queue when busy → `Resource::new(2)`
  - **Randomness**: arrival/service times → seeded RNG, replications
- Picture idea: a coffee shop with 2 baristas, a queue, and a timeline of events.
  The ASCII flow diagram in `examples/coffee_shop.md` can be redrawn as the
  article's first figure.
- Why SimPy's style won: processes written as *straight-line code* (generators
  with `yield`) instead of hand-written event handlers.
- Bridge sentence idea: Rust's `async/await` is the same idea as SimPy's
  generators. A process suspends at `.await` the way it suspends at `yield`.

## 3. Meet simu

- crates.io: **`simu-des`** (the library is imported as `simu`), v0.1.0, MIT OR Apache-2.0.
  `cargo add simu-des`
- Repo: https://github.com/chkhm/simu · Docs: https://docs.rs/simu-des (start at the `tutorial` module)
- Design points worth one sentence each:
  - Own **single-threaded async executor** driven by *simulated* time. No tokio,
    no async runtime dependency.
  - **Deterministic:** events ordered by (time, insertion order), so same seed ⇒ identical run.
  - Resources are cheap `Rc` handles you `clone()`. No `Arc<Mutex>` ceremony
    (the executor is single-threaded, so it doesn't need them).
  - **RAII release:** a resource unit is returned automatically when its guard
    goes out of scope. You can't forget to release it.
  - Parallelism lives *between* replications (`monte_carlo::run`), not inside one.

## 4. Your first model: SimPy vs simu side by side

- Model: 4 electric cars, staggered arrivals, a charging station with 2 spots.
  This is the classic SimPy tutorial scenario. Both programs print **identical
  traces** (verified; output below).
- Rust (verbatim from `examples/intro_charging_station.rs`, tutorial chapter 4):

```rust
use simu::{Resource, SimEnv};

fn main() {
    let mut env = SimEnv::with_seed(42);
    let bcs = Resource::new(2); // battery charging station, 2 spots

    for i in 0..4u32 {
        let h = env.handle();
        let station = bcs.clone(); // same pool, cheap Rc clone
        env.spawn(async move {
            h.timeout(f64::from(i) * 2.0).await; // drive to the station
            println!("Car {i} arriving at {}", h.now());

            let _spot = station.request().await; // queue for a spot (FIFO)
            println!("Car {i} starting to charge at {}", h.now());

            h.timeout(5.0).await; // charge
            println!("Car {i} leaving at {}", h.now());
        }); // _spot drops here → spot handed to the next car in line
    }

    env.run();
    println!("Simulation ended at {}", env.now());
}
```

- SimPy equivalent (written for this article and verified with SimPy 4.1.2):

```python
import simpy

def car(env, i, station):
    yield env.timeout(i * 2.0)                      # drive to the station
    print(f"Car {i} arriving at {env.now}")
    with station.request() as spot:                 # queue for a spot (FIFO)
        yield spot
        print(f"Car {i} starting to charge at {env.now}")
        yield env.timeout(5.0)                      # charge
        print(f"Car {i} leaving at {env.now}")      # spot released on exit

env = simpy.Environment()
bcs = simpy.Resource(env, capacity=2)               # 2 charging spots
for i in range(4):
    env.process(car(env, i, bcs))
env.run()
print(f"Simulation ended at {env.now}")
```

- Output (both, identical except `5.0` vs `5` float formatting):

```
Car 0 arriving at 0
Car 0 starting to charge at 0
Car 1 arriving at 2
Car 1 starting to charge at 2
Car 2 arriving at 4
Car 0 leaving at 5
Car 2 starting to charge at 5
Car 3 arriving at 6
Car 1 leaving at 7
Car 3 starting to charge at 7
Car 2 leaving at 10
Car 3 leaving at 12
Simulation ended at 12
```

- Mapping table to explain (or show as a small table):

| SimPy | simu |
|---|---|
| `env.process(gen)` | `env.spawn(async move { … })` |
| `yield env.timeout(5)` | `h.timeout(5.0).await` |
| `with res.request() as r: yield r` | `let _g = res.request().await;` (released on drop) |
| `simpy.Resource(env, capacity=2)` | `Resource::new(2)`, shared via `.clone()` |
| `env.run(until=15)` | `env.run_until(15.0)` |

- Honest talking point: the Rust version is a few lines longer (`handle()`,
  `clone()`, `async move`). That boilerplate is what an agent writes for you (see §8).

## 5. What's in the box

- Resources: `Resource` (FIFO), `PriorityResource` (lower number = higher
  priority, FIFO within a level), `PreemptiveResource` (higher priority evicts
  a running holder; cooperative: the holder notices via `guard.preempted()`),
  `Container` (continuous quantity, e.g. a fuel tank, strict FIFO).
- Signalling: `env.event()` gives `(EventTrigger, EventAwaitable)`. Many waiters,
  fire once. If the event fires before anyone awaits it, later waiters still see it.
- Combinators: `any_of!` / `all_of!` (race a timeout against a signal; the losing
  branch is dropped, which is simu's equivalent of SimPy's `Interrupt`).
- `ProcessHandle<T>`: spawn a child, `.await` its return value (like a typed join).
- Pluggable randomness: default seeded `StdRng`, or `SplitMix64` for cross-engine comparisons.
- `monte_carlo::run`: replications over threads (`std::thread`, or rayon via
  the `monte-carlo` feature). Results come back in seed order, identical to a sequential run.
- Showcase examples to mention: **`coffee_shop`** (the article's running example,
  see below) and `warehouse` (logistics: a forklift fleet shared between
  receiving and shipping, the `PreemptiveResource` showcase). Each has a `.md`
  walkthrough. (Leave the other repo examples out of the article.)
- **Coffee-shop showcase** (`cargo run --release --example coffee_shop`): a
  staffing study with 200 simulated days per setting, run in parallel. Real
  output (deterministic seeds 0–199; takes <1 s for all 800 days):

```
Baristas  Cashiers    Served  Walk-outs    Time in shop  Barista use
------------------------------------------------------------------
       3         2       779       34.2        59.0 min          99%
       4         2       779       34.2         5.2 min          83%
       5         3       806        7.1         3.1 min          67%
       6         3       806        7.1         2.9 min          56%
```

  - Story: going from 3 to 4 baristas cuts time in shop from 59 to 5 minutes
    (queues explode near 100% utilisation). The 6th barista buys almost nothing.
    Walk-outs depend on the cashiers, not the baristas.
  - Primitives used: `Resource` (cashiers), `any_of!` (patience vs queue),
    `PriorityResource` (walk-ins before mobile orders), `Container` (milk
    fridge), `monte_carlo::run`.
- Optional cancellation snippet (verbatim, `examples/intro_cancellation.rs`, the core of it):

```rust
any_of![h.timeout(5.0), stop_signal].await;
```

## 6. Same answers as SimPy, and how that's proven

- Problem: "is it correct?" is the first question for any new simulator.
- Method: a comparison harness (`compare/`) runs the *same model* in both engines.
  The SplitMix64 random generator and the distribution transforms are
  re-implemented byte-for-byte in Python, so **both engines draw identical random numbers**.
- Result (200 seeds × 1,000 customers per model): M/M/1, M/M/2, priority queue,
  container, **and the coffee shop** agree **seed-by-seed** with a max
  per-seed difference of **~3e-15** (floating-point summation order).
  Statistically: t-test p = 1.000, KS p = 1.000.
- The coffee shop is the strongest proof point: reneging, priorities, and milk
  stockouts all interact, and every count still matches exactly (served 957.7,
  reneged 42.34 on average, 0 difference on every seed).

| model | metric | simu | SimPy | max per-seed Δ |
|---|---|---|---|---|
| M/M/1 (λ=0.85, μ=1) | mean wait | 5.237 | 5.237 | 2.9e-15 |
| M/M/2 (λ=1.6, μ=1) | mean wait | 1.685 | 1.685 | 2.3e-15 |
| priority (2 classes) | mean wait, high / low | 1.454 / 9.33 | 1.454 / 9.33 | ≤1.7e-15 |
| container | served | 942.4 | 942.4 | 0 |
| coffee shop (4 baristas) | mean time in shop | 5.332 min | 5.332 min | 2.8e-15 |
| coffee shop | walk-outs per 1,000 customers | 42.34 | 42.34 | 0 |

- Independent sanity check: queue models are also compared against queueing
  theory (Erlang C). Analytic M/M/1 mean wait is 5.667 vs. 5.237 simulated. The
  gap is the expected bias of short runs that start empty (1,000 customers, no
  warm-up). Explain in one sentence or skip; don't let it look like an error.
- Honesty point (optional, builds trust): exact agreement needs models where
  no two events land on the *same* timestamp in an order-sensitive way. When
  they do (e.g. one process evicting another at the same instant), the two
  engines may tie-break differently. Both orders are valid, so results then
  agree only statistically. The repo documents one such case in
  `compare/README.md`; no need to name it in the article.

## 7. The numbers

- Charts (SVG sources next to them):
  - `articles/simu-vs-simpy-speedup.png`: speedup bars, single-thread and parallel.
  - `articles/coffee-scaling.png`: coffee shop, wall time vs. customers (log-log),
    two parallel lines 18× apart. This is probably the hero image.
- Single-thread (one big replication, 100,000 customers = 200,000 events):

| model | simu | SimPy | speedup | peak memory (simu / SimPy) |
|---|---|---|---|---|
| M/M/1 | 0.039 s · 5.1 M events/s | 0.488 s · 0.41 M events/s | **12.5×** | 9 MB / 48 MB |
| M/M/2 | 0.038 s · 5.2 M events/s | 0.500 s · 0.40 M events/s | **13.0×** | 9 MB / 48 MB |
| priority | 0.041 s · 4.8 M events/s | 0.598 s · 0.33 M events/s | **14.4×** | 9 MB / 41 MB |
| coffee shop | 0.069 s · 2.9 M events/s | 1.234 s · 0.16 M events/s | **17.9×** | 12 MB / 59 MB |

- Talking point: the richer the model, the bigger the gap (12.5× for a bare
  queue, 17.9× for the coffee shop). Plausible explanation, not profiled: each
  extra SimPy operation (condition events for reneging, container get/put)
  costs interpreter time, while simu's extra work is compiled code. Phrase it
  as an observation, not a proven cause.
- Scaling (coffee shop, 4 baristas, one run each):

| customers | simu | SimPy | speedup | peak memory (simu / SimPy) |
|---|---|---|---|---|
| 1,000 | 0.7 ms | 13 ms | 17.3× | 2 MB / 32 MB |
| 10,000 | 7 ms | 122 ms | 17.5× | 3 MB / 34 MB |
| 100,000 | 68 ms | 1.25 s | 18.3× | 12 MB / 59 MB |
| 1,000,000 | 0.68 s | 12.5 s | **18.5×** | 100 MB / 334 MB |

  - Both scale linearly (constant ~2.9 M vs ~0.16 M events/s). The speedup does
    not shrink with model size. Memory: 3.3× less at 1M customers.
- Parallel replications (simu `monte_carlo::run` on 10 cores vs SimPy sequential;
  Python's GIL prevents SimPy from using threads):

| workload | simu 1 core | simu 10 cores | SimPy | simu ‖ vs SimPy |
|---|---|---|---|---|
| coffee shop, 1,000 days × 2,000 customers | 1.52 s | 0.59 s | 25.1 s | **42.8×** |
| M/M/1, 300 runs × 4,000 customers | 0.53 s | 0.25 s | 6.3 s | **25.9×** |

- Caveats to state (credibility):
  - Parallel scaling is only ~2.2–2.6× on this 10-core M4 (4 performance + 6
    efficiency cores; per-run allocation and memory bandwidth also limit it).
    Don't claim "10× from 10 cores".
  - SimPy could be parallelised with `multiprocessing`. That's fair to mention,
    but it costs process start-up and pickling, and the single-thread 12–14×
    gap remains.
  - All numbers come from one machine. Link the harness so readers can rerun it.
- Possible framing: "What takes a coffee break in SimPy finishes before you
  switch windows." Use your own words.

## 8. "But Rust is hard." Agentic coding changes the equation

Talking points, ordered from strongest to weakest:
- The traditional cost of Rust is *writing* it (lifetimes, ownership, clone/move
  dance). Coding agents now absorb most of that cost. What remains for the human
  is *reading* the model logic, which looks almost identical to SimPy (§4 table).
- The compiler acts as a **reviewer for agent-written code**. What it actually
  enforces here (verified facts only):
  - A `SimEnv` can't be accidentally shared across threads (`!Send`). Parallelism
    only happens the sanctioned way: one env per replication.
  - Ownership forces you to say explicitly which processes share a resource (`clone()`).
  - Resources are released by scope (RAII). No forgotten `release()` calls.
  - Type-checked return values from child processes (`ProcessHandle<T>`).
- **Do NOT claim** the compiler stops you holding `h.rng()` across an `.await`.
  It compiles fine and only panics at runtime if another process draws randomness
  meanwhile (verified 2026-10-05). The agent docs warn about it, so it can serve
  as an example of "docs for agents cover what the compiler can't".
- What simu ships for agents (this is the differentiator):
  - `llms.txt` (343 lines): single-file LLM reference, including a pitfalls table.
  - `docs/simu-for-agents.md` (92 lines): a drop-in context file for CLAUDE.md /
    Cursor rules / etc. ("Drop this file into your project's agent context…").
  - Rule in the repo: **every code pattern in those files is a verbatim copy of a
    doc-test**, so the agent's examples are guaranteed to compile and stay current.
  - Tutorial module on docs.rs (5 chapters) plus runnable `examples/intro_*.rs` twins.
- Suggested demonstration: a short real prompt you gave an agent ("model a
  bakery with 3 ovens…") and how many iterations it took to compile and run.
  *Only include if you actually do it.* An authentic anecdote beats a claim.
- Meta point (optional, if you're comfortable with it): simu itself was built
  with agentic coding: 52 commits since 2026-04-04, with a SimPy parity harness
  as the safety net. Decide how much to disclose.

## 9. When to stay with SimPy

- Exploratory modelling in Jupyter, teaching, quick one-offs: SimPy wins on iteration speed.
- An existing large SimPy codebase that's fast enough.
- Features simu doesn't have yet: `Interrupt` (simu uses `any_of!` racing
  instead), `Store` / `FilterStore`, real-time environment. These are on the roadmap (SPEC §6).
- Ergonomics gap, seen while building the coffee shop: SimPy's
  `yield req | env.timeout(patience)` is one line. In simu, `any_of!` returns `()`,
  so "request with a timeout" needs a small helper that parks the guard in an
  `Rc<RefCell<Option<_>>>` (`wait_for_cashier` in `examples/coffee_shop.rs`, 13
  lines). Worth admitting, and a candidate for a future API addition.
- A process panic currently aborts the whole run (no per-process isolation yet).
- Ecosystem: Python's plotting and stats stack. Possible mitigation: export
  results from Rust as CSV/JSON.

## 10. Get started / call to action

- `cargo add simu-des`, then `cargo run --example intro_charging_station`, then
  `cargo run --release --example coffee_shop`.
- docs.rs tutorial: https://docs.rs/simu-des, the "simu in 10 minutes" module
- GitHub: https://github.com/chkhm/simu (stars, issues, contributions;
  CONTRIBUTING.md, DCO sign-off)
- For agent users: copy `docs/simu-for-agents.md` into your agent context.
- Ask: "Tell me what model you'd port first", or which missing feature matters most.

---

## Appendix A: Measurement setup (for a footnote)

- Machine: Apple M4, 10 cores (4P + 6E), 16 GB RAM, macOS 26.7.1
- Rust: rustc 1.96.0-nightly (2026-02-28), `--release`, feature `monte-carlo` (rayon)
- Python: CPython 3.14.8, SimPy 4.1.2, numpy 2.5.0, scipy 1.18.0
- Python deps (fresh venv): numpy 2.5.3, scipy 1.18.1
- Date: 2026-10-05. Full report: `compare/REPORT.md` (regenerated on this date).
- Single-thread runs are sequential in one process for both tools. Wall time is
  the simulation time each tool reports itself, so it **excludes** interpreter
  and binary start-up. Memory = peak RSS of the whole process, so it includes
  the Python interpreter (~30 MB baseline).

## Appendix B: How to reproduce

```bash
compare/run_comparison.sh
```

This builds the Rust runner, creates `compare/.venv`, and writes
`compare/REPORT.md`: the correctness tables, single-thread table, coffee scaling
table, and Monte Carlo table. Every number in §6–§7 comes from that report
(full run takes ~2 minutes, mostly SimPy). A single coffee run by hand:

```bash
cargo build --release --features monte-carlo --example compare
target/release/examples/compare --model coffee --seeds 1 --n 1000000 --lambda 1.7 --mu 0.5 --servers 4
(cd compare/models && ../.venv/bin/python coffee.py --seeds 1 --n 1000000 --lambda 1.7 --mu 0.5 --servers 4)
```

## Appendix C: Pre-publication checklist

- [ ] Siemens holds the copyright. Check whether publishing needs approval or a disclosure line.
- [ ] Code blocks still match `examples/intro_charging_station.rs` / `examples/coffee_shop.rs`.
- [ ] Links resolve (crates.io, docs.rs, GitHub).
- [ ] Chart PNGs uploaded with alt text, e.g. "Bar chart: simu is 12.5–17.9× faster
      than SimPy single-threaded and 26–43× with parallel replications" and
      "Log-log line chart: coffee-shop run time grows linearly in both engines;
      at 1 million customers SimPy takes 12.5 s and simu 0.68 s".
- [ ] Medium tags (max 5): Rust, Simulation, Python, Programming, AI / Software Engineering.
- [ ] Optional: cross-post to r/rust, users.rust-lang.org, This Week in Rust (CFP/articles section).
