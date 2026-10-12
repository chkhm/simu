# simu vs. SimPy comparison harness

Cross-checks the `simu` Rust DES engine against Python's
[SimPy](https://simpy.readthedocs.io/) on two axes:

1. **Correctness** — treating SimPy as a reference oracle, are the two engines'
   output-metric *distributions* statistically indistinguishable across many
   seeds? (Canonical queue models are additionally checked against closed-form
   queueing theory, so neither tool has to be trusted blindly.)
2. **Performance / efficiency** — wall-clock time, events/sec, and peak resident
   memory for each engine on the same workloads.

Both engines emit the **same JSON contract**, so the harness compares them
directly. They also draw from the **same portable random feed**: the Rust
`SplitMix64` generator plus the `simu::rng::sample` transforms (`src/rng.rs`) are
re-implemented byte-for-byte in Python (`compare/models/_feed.py`). Seeded
identically per replication, the two engines draw the same numbers and turn them
into the same samples, so for the order-insensitive models the harness compares
**per-seed metrics exactly** (within ~1e-15, just floating-point summation
order) — far tighter than the old distributions-only comparison.

The one model that is **not** exact-eligible is `hospital`: its
`early_discharged` metric depends on simultaneous-event tie-breaking during the
eviction handoff, which legitimately differs between the two engines' execution
models (detailed below). That is an *engine* ordering difference, not an RNG one,
so the shared feed cannot remove it — `hospital` stays on the distributional
test.

## Layout

```
compare/
├── requirements.txt          # simpy, numpy, scipy
├── models/                   # SimPy models (one JSON-emitting script each)
│   ├── _common.py            # CLI parsing + JSON emission + seed loop
│   ├── _feed.py              # portable SplitMix64 + transforms (twin of src/rng.rs)
│   ├── test_feed.py          # cross-language known-answer test (shared with Rust)
│   ├── queue_model.py        # shared M/M/c logic
│   ├── mm1.py  mmc.py  priority_queue.py  container.py  coffee.py  hospital.py
├── harness/
│   ├── contract.py           # subprocess runners + result schema + RSS/timing
│   ├── run_correctness.py    # exact (per-seed) + distribution tests + Erlang-C ground truth
│   ├── run_performance.py    # wall-clock / events-per-sec / peak memory
│   └── report.py             # writes REPORT.md, exit non-zero on FAIL
└── run_comparison.sh         # build Rust, set up venv, run everything
```

The Rust side is `examples/compare.rs` (built with
`cargo build --release --features monte-carlo --example compare`). It shares no
code with the harness — it is a pure consumer of the public `simu` API and prints
one JSON object. The `monte-carlo` feature is only needed for the parallel Monte
Carlo benchmark (it switches `monte_carlo::run` to rayon's bounded pool); the
correctness and single-thread performance comparisons work without it.

## Running

```bash
# Full run (builds Rust, creates venv, writes compare/REPORT.md):
compare/run_comparison.sh

# Quick smoke run with fewer seeds/arrivals:
compare/run_comparison.sh --scale 0.1 --perf-scale 0.05

# Correctness only:
compare/run_comparison.sh --no-perf
```

Or drive pieces directly after `source compare/.venv/bin/activate`:

```bash
cargo build --release --features monte-carlo --example compare
PYTHONPATH=compare/harness:compare/models python compare/harness/run_correctness.py --scale 0.5
PYTHONPATH=compare/harness:compare/models python compare/harness/run_performance.py --scale 0.2
```

## Models and the JSON contract

Every run prints one line:

```json
{"tool":"simu","model":"mm1","seeds":1000,"n":1000,"lambda":0.9,"mu":1.0,
 "servers":1,"events":2000000,"wall_secs":0.12,"per_seed":[{...}, ...]}
```

| model       | primitive exercised        | per-seed metrics |
|-------------|----------------------------|------------------|
| `mm1`       | `Resource(1)`              | mean_wait, mean_sojourn, mean_queue, utilization, throughput |
| `mmc`       | `Resource(c)`              | (same) |
| `priority`  | `PriorityResource`         | mean_wait_high / _low / _all |
| `container` | `Container` (mixed sizes)  | mean_wait_all / _small / _large, served |
| `coffee`    | `Resource` + `any_of!` reneging, `PriorityResource`, `Container` (`--servers` baristas; standalone copy: `examples/coffee_shop_benchmark.rs`) | served, reneged, mean_wait_cashier / _barista / _milk, mean_time_in_shop, barista_utilization |
| `hospital`  | port of `examples/hospital.rs` | the 8 hospital metrics |

`events` uses an identical, model-specific definition on both sides (e.g. `2·N`
for the queue models: one arrival + one departure transition per customer) so
events/sec is comparable across tools.

### Metric definitions (computed identically on both sides)
- `mean_wait` — mean time in queue before service.
- `mean_queue` — mean number in system, `L = Σ sojourn / horizon` (area under
  `N(t)` equals the sum of sojourn times).
- `utilization` — busy server-time `/ (servers · horizon)`.

## Correctness methodology

Because both tools draw from the same feed, most models are checked in **exact
mode**, with the distributional test reserved for the one model that legitimately
diverges on engine tie-breaking.

**Exact mode** (`mm1`, `mmc`, `priority`, `container`). The per-seed metric
arrays from the two tools are aligned by seed and every value must match within a
tight relative tolerance (`--exact-tol`, default `1e-9`); a metric **FAILS**
otherwise. In practice the residual is ~1e-15 (floating-point summation order).

**Distributional mode** (`hospital`). The per-seed arrays are compared as
distributions:

- **Welch's t-test** on the means (`scipy.stats.ttest_ind`, `equal_var=False`).
- **Two-sample KS test** on the full distribution.
- **Relative mean difference** (effect size).

A metric **FAILS only when the difference is both statistically significant
(t-test p < α=0.01) and material (relative mean difference > tol=5%)**. This
"significant **and** material" gate avoids the large-sample trap where a
negligible difference becomes significant purely because there are many seeds.

For `mm1`/`mmc` the report also prints the closed-form Erlang-C mean wait; both
engines should land near it.

**Known exceptions.** A small allowlist of `(model, metric)` pairs
(`KNOWN_EXCEPTIONS` in `harness/run_correctness.py`) marks *accepted, documented*
divergences. A failing metric on the allowlist is rendered as `known ⚠` and the
model's status becomes `✅ PASS (N known exceptions)`; it is surfaced in the
report but does **not** fail the run, so the harness exits non-zero only on a
non-allowlisted FAIL — ready to gate CI on real regressions. The only current
entry is `hospital.early_discharged` (see below). Keep the list as small as the
evidence allows; every entry must have an explanation in this README.

### Keeping the two feeds in lockstep

`compare/models/_feed.py` is a hand-maintained twin of `src/rng.rs`. Both
`compare/models/test_feed.py` (Python) and the `rng` unit tests in `src/rng.rs`
(Rust) assert the **same** SplitMix64 known-answer table, so if either side
drifts, one of the two test suites fails. Run the Python side with
`python compare/models/test_feed.py`.

## Performance methodology

The performance report has three parts:

1. **Single-thread engine efficiency.** One large replication per model
   (`mm1`, `mmc`, `priority`, `coffee`; `hospital` is correctness-only), run
   sequentially in both tools. This isolates per-event overhead — Rust vs CPython
   — and yields the wall-clock / events-per-sec / peak-RSS table.
2. **Scaling.** The `coffee` model at 1k, 10k, 100k and 1M customers (one
   replication each), showing linear scaling and a size-independent speedup.
3. **Monte Carlo parallelism.** Many independent replications, where simu fans the
   seeds across threads via `monte_carlo::run` (rayon pool) while SimPy runs them
   sequentially — CPython's GIL prevents thread parallelism. The headline column
   is simu-parallel vs SimPy-sequential wall-clock: roughly the single-thread
   speedup multiplied by parallel scaling (saturating near the logical-core
   count). The parallel run is byte-identical to the sequential one (same seeds,
   results in seed order), so it costs no correctness. This is the axis SimPy
   structurally cannot follow.

## Resolved finding: `Container` strict-FIFO

Earlier runs of this harness surfaced a real semantic difference (not a harness
flaw): simu's `Container` granted a *freshly-arriving* `get`/`put` immediately
whenever the current level/space covered it, bypassing already-queued larger
waiters, whereas SimPy's `Container` is strict head-of-line FIFO (while a large
`get` at the front is unsatisfiable, every later `get` waits behind it, even ones
that would fit). Small draws therefore skipped ahead of blocked large draws
(small waits ≪ large waits), while SimPy makes both wait equally:

| tool  | mean_wait_small | mean_wait_large |
|-------|-----------------|-----------------|
| simu (pre-fix) | ~22    | ~149            |
| simpy          | ~59    | ~59             |

**Fixed** in `src/resource/container.rs`: the immediate-completion fast paths in
`ContainerGetRequest::poll` / `ContainerPutRequest::poll` are now gated on
`has_live_get_waiter` / `has_live_put_waiter`, so a fresh request never takes
level/space ahead of a live same-kind waiter. The `container` model now **passes**
— `mean_wait_small` ≈ `mean_wait_large` ≈ SimPy — and the Container-driven
hospital metrics (`blood_bank_waits`, `mean_blood_wait`, `critical_treated`) come
into agreement too. See the `container` section of `REPORT.md`.

## Open finding: `hospital.early_discharged` — eviction-handoff ordering

The hospital model's `early_discharged` metric diverges by ~15% (simu ~4.1 vs
SimPy ~3.5; Welch t-test p≈0.002, but **KS p≈0.22** — a means shift, not a clear
distributional difference). Unlike the now-resolved `Container` finding above,
this is **not** a semantic bug in either port and it is **independent of the
blood-bank FIFO question**: the two engines run byte-identical eviction logic, and
the gap is the same whether or not `Container` is strict-FIFO.

