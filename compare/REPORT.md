# simu vs. SimPy comparison report

_Generated 2026-10-05T20:17:31_

## Correctness: simu vs. SimPy

Both tools draw from the same portable feed (SplitMix64 + shared transforms), seeded identically per replication.

- **Exact mode** (mm1, mmc, priority, container, coffee): every metric must agree **seed-by-seed** within 1e-9 relative; FAIL otherwise.
- **Distributional mode** (hospital): FAIL = statistically significant (t-test p < 0.01) **and** material (relative mean difference > 5%).
- **Known exceptions** (marked `known ⚠`): accepted, documented divergences that are surfaced but do **not** fail the run — currently `hospital.early_discharged` (eviction-handoff event ordering; see README). The harness exits non-zero only on a non-allowlisted FAIL.

### mm1  (✅ PASS, exact) — 200 seeds × 1000 arrivals, λ=0.85, μ=1.0, c=1

| metric | simu mean | simpy mean | rel.diff | max/seed Δ | t-test p | KS p | result |
|---|---|---|---|---|---|---|---|
| mean_queue | 5.299 ± 0.15 | 5.299 ± 0.15 | 0.00% | 2.9e-15 | 1.000 | 1.000 | pass |
| mean_sojourn | 6.232 ± 0.16 | 6.232 ± 0.16 | 0.00% | 3.0e-15 | 1.000 | 1.000 | pass |
| mean_wait | 5.237 ± 0.16 | 5.237 ± 0.16 | 0.00% | 2.9e-15 | 1.000 | 1.000 | pass |
| throughput | 0.8457 ± 0.0018 | 0.8457 ± 0.0018 | 0.00% | 0.0e+00 | 1.000 | 1.000 | pass |
| utilization | 0.8422 ± 0.0028 | 0.8422 ± 0.0028 | 0.00% | 2.2e-15 | 1.000 | 1.000 | pass |

_Analytical mean wait (Erlang C) = **5.667**; simu = 5.237, simpy = 5.237._

### mmc  (✅ PASS, exact) — 200 seeds × 1000 arrivals, λ=1.6, μ=1.0, c=2

| metric | simu mean | simpy mean | rel.diff | max/seed Δ | t-test p | KS p | result |
|---|---|---|---|---|---|---|---|
| mean_queue | 4.284 ± 0.079 | 4.284 ± 0.079 | 0.00% | 2.5e-15 | 1.000 | 1.000 | pass |
| mean_sojourn | 2.681 ± 0.046 | 2.681 ± 0.046 | 0.00% | 2.5e-15 | 1.000 | 1.000 | pass |
| mean_wait | 1.685 ± 0.044 | 1.685 ± 0.044 | 0.00% | 2.3e-15 | 1.000 | 1.000 | pass |
| throughput | 1.593 ± 0.0034 | 1.593 ± 0.0034 | 0.00% | 0.0e+00 | 1.000 | 1.000 | pass |
| utilization | 0.793 ± 0.0026 | 0.793 ± 0.0026 | 0.00% | 2.2e-15 | 1.000 | 1.000 | pass |

_Analytical mean wait (Erlang C) = **1.778**; simu = 1.685, simpy = 1.685._

### priority  (✅ PASS, exact) — 200 seeds × 1000 arrivals, λ=0.85, μ=1.0, c=1

| metric | simu mean | simpy mean | rel.diff | max/seed Δ | t-test p | KS p | result |
|---|---|---|---|---|---|---|---|
| mean_wait_all | 5.373 ± 0.16 | 5.373 ± 0.16 | 0.00% | 2.6e-15 | 1.000 | 1.000 | pass |
| mean_wait_high | 1.454 ± 0.015 | 1.454 ± 0.015 | 0.00% | 5.4e-16 | 1.000 | 1.000 | pass |
| mean_wait_low | 9.33 ± 0.32 | 9.33 ± 0.32 | 0.00% | 1.7e-15 | 1.000 | 1.000 | pass |

### container  (✅ PASS, exact) — 200 seeds × 1000 arrivals, λ=0.9, μ=1.0, c=1

