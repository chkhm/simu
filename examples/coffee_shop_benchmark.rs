// SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use simu::rng::{sample, SplitMix64};
use simu::{any_of, Container, EnvHandle, PriorityResource, Resource, SimEnv};

// ---------------------------------------------------------------------------
// Coffee shop, benchmark version: the model behind the SimPy comparison.
//
// This is a standalone, readable copy of `run_coffee` in `examples/compare.rs`,
// the model that `compare/run_comparison.sh` checks seed-by-seed against the
// SimPy twin `compare/models/coffee.py` and times for the performance report.
// The simulation code (constants, processes, draw order) is identical; only the
// JSON plumbing is replaced by a readable summary. See
// `examples/coffee_shop_benchmark.md` for how it differs from the showcase
// `examples/coffee_shop.rs`, and why.
//
// Usage:
//   cargo run --release --example coffee_shop_benchmark
//   cargo run --release --example coffee_shop_benchmark -- --customers 1000000
//   cargo run --release --example coffee_shop_benchmark -- --seeds 1000 --customers 2000 --parallel
//
// Units: minutes.
// ---------------------------------------------------------------------------

const ARRIVALS_PER_MIN: f64 = 1.7; // `--lambda` in the comparison harness
const PREP_RATE: f64 = 0.5; // `--mu`: prep time ~ Exp(mean 1 / 0.5 = 2 min)

const MOBILE_P: f64 = 0.3;
const ORDER_MEAN: f64 = 1.0;
const PATIENCE_MEAN: f64 = 5.0;
const MILK_P: f64 = 0.6;
const MILK_PER_DRINK: f64 = 0.25;
const MILK_CAP_PER_BARISTA: f64 = 2.0;
const RESTOCK_INTERVAL: f64 = 30.0;

#[derive(Clone)]
struct Shop {
    cashier: Resource,
    barista: PriorityResource,
    milk: Container,
    recs: Rc<RefCell<Vec<Rec>>>,
    reneged: Rc<RefCell<u64>>,
    departed: Rc<RefCell<u64>>,
}

/// What we record for each served customer.
struct Rec {
    walk_in: bool,
    milk: bool,
    wait_cashier: f64,
    wait_barista: f64,
    wait_milk: f64,
    busy: f64,
    in_shop: f64,
    departure: f64,
}

/// All random values of one customer, drawn at arrival.
struct Customer {
    mobile: bool,
    order: f64,
    patience: f64,
    prep: f64,
    milk: bool,
}

async fn customer(env: EnvHandle, shop: Shop, c: Customer) {
    let arrival = env.now();
    let mut wait_cashier = 0.0;
    if !c.mobile {
        // Race the cashier queue against the customer's patience. The request
        // arm parks its guard in `slot`; an empty slot means patience won.
        let slot = Rc::new(RefCell::new(None));
        let s = slot.clone();
        let req = shop.cashier.request();
        any_of![
            async move {
                *s.borrow_mut() = Some(req.await);
            },
            env.timeout(c.patience)
        ]
        .await;
        let Some(cashier) = slot.take() else {
            *shop.reneged.borrow_mut() += 1;
            *shop.departed.borrow_mut() += 1;
            return;
        };
        wait_cashier = env.now() - arrival;
        env.timeout(c.order).await;
        drop(cashier);
    }

    let queued = env.now();
    let barista = shop.barista.request(if c.mobile { 1 } else { 0 }).await;
    let start = env.now();
    if c.milk {
        shop.milk.get(MILK_PER_DRINK).await;
    }
    let wait_milk = env.now() - start;
    env.timeout(c.prep).await;
    drop(barista);

    let departure = env.now();
    shop.recs.borrow_mut().push(Rec {
        walk_in: !c.mobile,
        milk: c.milk,
        wait_cashier,
        wait_barista: start - queued,
        wait_milk,
        busy: departure - start,
        in_shop: departure - arrival,
        departure,
    });
    *shop.departed.borrow_mut() += 1;
}

/// Spawn `n` customers with Exp(1/lambda) gaps, drawing every random value at arrival.
async fn arrivals(env: EnvHandle, shop: Shop, n: u64, lambda: f64, mu: f64) {
    for _ in 0..n {
        let inter_arrival = sample::exponential(&mut env.rng(), 1.0 / lambda);
        env.timeout(inter_arrival).await;
        let c = {
            let mut rng = env.rng();
            Customer {
                mobile: sample::bernoulli(&mut rng, MOBILE_P),
                order: sample::exponential(&mut rng, ORDER_MEAN),
                patience: sample::exponential(&mut rng, PATIENCE_MEAN),
                prep: sample::exponential(&mut rng, 1.0 / mu),
                milk: sample::bernoulli(&mut rng, MILK_P),
            }
        };
        env.spawn(customer(env.clone(), shop.clone(), c));
    }
}

/// Top the milk fridge up to capacity every interval until every customer has left.
async fn restock(env: EnvHandle, shop: Shop, n: u64) {
    loop {
        env.timeout(RESTOCK_INTERVAL).await;
        if *shop.departed.borrow() == n {
            break;
        }
        let top_up = shop.milk.capacity() - shop.milk.level();
        if top_up > 0.0 {
            shop.milk.put(top_up).await;
        }
    }
}