The shared feed makes this especially clear: now that both engines consume an
identical draw stream, every other hospital metric matches **exactly** per seed
(`critical_treated`, `standard_treated`, `blood_bank_waits`, `mean_nurse_wait`,
`mean_blood_wait` → max per-seed Δ = 0). The divergence is confined to
`early_discharged` and its small downstream cascade (`mean_bed_wait`,
`ed_cleared_at`), which is exactly what an execution-order — not RNG — difference
looks like. This is why `hospital` stays on the distributional test.

**What's the same.** Both `examples/hospital.rs` and `compare/models/hospital.py`
implement the identical preemption rule: when a *critical* patient finishes its
blood draw and all beds are occupied, it fires the longest-admitted patient's
eviction event (an entry in an `id -> Event` map), which loses its
treatment-vs-eviction race and frees the bed. Instrumenting both engines confirms
the count of "evictions fired" equals the count of `early_discharged` *exactly* in
both — so there is **no spurious early-discharge**; every early discharge is a
real, intended eviction.

**Where the gap comes from.** The difference is entirely in *how often* a critical
patient's "beds full → fire an eviction" branch is taken, and it decomposes
cleanly (200 seeds × 1000 arrivals):

| engine | crit. checks | beds full at check | evictions fired | = early_discharged |
|--------|-------------:|-------------------:|----------------:|-------------------:|
| simu   | 14.1         | 4.11               | **4.11**        | 4.11               |
| SimPy  | 14.5         | 3.91               | **3.48**        | 3.48               |