| metric | simu mean | simpy mean | rel.diff | max/seed Δ | t-test p | KS p | result |
|---|---|---|---|---|---|---|---|
| mean_wait_all | 31.2 ± 1.6 | 31.2 ± 1.6 | 0.00% | 2.2e-15 | 1.000 | 1.000 | pass |
| mean_wait_large | 31.84 ± 1.6 | 31.84 ± 1.6 | 0.00% | 1.1e-15 | 1.000 | 1.000 | pass |
| mean_wait_small | 30.93 ± 1.6 | 30.93 ± 1.6 | 0.00% | 1.8e-15 | 1.000 | 1.000 | pass |
| served | 942.4 ± 2.4 | 942.4 ± 2.4 | 0.00% | 0.0e+00 | 1.000 | 1.000 | pass |

### coffee  (✅ PASS, exact) — 200 seeds × 1000 arrivals, λ=1.7, μ=0.5, c=4

| metric | simu mean | simpy mean | rel.diff | max/seed Δ | t-test p | KS p | result |
|---|---|---|---|---|---|---|---|
| barista_utilization | 0.8366 ± 0.0032 | 0.8366 ± 0.0032 | 0.00% | 2.4e-15 | 1.000 | 1.000 | pass |
| mean_time_in_shop | 5.332 ± 0.12 | 5.332 ± 0.12 | 0.00% | 2.8e-15 | 1.000 | 1.000 | pass |
| mean_wait_barista | 2.382 ± 0.11 | 2.382 ± 0.11 | 0.00% | 2.8e-15 | 1.000 | 1.000 | pass |
| mean_wait_cashier | 0.2688 ± 0.0031 | 0.2688 ± 0.0031 | 0.00% | 2.8e-16 | 1.000 | 1.000 | pass |
| mean_wait_milk | 0.1271 ± 0.0068 | 0.1271 ± 0.0068 | 0.00% | 2.2e-16 | 1.000 | 1.000 | pass |
| reneged | 42.34 ± 0.55 | 42.34 ± 0.55 | 0.00% | 0.0e+00 | 1.000 | 1.000 | pass |
| served | 957.7 ± 0.55 | 957.7 ± 0.55 | 0.00% | 0.0e+00 | 1.000 | 1.000 | pass |

### hospital  (✅ PASS (1 known exception), distributional) — 200 seeds × 1000 arrivals, λ=0.9, μ=1.0, c=1

| metric | simu mean | simpy mean | rel.diff | max/seed Δ | t-test p | KS p | result |
|---|---|---|---|---|---|---|---|
| blood_bank_waits | 17.52 ± 0.63 | 17.52 ± 0.63 | 0.00% | 0.0e+00 | 1.000 | 1.000 | pass |
| critical_treated | 14.62 ± 0.12 | 14.62 ± 0.12 | 0.00% | 0.0e+00 | 1.000 | 1.000 | pass |
| early_discharged | 3.965 ± 0.14 | 3.43 ± 0.14 | 13.49% | 1.0e+00 | 0.006 | 0.205 | known ⚠ |
| ed_cleared_at | 526.7 ± 2.1 | 527.8 ± 2.1 | 0.22% | 1.2e-01 | 0.695 | 1.000 | pass |
| mean_bed_wait | 4.044 ± 0.25 | 4.217 ± 0.25 | 4.11% | 7.0e-01 | 0.623 | 0.906 | pass |
| mean_blood_wait | 24.84 ± 1.5 | 24.84 ± 1.5 | 0.00% | 2.9e-16 | 1.000 | 1.000 | pass |
| mean_nurse_wait | 3.814 ± 0.14 | 3.814 ± 0.14 | 0.00% | 3.4e-16 | 1.000 | 1.000 | pass |
| standard_treated | 34.35 ± 0.49 | 34.35 ± 0.49 | 0.00% | 0.0e+00 | 1.000 | 1.000 | pass |

_Note: differences here are **expected**, not a bug. Both engines draw the identical random stream, so every metric that depends on the random values matches exactly. The diverging metrics (`early_discharged`, and its cascade into `mean_bed_wait` / `ed_cleared_at`) depend instead on the **order in which events scheduled at the same simulated timestamp fire** — which is implementation-specific and differs between simu (poll/waker) and SimPy (generator/callback) during the bed-eviction handoff. No random number is consumed at that decision point, so a shared feed cannot remove the difference. See `compare/README.md`._

