# Coffee Shop Benchmark: Differences From the Showcase

> This file explains `examples/coffee_shop_benchmark.rs`: the version of the
> coffee shop that is compared against SimPy and timed in `compare/REPORT.md`.
> It models the same café as the showcase `examples/coffee_shop.rs` (walkthrough:
> `coffee_shop.md`), but a few details are changed so the model can be
> **checked seed-by-seed against SimPy** and **scaled from 1,000 to 1,000,000
> customers**. This document lists every difference and the reason for it.

## Where this code comes from

The comparison harness runs the coffee model through `examples/compare.rs`, a
runner that hosts all comparison models (M/M/1, M/M/c, priority queue,
container, coffee, hospital) behind one command line and prints JSON for the
Python harness. That makes the coffee model hard to read in isolation.
`coffee_shop_benchmark.rs` is a standalone copy of its `run_coffee` model:

- **Identical simulation code:** the same constants, the same three processes
  (`arrivals`, `customer`, `restock`), and the same order of random draws.
  Only the `Cf`/`CF_` name prefixes (needed inside `compare.rs` to avoid name
  clashes) are dropped.
- **Readable output instead of JSON:** the metrics are printed as a summary,
  with wall time and events per second.

## The SimPy twin

The same model in Python/SimPy has two copies, mirroring the Rust side:

- `compare/models/coffee.py` is the version the comparison harness runs (JSON
  output, shared command-line code, random generator imported from `_feed.py`).
- [`compare/standalone/coffee_shop_benchmark.py`](../compare/standalone/coffee_shop_benchmark.py)
  is a standalone, readable copy for humans. It contains the SplitMix64
  generator and its two sampling transforms in the same file, so you can see
  where every random number comes from, and it has the same command line and
  output as the Rust example. It only needs `pip install simpy`.

Run the two readable versions side by side:

```bash
cargo run --release --example coffee_shop_benchmark -- --customers 2000
```

```bash
python3 compare/standalone/coffee_shop_benchmark.py --customers 2000
```

Both print, for seed 0: 1,917 served, 83 walk-outs, mean time in shop
4.074548 minutes, and identical values for every other metric. Everything
except the final wall-time line is the same, character for character. The
Python version has no `--parallel` option: CPython's global interpreter lock
prevents running replications on several threads, so they always run one
after another.

## What is the same

The café itself does not change:

| Element | simu type | Behaviour |
|---|---|---|
| Cashiers (`ceil(baristas / 2)`) | `Resource` | FIFO queue; a walk-in holds a cashier while ordering (~1 min) |
| Patience (~5 min) | `any_of!` | the cashier request races `timeout(patience)`; if patience wins, the customer walks out |
| Baristas | `PriorityResource` | walk-ins (priority 0) before mobile orders (priority 1); prep ~2 min |
| Milk fridge (2 L per barista) | `Container` | 60% of drinks take 0.25 L; `get` waits when the fridge is dry |
| Milk run | process | tops the fridge up to capacity every 30 minutes |
| Arrivals | process | ~1.7 customers per minute, 30% of them mobile orders |

## What is different, and why

| | Showcase `coffee_shop.rs` | Benchmark `coffee_shop_benchmark.rs` | Why |
|---|---|---|---|
| Random number generator | `SimEnv::with_seed(seed)` → `StdRng` from the `rand` crate | `SimEnv::with_source(SplitMix64::new(seed))` | SplitMix64 is simple enough to re-implement exactly in Python (`compare/models/_feed.py`), so both engines draw the **same** random numbers for the same seed |
| Turning random numbers into samples | `rand_distr::Exp`, `random_bool` | `simu::rng::sample::exponential`, `sample::bernoulli` | these closed-form transforms are also re-implemented exactly in Python; `rand_distr`'s algorithms are not, so they would give different samples |
| Order of random draws | one block per customer, at arrival | the same, in a fixed order: gap, mobile?, order time, patience, prep time, milk? | drawing everything at arrival makes the sequence independent of how the two engines interleave their processes, which is what makes seed-by-seed agreement possible |
| Length of a run | one 8-hour day: the door closes at 480 minutes | a fixed number of customers (`--customers`, default 100,000) | the workload must scale (1k → 1M customers for the scaling chart), and a fixed count gives both engines exactly the same amount of work |
| When the milk run stops | when the shop has closed and every customer has left | when all `n` customers have left | follows from the run length above |
| Staffing levels | loops over 3–6 baristas | one `--baristas` value per run (default 4) | the benchmark measures one configuration at a time |
| Many runs | always 200 days per staffing level via `monte_carlo::run` | `--seeds` runs, sequential by default, `--parallel` uses `monte_carlo::run` | sequential runs measure single-core speed; parallel runs measure the Monte Carlo speedup. The results are identical either way |
| Output | staffing table (served, walk-outs, time in shop, barista use) | the per-run metrics the harness compares, plus wall time and events per second | these are the exact metrics `compare/models/coffee.py` computes |

## What this means for the numbers

- The staffing table printed by `coffee_shop.rs` and the numbers printed by
  this benchmark are **not directly comparable**. The random generator, the
  sampling and the run length differ, so they are different random
  experiments of the same café.
- What is cross-checked against SimPy is **this benchmark version**. The
  showcase uses the same simu building blocks and the same model logic, but it
  is not run through the SimPy comparison itself.

## Running it

One long day with 100,000 customers (single core):

```bash
cargo run --release --example coffee_shop_benchmark
```

The scaling series from the article (1k, 10k, 100k, 1M customers):

```bash
cargo run --release --example coffee_shop_benchmark -- --customers 1000000
```

The Monte Carlo benchmark: 1,000 runs of 2,000 customers on all CPU cores (build
with the `monte-carlo` feature for rayon's thread pool):

```bash
cargo run --release --features monte-carlo --example coffee_shop_benchmark -- --seeds 1000 --customers 2000 --parallel
```

The official numbers in `compare/REPORT.md` are produced by
`compare/run_comparison.sh`, which runs `examples/compare.rs` and the SimPy
twin side by side. Wall times reported by this example are measured the same
way (simulation time only, excluding program start-up), but can vary from run
to run.