The dominant term is the **fired vs. beds-full gap**: SimPy fires on only ~89% of
beds-full checks (3.48 / 3.91); simu fires on **100%** (4.11 / 4.11). That gap is
the *eviction-handoff window*. When critical patient X evicts victim V, V is
removed from the eviction map immediately but keeps holding its bed until its
process is next scheduled and releases it. During that window the beds are still
"full" but one occupant is no longer in the map. If another critical patient
checks during the window and the map has gone empty, it finds **nobody to evict**
and simply waits for the bed that is already coming free.

SimPy's generator/callback execution (`event.succeed()` → `Condition` callback →
process resume → `Resource.release` → re-trigger) inserts several scheduling hops
between the eviction and the victim's release, *widening* that window — so the
"beds full but map empty, skip the eviction" case occurs ~0.4×/seed. simu's
poll/waker execution hands the bed off in a tighter sequence (the evicted victim
releases and the evictor re-acquires and re-registers in the map before another
critical can observe an empty map), so the window is effectively zero and the
map is never empty when beds are full. Because each fired eviction admits a new
patient who immediately re-populates the map, the effect is mildly
self-reinforcing, which is why a sub-timestep ordering difference shows up as a
~0.6-event mean shift.

**Verdict — acceptable modeling difference.** Both ports faithfully implement the
same rule; `early_discharged` is just unusually sensitive to *simultaneous-event
tie-breaking* during the eviction handoff, which legitimately differs between the
two engines' execution models. Neither tie-breaking is "more correct": a newly
arriving critical patient that evicts the longest-admitted occupant even though a
bed is already being freed (simu) is as defensible as one that waits for the
in-flight release (SimPy). Making the metric engine-agnostic would require
redesigning the model's eviction accounting (e.g. tracking in-flight evictions
explicitly) rather than fixing a port — so the harness surfaces it here instead.

Because it is accepted and well understood, `("hospital", "early_discharged")`
is on the `KNOWN_EXCEPTIONS` allowlist in `harness/run_correctness.py`: the
report still shows the divergence (as `known ⚠`), but it no longer fails the run,
so a future CI job can gate on genuine regressions. If a fix or a deliberate
model redesign ever brings it into agreement, drop that entry so the metric is
held to the normal gate again.