## Performance: simu vs. SimPy

Each model runs one large replication (`seeds=1`, large `n`). The coffee model is the coffee shop (4 baristas, 2 cashiers, reneging, milk fridge). The `events` column is the common, model-specific event tally and is identical across the two tools by construction.

| model | workload | events | tool | wall (s) | events/sec | peak RSS (MB) |
|---|---|---|---|---|---|---|
| mm1 | 100,000 arrivals | 200,000 | simu | 0.039 | 5,137,503 | 9.3 |
| mm1 | 100,000 arrivals | 200,000 | simpy | 0.488 | 410,251 | 48.2 |
| mmc | 100,000 arrivals | 200,000 | simu | 0.038 | 5,207,356 | 9.3 |
| mmc | 100,000 arrivals | 200,000 | simpy | 0.500 | 399,749 | 48.2 |
| priority | 100,000 arrivals | 200,000 | simu | 0.041 | 4,824,082 | 8.6 |
| priority | 100,000 arrivals | 200,000 | simpy | 0.598 | 334,655 | 40.7 |
| coffee | 100,000 arrivals | 200,000 | simu | 0.069 | 2,898,983 | 12.3 |
| coffee | 100,000 arrivals | 200,000 | simpy | 1.234 | 162,082 | 59.0 |

| model | simu speedup × | simu memory advantage × |
|---|---|---|
| mm1 | 12.5× | 5.2× |
| mmc | 13.0× | 5.2× |
| priority | 14.4× | 4.7× |
| coffee | 17.9× | 4.8× |

_Note: all seeds run **sequentially** in a single process for both tools — these figures measure single-thread engine efficiency, not parallelism. (simu can parallelize independent seeds across cores via `monte_carlo::run`, which CPython's GIL effectively denies SimPy, but that is intentionally not exercised here.)_

## Scaling: coffee shop from 1k to 1M customers

One replication of the coffee model (4 baristas, λ=1.7/min, mean prep 2 min) at growing day lengths. Both engines should scale linearly (constant events/sec); the speedup column shows whether simu's advantage holds at every size.

| customers | events | simu wall (s) | SimPy wall (s) | simu events/sec | SimPy events/sec | simu peak RSS (MB) | SimPy peak RSS (MB) | speedup × |
|---|---|---|---|---|---|---|---|---|
| 1,000 | 2,000 | 0.0007 | 0.013 | 2,718,932 | 157,022 | 2.0 | 31.9 | 17.3× |
| 10,000 | 20,000 | 0.0070 | 0.122 | 2,854,883 | 163,459 | 2.9 | 34.4 | 17.5× |
| 100,000 | 200,000 | 0.0682 | 1.246 | 2,933,232 | 160,473 | 12.3 | 59.0 | 18.3× |
| 1,000,000 | 2,000,000 | 0.6756 | 12.493 | 2,960,288 | 160,095 | 100.4 | 333.9 | 18.5× |

## Monte Carlo: parallel seeds (simu) vs sequential (SimPy)

simu fans independent replications across threads via `monte_carlo::run` (rayon; 10 logical cores on this machine); SimPy runs them sequentially because CPython's GIL prevents thread parallelism. Wall-clock is the total time to complete **all** replications. Output is byte-identical to the sequential run (same seeds, results returned in seed order).

| model | replications | simu seq (s) | simu ‖ (s) | SimPy seq (s) | simu parallel speedup × | simu ‖ vs SimPy × |
|---|---|---|---|---|---|---|
| coffee | 1,000 seeds × 2,000 | 1.518 | 0.587 | 25.087 | 2.6× | 42.8× |
| mm1 | 300 seeds × 4,000 | 0.533 | 0.245 | 6.337 | 2.2× | 25.9× |

_The **simu ‖ vs SimPy** column is the headline cross-tool advantage when replications run in parallel: roughly the single-thread per-event speedup times the parallel scaling (which saturates near the ~10× logical-core count and is trimmed by per-run setup and memory bandwidth). This is the axis SimPy cannot follow — its replications are GIL-serialised._

