# Coffee Shop Simulation: Example Walkthrough

> This file walks through `examples/coffee_shop.rs`. It is the smallest of the
> end-to-end examples (~200 lines) and the human-readable twin of the `coffee`
> benchmark in `examples/compare.rs`. That benchmark is checked seed-by-seed
> against SimPy (`compare/models/coffee.py`) and drives the scaling numbers in
> `compare/REPORT.md`.

## The question

A café expects about 100 customers an hour over an 8-hour day. **How many
baristas does it need?** Too few and customers wait (or walk out); too many and
staff stand idle. The answer is hard to guess because cashiers, baristas,
mobile orders and the milk supply all interact. Simulating many days at each
staffing level answers it in under a second.

## The model

```
             walk-ins (70%)                         mobile orders (30%)
                   │                                        │
   ┌───────────────▼───────────────┐                        │
   │ cashier queue (Resource)      │── patience runs out ──▶ walk-out
   └───────────────┬───────────────┘                        │
                   │ order (~1 min)                         │
   ┌───────────────▼────────────────────────────────────────▼──┐
   │ barista queue (PriorityResource): walk-ins prio 0,        │
   │                                   mobile orders prio 1    │
   └───────────────┬───────────────────────────────────────────┘
                   │ milk drink? take 0.25 L from the fridge (Container)
                   │ prepare (~2 min)
                   ▼
                 leave
```

| Element | simu type | Behaviour |
|---|---|---|
| Cashiers (`ceil(baristas / 2)`) | `Resource` | FIFO queue; a walk-in holds a cashier while ordering |
| Patience | `any_of!` | the cashier request races `timeout(patience)`; the losing arm is dropped, so a walk-out leaves the queue cleanly |
| Baristas | `PriorityResource` | walk-ins (prio 0) are served before mobile orders (prio 1); a drink in progress is never interrupted |
| Milk fridge (2 L per barista) | `Container` | `get(0.25)` waits when the fridge is dry; the milk run tops it up every 30 minutes |
| Many days | `monte_carlo::run` | 200 independent days per staffing level, in parallel |

All times are exponentially distributed with the means at the top of the file
(`ORDER_MEAN`, `PATIENCE_MEAN`, `PREP_MEAN`, …). The doors close after 480
minutes; customers already inside are still served.

## Patterns worth copying

**Request with a timeout (reneging).** `any_of!` only reports *that* one arm
finished, so the request arm parks its guard in a slot:

```rust
async fn wait_for_cashier(env: &EnvHandle, cashiers: &Resource, patience: f64) -> Option<ResourceGuard> {
    let slot = Rc::new(RefCell::new(None));
    let s = slot.clone();
    let request = cashiers.request();
    any_of![
        async move {
            *s.borrow_mut() = Some(request.await);
        },
        env.timeout(patience)
    ]
    .await; // the losing arm is dropped: a timed-out request leaves the queue
    slot.take()
}
```

**Sample before awaiting.** Each customer draws all its random numbers in one
block, so the RNG guard is released before the first `.await`.

**Scoped borrows in long-lived processes.** The milk run reads the shared
statistics inside a block (`let everyone_left = { … };`) so the `RefCell`
borrow ends before it awaits the `put`.

## Run it

```bash
cargo run --release --example coffee_shop
```

Output (seeds 0–199, deterministic):

```
200 simulated days per staffing level, ~102 customers/hour

Baristas  Cashiers    Served  Walk-outs    Time in shop  Barista use
------------------------------------------------------------------
       3         2       779       34.2        59.0 min          99%
       4         2       779       34.2         5.2 min          83%
       5         3       806        7.1         3.1 min          67%
       6         3       806        7.1         2.9 min          56%
```

Reading it:
- **3 baristas** run at 99% utilisation: the barista queue never drains and
  customers spend about an hour in the shop.
- **4 baristas** cut that to 5 minutes. This is the classic non-linearity of
  queues near full utilisation.
- **Walk-outs** depend on the *cashiers*, not the baristas. They only drop when
  the third cashier arrives (5 baristas).
- **6 baristas** buy almost nothing over 5 (2.9 vs 3.1 min) at 56% utilisation.

## Relation to the benchmark

The model that is compared against SimPy and timed in `compare/REPORT.md` is a
benchmark version of this café: [`coffee_shop_benchmark.rs`](coffee_shop_benchmark.rs).
It has the same flow, with a few changes needed for an exact cross-engine
comparison and a scalable workload:
- It uses the portable `SplitMix64` feed and the `rng::sample` transforms
  instead of `StdRng` + `rand_distr`.
- It runs a fixed number of customers (`--customers`) instead of an 8-hour day,
  so the workload can scale from 1,000 to 1,000,000 customers.
- It runs one staffing level at a time and prints the metrics the SimPy twin
  computes.

[`coffee_shop_benchmark.md`](coffee_shop_benchmark.md) lists every difference and
the reason for it. The same model also runs inside the comparison runner
`examples/compare.rs` (`--model coffee`).