/// The per-run metrics, computed exactly like the SimPy twin computes them.
struct Metrics {
    served: f64,
    reneged: f64,
    mean_wait_cashier: f64,
    mean_wait_barista: f64,
    mean_wait_milk: f64,
    mean_time_in_shop: f64,
    barista_utilization: f64,
}

/// One replication: `n` customers, `servers` baristas, randomness from `SplitMix64(seed)`.
fn run_coffee(seed: u64, n: u64, lambda: f64, mu: f64, servers: usize) -> Metrics {
    let mut env = SimEnv::with_source(SplitMix64::new(seed));
    let milk_cap = MILK_CAP_PER_BARISTA * servers as f64;
    let shop = Shop {
        cashier: Resource::new(servers.div_ceil(2)),
        barista: PriorityResource::new(servers),
        milk: Container::new(milk_cap, milk_cap),
        recs: Rc::new(RefCell::new(Vec::with_capacity(n as usize))),
        reneged: Rc::new(RefCell::new(0)),
        departed: Rc::new(RefCell::new(0)),
    };

    env.spawn(arrivals(env.handle(), shop.clone(), n, lambda, mu));
    env.spawn(restock(env.handle(), shop.clone(), n));
    env.run();

    let reneged = *shop.reneged.borrow();
    let recs = shop.recs.borrow();
    let mean = |xs: Vec<f64>| -> f64 {
        if xs.is_empty() {
            0.0
        } else {
            xs.iter().sum::<f64>() / xs.len() as f64
        }
    };
    let t_end = recs.iter().map(|r| r.departure).fold(0.0_f64, f64::max);
    let busy: f64 = recs.iter().map(|r| r.busy).sum();
    let utilization = if t_end > 0.0 {
        busy / (servers as f64 * t_end)
    } else {
        0.0
    };

    Metrics {
        served: recs.len() as f64,
        reneged: reneged as f64,
        mean_wait_cashier: mean(
            recs.iter()
                .filter(|r| r.walk_in)
                .map(|r| r.wait_cashier)
                .collect(),
        ),
        mean_wait_barista: mean(recs.iter().map(|r| r.wait_barista).collect()),
        mean_wait_milk: mean(
            recs.iter()
                .filter(|r| r.milk)
                .map(|r| r.wait_milk)
                .collect(),
        ),
        mean_time_in_shop: mean(recs.iter().map(|r| r.in_shop).collect()),
        barista_utilization: utilization,
    }
}

// ---------------------------------------------------------------------------
// Command line + readable summary (replaces compare.rs's JSON output)
// ---------------------------------------------------------------------------

struct Args {
    customers: u64,
    seeds: u64,
    baristas: usize,
    parallel: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        customers: 100_000,
        seeds: 1,
        baristas: 4,
        parallel: false,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        if argv[i] == "--parallel" {
            args.parallel = true;
            i += 1;
            continue;
        }
        let val = argv.get(i + 1).cloned().unwrap_or_default();
        match argv[i].as_str() {
            "--customers" => args.customers = val.parse().expect("--customers expects an integer"),
            "--seeds" => args.seeds = val.parse().expect("--seeds expects an integer"),
            "--baristas" => args.baristas = val.parse().expect("--baristas expects an integer"),
            other => panic!(
                "unknown argument: {other} (use --customers, --seeds, --baristas, --parallel)"
            ),
        }
        i += 2;
    }
    args
}

fn main() {
    let a = parse_args();
    let run = |seed: u64| run_coffee(seed, a.customers, ARRIVALS_PER_MIN, PREP_RATE, a.baristas);

    let t0 = Instant::now();
    let results: Vec<Metrics> = if a.parallel {
        simu::monte_carlo::run(0..a.seeds, run)
    } else {
        (0..a.seeds).map(run).collect()
    };
    let wall = t0.elapsed().as_secs_f64();

    let n = results.len() as f64;
    let mean = |f: fn(&Metrics) -> f64| results.iter().map(f).sum::<f64>() / n;
    let events = 2 * a.customers * a.seeds; // one arrival + one departure (or walk-out) per customer

    println!(
        "{} run(s) × {} customers, {} baristas, {} cashiers ({})\n",
        a.seeds,
        a.customers,
        a.baristas,
        a.baristas.div_ceil(2),
        if a.parallel { "parallel" } else { "sequential" },
    );
    let label = if a.seeds == 1 {
        "seed 0"
    } else {
        "mean over seeds"
    };
    println!("{label}:");
    println!("  served               {:.2}", mean(|m| m.served));
    println!("  reneged (walk-outs)  {:.2}", mean(|m| m.reneged));
    println!(
        "  mean_wait_cashier    {:.6} min",
        mean(|m| m.mean_wait_cashier)
    );
    println!(
        "  mean_wait_barista    {:.6} min",
        mean(|m| m.mean_wait_barista)
    );
    println!(
        "  mean_wait_milk       {:.6} min",
        mean(|m| m.mean_wait_milk)
    );
    println!(
        "  mean_time_in_shop    {:.6} min",
        mean(|m| m.mean_time_in_shop)
    );
    println!(
        "  barista_utilization  {:.6}",
        mean(|m| m.barista_utilization)
    );
    println!(
        "\nwall time {wall:.3} s · {:.2} M events/s",
        events as f64 / wall / 1e6
    );
}
