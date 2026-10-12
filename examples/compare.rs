// SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Machine-readable comparison runner for the simu-vs-SimPy harness.
//!
//! Runs one of several models for a range of seeds and prints a single JSON
//! object on stdout describing per-seed output metrics plus wall-clock time and
//! an event count. The Python side (`compare/models/*.py`) emits the **same**
//! JSON contract so the harness can compare distributions across many seeds and
//! measure relative performance. See `compare/README.md`.
//!
//! Randomness comes from the portable [`SplitMix64`] feed plus the shared
//! [`sample`] transforms, both re-implemented byte-for-byte in
//! `compare/models/_feed.py`. Seeded with the same value, the Rust and Python
//! runs draw the same numbers and turn them into the same samples, so the
//! harness can compare per-seed metrics exactly (not just in distribution).
//!
//! Usage:
//!   cargo run --release --example compare -- \
//!       --model mm1 --seeds 1000 --n 1000 --lambda 0.9 --mu 1.0 --servers 1
//!
//! Models:
//!   mm1       M/M/1 queue (single server)
//!   mmc       M/M/c queue (`--servers c`)
//!   priority  two-class priority queue (PriorityResource, 50/50 split)
//!   container Container head-of-line FIFO stress test
//!   coffee    coffee shop: cashiers, reneging, priority baristas, milk Container
//!             (`--servers` baristas; scalable benchmark)
//!   hospital  faithful port of examples/hospital.rs (ignores --n/--lambda/--mu)
//!
//! Flags:
//!   --parallel  run the seed replications across threads via monte_carlo::run
//!               (build with `--features monte-carlo` for the rayon pool). Output
//!               is identical to the sequential run; only wall-clock differs.
//!
//! JSON is hand-written (no serde), mirroring the JSONL approach already used by
//! examples/warehouse.rs.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Instant;

use simu::rng::{sample, SplitMix64};
use simu::{any_of, Container, EnvHandle, EventTrigger, PriorityResource, Resource, SimEnv};

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

struct Args {
    model: String,
    seeds: u64,
    n: u64,
    lambda: f64,
    mu: f64,
    servers: usize,
    /// Run the seed replications in parallel via `monte_carlo::run` instead of
    /// the default sequential loop. Build with `--features monte-carlo` so the
    /// parallelism uses rayon's bounded pool (one std::thread per seed otherwise).
    parallel: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        model: "mm1".to_string(),
        seeds: 1,
        n: 1000,
        lambda: 0.9,
        mu: 1.0,
        servers: 1,
        parallel: false,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let key = argv[i].as_str();
        // `--parallel` is a valueless flag; everything else is `--key value`.
        if key == "--parallel" {
            args.parallel = true;
            i += 1;
            continue;
        }
        let val = argv.get(i + 1).cloned().unwrap_or_default();
        match key {
            "--model" => args.model = val,
            "--seeds" => args.seeds = val.parse().expect("--seeds expects an integer"),
            "--n" => args.n = val.parse().expect("--n expects an integer"),
            "--lambda" => args.lambda = val.parse().expect("--lambda expects a float"),
            "--mu" => args.mu = val.parse().expect("--mu expects a float"),
            "--servers" => args.servers = val.parse().expect("--servers expects an integer"),
            other => panic!("unknown argument: {other}"),
        }
        i += 2;
    }
    args
}

// ---------------------------------------------------------------------------
// Tiny JSON helpers (numbers + string-keyed objects only)
// ---------------------------------------------------------------------------

/// Format an f64 so it always round-trips and never emits bare `NaN`/`inf`
/// (which are not valid JSON). Non-finite values become `0`.
fn num(x: f64) -> String {
    if x.is_finite() {
        format!("{x}")
    } else {
        "0".to_string()
    }
}

/// Build a JSON object body from `(key, value)` pairs already-formatted as JSON.
fn obj(pairs: &[(&str, String)]) -> String {
    let body: Vec<String> = pairs.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
    format!("{{{}}}", body.join(","))
}

// ---------------------------------------------------------------------------
// Queue models (M/M/1 and M/M/c share the same code; mm1 == servers 1)
// ---------------------------------------------------------------------------

struct Rec {
    wait: f64,
    service: f64,
    departure: f64,
}

async fn queue_customer(
    env: EnvHandle,
    res: Resource,
    arrival: f64,
    service: f64,
    recs: Rc<RefCell<Vec<Rec>>>,
) {
    let guard = res.request().await;
    let wait = env.now() - arrival;
    env.timeout(service).await;
    drop(guard);
    let departure = env.now();
    recs.borrow_mut().push(Rec {
        wait,
        service,
        departure,
    });
}

async fn queue_arrivals(
    env: EnvHandle,
    res: Resource,
    n: u64,
    lambda: f64,
    mu: f64,
    recs: Rc<RefCell<Vec<Rec>>>,
) {
    for _ in 0..n {
        let inter_arrival = sample::exponential(&mut env.rng(), 1.0 / lambda);
        env.timeout(inter_arrival).await;
        let arrival = env.now();
        let service = sample::exponential(&mut env.rng(), 1.0 / mu);
        env.spawn(queue_customer(
            env.clone(),
            res.clone(),
            arrival,
            service,
            recs.clone(),
        ));
    }
}

/// One queue replication. Returns the per-seed JSON object body.
fn run_queue(seed: u64, n: u64, lambda: f64, mu: f64, servers: usize) -> String {
    let mut env = SimEnv::with_source(SplitMix64::new(seed));
    let res = Resource::new(servers);
    let recs: Rc<RefCell<Vec<Rec>>> = Rc::new(RefCell::new(Vec::with_capacity(n as usize)));

    env.spawn(queue_arrivals(
        env.handle(),
        res.clone(),
        n,
        lambda,
        mu,
        recs.clone(),
    ));
    env.run();

    let recs = recs.borrow();
    let count = recs.len() as f64;
    let t_end = recs.iter().map(|r| r.departure).fold(0.0_f64, f64::max);
    let sum_wait: f64 = recs.iter().map(|r| r.wait).sum();
    let sum_service: f64 = recs.iter().map(|r| r.service).sum();
    let sum_sojourn: f64 = recs.iter().map(|r| r.wait + r.service).sum();

    let mean_wait = if count > 0.0 { sum_wait / count } else { 0.0 };
    let mean_sojourn = if count > 0.0 {
        sum_sojourn / count
    } else {
        0.0
    };
    // Mean number in system L = area under N(t) / horizon = sum(sojourn) / T.
    let mean_queue = if t_end > 0.0 {
        sum_sojourn / t_end
    } else {
        0.0
    };
    let utilization = if t_end > 0.0 {
        sum_service / (servers as f64 * t_end)
    } else {
        0.0
    };
    let throughput = if t_end > 0.0 { count / t_end } else { 0.0 };

    obj(&[
        ("mean_wait", num(mean_wait)),
        ("mean_sojourn", num(mean_sojourn)),
        ("mean_queue", num(mean_queue)),
        ("utilization", num(utilization)),
        ("throughput", num(throughput)),
    ])
}

// ---------------------------------------------------------------------------
// Priority queue (two classes, 50/50 split)
// ---------------------------------------------------------------------------

struct PRec {
    prio: u32,
    wait: f64,
}

async fn prio_customer(
    env: EnvHandle,
    res: PriorityResource,
    prio: u32,
    arrival: f64,
    service: f64,
    recs: Rc<RefCell<Vec<PRec>>>,
) {
    let guard = res.request(prio).await;
    let wait = env.now() - arrival;
    env.timeout(service).await;
    drop(guard);
    recs.borrow_mut().push(PRec { prio, wait });
}

async fn prio_arrivals(
    env: EnvHandle,
    res: PriorityResource,
    n: u64,
    lambda: f64,
    mu: f64,
    recs: Rc<RefCell<Vec<PRec>>>,
) {
    for _ in 0..n {
        let inter_arrival = sample::exponential(&mut env.rng(), 1.0 / lambda);
        env.timeout(inter_arrival).await;
        let arrival = env.now();
        let service = sample::exponential(&mut env.rng(), 1.0 / mu);
        let prio: u32 = if sample::bernoulli(&mut env.rng(), 0.5) {
            0
        } else {
            1
        };
        env.spawn(prio_customer(
            env.clone(),
            res.clone(),
            prio,
            arrival,
            service,
            recs.clone(),
        ));
    }
}

fn run_priority(seed: u64, n: u64, lambda: f64, mu: f64, servers: usize) -> String {
    let mut env = SimEnv::with_source(SplitMix64::new(seed));
    let res = PriorityResource::new(servers);
    let recs: Rc<RefCell<Vec<PRec>>> = Rc::new(RefCell::new(Vec::with_capacity(n as usize)));

    env.spawn(prio_arrivals(
        env.handle(),
        res.clone(),
        n,
        lambda,
        mu,
        recs.clone(),
    ));
    env.run();

    let recs = recs.borrow();
    let mean_of = |class: Option<u32>| -> f64 {
        let xs: Vec<f64> = recs
            .iter()
            .filter(|r| class.is_none_or(|c| r.prio == c))
            .map(|r| r.wait)
            .collect();
        if xs.is_empty() {
            0.0
        } else {
            xs.iter().sum::<f64>() / xs.len() as f64
        }
    };

    obj(&[
        ("mean_wait_high", num(mean_of(Some(0)))),
        ("mean_wait_low", num(mean_of(Some(1)))),
        ("mean_wait_all", num(mean_of(None))),
    ])
}

// ---------------------------------------------------------------------------
// Container (mixed-size FIFO stress test)
//
// A producer drips fixed-size puts into an initially-empty container while
// consumers draw either a LARGE or SMALL amount. Under contention a large draw
// at the head of the queue cannot be satisfied until enough has accumulated;
// the question is whether a later small draw that *currently fits* must wait
// behind it (strict FIFO) or may be granted out of turn. This isolates the
// blood-bank divergence seen in the hospital model. Self-contained constants;
// ignores --lambda/--mu/--servers.
// ---------------------------------------------------------------------------

const C_CAP: f64 = 1000.0;
const C_ARRIVAL_SCALE: f64 = 1.0; // mean inter-arrival
const C_SMALL: f64 = 2.0;
const C_LARGE: f64 = 20.0;
const C_P_LARGE: f64 = 0.3;
const C_PUT_AMT: f64 = 7.0;
const C_PUT_INTERVAL: f64 = 1.0;

struct CRec {
    large: bool,
    wait: f64,
}

async fn c_consumer(
    env: EnvHandle,
    cont: Container,
    amount: f64,
    large: bool,
    arrival: f64,
    recs: Rc<RefCell<Vec<CRec>>>,
) {
    cont.get(amount).await;
    let wait = env.now() - arrival;
    recs.borrow_mut().push(CRec { large, wait });
}

async fn c_arrivals(env: EnvHandle, cont: Container, n: u64, recs: Rc<RefCell<Vec<CRec>>>) {
    for _ in 0..n {
        let inter_arrival = sample::exponential(&mut env.rng(), C_ARRIVAL_SCALE);
        env.timeout(inter_arrival).await;
        let arrival = env.now();
        let large = sample::bernoulli(&mut env.rng(), C_P_LARGE);
        let amount = if large { C_LARGE } else { C_SMALL };
        env.spawn(c_consumer(
            env.clone(),
            cont.clone(),
            amount,
            large,
            arrival,
            recs.clone(),
        ));
    }
}

async fn c_producer(env: EnvHandle, cont: Container, puts: u64) {
    for _ in 0..puts {
        env.timeout(C_PUT_INTERVAL).await;
        cont.put(C_PUT_AMT).await;
    }
}

fn run_container(seed: u64, n: u64) -> String {
    let mut env = SimEnv::with_source(SplitMix64::new(seed));
    let cont = Container::new(C_CAP, 0.0);
    let recs: Rc<RefCell<Vec<CRec>>> = Rc::new(RefCell::new(Vec::with_capacity(n as usize)));

    env.spawn(c_arrivals(env.handle(), cont.clone(), n, recs.clone()));
    env.spawn(c_producer(env.handle(), cont.clone(), n));
    env.run();

    let recs = recs.borrow();
    let mean_of = |which: Option<bool>| -> f64 {
        let xs: Vec<f64> = recs
            .iter()
            .filter(|r| which.is_none_or(|w| r.large == w))
            .map(|r| r.wait)
            .collect();
        if xs.is_empty() {
            0.0
        } else {
            xs.iter().sum::<f64>() / xs.len() as f64
        }
    };
    let served = recs.len() as f64;

    obj(&[
        ("mean_wait_all", num(mean_of(None))),
        ("mean_wait_small", num(mean_of(Some(false)))),
        ("mean_wait_large", num(mean_of(Some(true)))),
        ("served", num(served)),
    ])
}

// ---------------------------------------------------------------------------
// Coffee shop (scalable benchmark; exact-comparison twin of compare/models/coffee.py)
//
// `--servers c` baristas, `ceil(c/2)` cashiers, Poisson(`--lambda`) arrivals,
// barista prep Exp(mean 1/`--mu`). A share of customers order ahead on mobile and
// skip the cashier; walk-ins give up (renege) if the cashier queue outlasts
// their patience. Baristas serve walk-ins (prio 0) ahead of mobile orders
// (prio 1), and milk drinks draw from a milk fridge (Container) that is topped
// up every 30 minutes. Every random number is drawn at arrival in a fixed order,
// so the draw sequence is independent of event order (exact-mode eligible).
// Units are minutes. A readable standalone copy of this model lives in
// examples/coffee_shop_benchmark.rs (differences to the showcase: coffee_shop_benchmark.md).
// ---------------------------------------------------------------------------

const CF_MOBILE_P: f64 = 0.3;
const CF_ORDER_MEAN: f64 = 1.0;
const CF_PATIENCE_MEAN: f64 = 5.0;
const CF_MILK_P: f64 = 0.6;
const CF_MILK_PER_DRINK: f64 = 0.25;
const CF_MILK_CAP_PER_BARISTA: f64 = 2.0;
const CF_RESTOCK_INTERVAL: f64 = 30.0;

#[derive(Clone)]
struct CfShop {
    cashier: Resource,
    barista: PriorityResource,
    milk: Container,
    recs: Rc<RefCell<Vec<CfRec>>>,
    reneged: Rc<RefCell<u64>>,
    departed: Rc<RefCell<u64>>,
}

struct CfRec {
    walk_in: bool,
    milk: bool,
    wait_cashier: f64,
    wait_barista: f64,
    wait_milk: f64,
    busy: f64,
    in_shop: f64,
    departure: f64,
}

struct CfCustomer {
    mobile: bool,
    order: f64,
    patience: f64,
    prep: f64,
    milk: bool,
}

async fn cf_customer(env: EnvHandle, shop: CfShop, c: CfCustomer) {
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
        shop.milk.get(CF_MILK_PER_DRINK).await;
    }
    let wait_milk = env.now() - start;
    env.timeout(c.prep).await;
    drop(barista);

    let departure = env.now();
    shop.recs.borrow_mut().push(CfRec {
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

async fn cf_arrivals(env: EnvHandle, shop: CfShop, n: u64, lambda: f64, mu: f64) {
    for _ in 0..n {
        let inter_arrival = sample::exponential(&mut env.rng(), 1.0 / lambda);
        env.timeout(inter_arrival).await;
        let c = {
            let mut rng = env.rng();
            CfCustomer {
                mobile: sample::bernoulli(&mut rng, CF_MOBILE_P),
                order: sample::exponential(&mut rng, CF_ORDER_MEAN),
                patience: sample::exponential(&mut rng, CF_PATIENCE_MEAN),
                prep: sample::exponential(&mut rng, 1.0 / mu),
                milk: sample::bernoulli(&mut rng, CF_MILK_P),
            }
        };
        env.spawn(cf_customer(env.clone(), shop.clone(), c));
    }
}

/// Top the milk fridge up to capacity every interval until every customer has left.
async fn cf_restock(env: EnvHandle, shop: CfShop, n: u64) {
    loop {
        env.timeout(CF_RESTOCK_INTERVAL).await;
        if *shop.departed.borrow() == n {
            break;
        }
        let top_up = shop.milk.capacity() - shop.milk.level();
        if top_up > 0.0 {
            shop.milk.put(top_up).await;
        }
    }
}

fn run_coffee(seed: u64, n: u64, lambda: f64, mu: f64, servers: usize) -> String {
    let mut env = SimEnv::with_source(SplitMix64::new(seed));
    let milk_cap = CF_MILK_CAP_PER_BARISTA * servers as f64;
    let shop = CfShop {
        cashier: Resource::new(servers.div_ceil(2)),
        barista: PriorityResource::new(servers),
        milk: Container::new(milk_cap, milk_cap),
        recs: Rc::new(RefCell::new(Vec::with_capacity(n as usize))),
        reneged: Rc::new(RefCell::new(0)),
        departed: Rc::new(RefCell::new(0)),
    };

    env.spawn(cf_arrivals(env.handle(), shop.clone(), n, lambda, mu));
    env.spawn(cf_restock(env.handle(), shop.clone(), n));
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

    obj(&[
        ("served", num(recs.len() as f64)),
        ("reneged", num(reneged as f64)),
        (
            "mean_wait_cashier",
            num(mean(
                recs.iter()
                    .filter(|r| r.walk_in)
                    .map(|r| r.wait_cashier)
                    .collect(),
            )),
        ),
        (
            "mean_wait_barista",
            num(mean(recs.iter().map(|r| r.wait_barista).collect())),
        ),
        (
            "mean_wait_milk",
            num(mean(
                recs.iter()
                    .filter(|r| r.milk)
                    .map(|r| r.wait_milk)
                    .collect(),
            )),
        ),
        (
            "mean_time_in_shop",
            num(mean(recs.iter().map(|r| r.in_shop).collect())),
        ),
        ("barista_utilization", num(utilization)),
    ])
}

// ---------------------------------------------------------------------------
// Hospital (port of examples/hospital.rs; statistical-comparison policy)
//
// Differences vs. the showcase example (deliberate, mirrored on the SimPy side):
//   * no per-run log files;
//   * `ed_cleared_at` is the latest patient discharge time (tracked directly),
//     instead of an AllOf join — equivalent and simpler;
//   * no forced-completion deadline: patients that block on a depleted blood
//     bank simply never discharge; the run ends when the event queue drains.
//     Both tools apply this identical policy, so the comparison stays fair.
// ---------------------------------------------------------------------------

const H_SIM_DURATION: f64 = 480.0;
const H_ARRIVAL_RATE: f64 = 1.0 / 8.0;
const H_TRIAGE_DURATION: f64 = 5.0;
const H_MEAN_TREATMENT: f64 = 20.0;
const H_CRITICAL_PROB: f64 = 0.3;
const H_BLOOD_CAPACITY: f64 = 100.0;
const H_BLOOD_INITIAL: f64 = 60.0;
const H_BLOOD_RESTOCK: f64 = 20.0;
const H_RESTOCK_INTERVAL: f64 = 60.0;
const H_BLOOD_CRITICAL: f64 = 10.0;
const H_BLOOD_STANDARD: f64 = 2.0;

#[derive(Default)]
struct HStats {
    critical_treated: u32,
    standard_treated: u32,
    early_discharged: u32,
    blood_bank_waits: u32,
    total_nurse_wait: f64,
    total_bed_wait: f64,
    total_blood_wait: f64,
    ed_cleared_at: f64,
}

#[derive(Clone)]
struct HCtx {
    nurse: PriorityResource,
    beds: Resource,
    blood_bank: Container,
    eviction_map: Rc<RefCell<BTreeMap<u32, EventTrigger>>>,
    stats: Rc<RefCell<HStats>>,
}

async fn h_restock(env: EnvHandle, ctx: HCtx) {
    loop {
        env.timeout(H_RESTOCK_INTERVAL).await;
        if env.now() > H_SIM_DURATION {
            break;
        }
        ctx.blood_bank.put(H_BLOOD_RESTOCK).await;
    }
}

async fn h_arrivals(env: EnvHandle, ctx: HCtx) {
    let mut patient_id = 1_u32;
    loop {
        let inter_arrival = sample::exponential(&mut env.rng(), 1.0 / H_ARRIVAL_RATE);
        env.timeout(inter_arrival).await;
        if env.now() > H_SIM_DURATION {
            break;
        }
        let treatment = sample::exponential(&mut env.rng(), H_MEAN_TREATMENT);
        let is_critical = sample::bernoulli(&mut env.rng(), H_CRITICAL_PROB);
        let triage = if is_critical { 0_u32 } else { 1_u32 };
        env.spawn(h_patient(
            env.clone(),
            patient_id,
            triage,
            treatment,
            ctx.clone(),
        ));
        patient_id += 1;
    }
}

async fn h_patient(env: EnvHandle, id: u32, triage: u32, treatment: f64, ctx: HCtx) {
    let blood_units = if triage == 0 {
        H_BLOOD_CRITICAL
    } else {
        H_BLOOD_STANDARD
    };

    // Triage nurse (priority).
    let nurse_wait_start = env.now();
    let nurse = ctx.nurse.request(triage).await;
    let nurse_wait = env.now() - nurse_wait_start;
    env.timeout(H_TRIAGE_DURATION).await;
    drop(nurse);

    // Blood draw.
    let blood_wait_start = env.now();
    ctx.blood_bank.get(blood_units).await;
    let blood_wait = env.now() - blood_wait_start;
    if blood_wait > 0.0 {
        ctx.stats.borrow_mut().blood_bank_waits += 1;
    }

    // Critical patient evicts the longest-admitted patient if beds are full.
    if triage == 0 && ctx.beds.in_use() >= ctx.beds.capacity() {
        let victim_id = ctx.eviction_map.borrow().keys().next().copied();
        if let Some(vid) = victim_id {
            if let Some(trigger) = ctx.eviction_map.borrow_mut().remove(&vid) {
                trigger.fire();
            }
        }
    }

    // Wait for a bed.
    let bed_wait_start = env.now();
    let bed = ctx.beds.request().await;
    let bed_wait = env.now() - bed_wait_start;
    let admitted_at = env.now();

    // Register personal eviction signal, race treatment vs. eviction.
    let (my_trigger, my_signal) = env.event();
    ctx.eviction_map.borrow_mut().insert(id, my_trigger);
    any_of![env.timeout(treatment), my_signal].await;
    let was_early = env.now() < admitted_at + treatment;
    ctx.eviction_map.borrow_mut().remove(&id);
    drop(bed);

    let mut s = ctx.stats.borrow_mut();
    if triage == 0 {
        s.critical_treated += 1;
    } else {
        s.standard_treated += 1;
    }
    if was_early {
        s.early_discharged += 1;
    }
    s.total_nurse_wait += nurse_wait;
    s.total_bed_wait += bed_wait;
    s.total_blood_wait += blood_wait;
    if env.now() > s.ed_cleared_at {
        s.ed_cleared_at = env.now();
    }
}

fn run_hospital(seed: u64) -> String {
    let mut env = SimEnv::with_source(SplitMix64::new(seed));
    let ctx = HCtx {
        nurse: PriorityResource::new(1),
        beds: Resource::new(3),
        blood_bank: Container::new(H_BLOOD_CAPACITY, H_BLOOD_INITIAL),
        eviction_map: Rc::new(RefCell::new(BTreeMap::new())),
        stats: Rc::new(RefCell::new(HStats::default())),
    };
    env.spawn(h_restock(env.handle(), ctx.clone()));
    env.spawn(h_arrivals(env.handle(), ctx.clone()));
    env.run();

    let s = ctx.stats.borrow();
    let total = (s.critical_treated + s.standard_treated) as f64;
    let mean = |sum: f64| if total > 0.0 { sum / total } else { 0.0 };

    obj(&[
        ("critical_treated", num(s.critical_treated as f64)),
        ("standard_treated", num(s.standard_treated as f64)),
        ("early_discharged", num(s.early_discharged as f64)),
        ("blood_bank_waits", num(s.blood_bank_waits as f64)),
        ("mean_nurse_wait", num(mean(s.total_nurse_wait))),
        ("mean_bed_wait", num(mean(s.total_bed_wait))),
        ("mean_blood_wait", num(mean(s.total_blood_wait))),
        ("ed_cleared_at", num(s.ed_cleared_at)),
    ])
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let args = parse_args();

    // Time the replications. `--parallel` fans the seeds out across threads via
    // monte_carlo::run (results still come back in seed order, so output is
    // identical to the sequential path); otherwise run them in a plain loop.
    let start = Instant::now();
    let per_seed: Vec<String> = if args.parallel {
        let model = args.model.clone();
        let (n, lambda, mu, servers) = (args.n, args.lambda, args.mu, args.servers);
        simu::monte_carlo::run(0..args.seeds, move |seed| {
            run_model(&model, seed, n, lambda, mu, servers)
        })
    } else {
        (0..args.seeds)
            .map(|seed| {
                run_model(
                    &args.model,
                    seed,
                    args.n,
                    args.lambda,
                    args.mu,
                    args.servers,
                )
            })
            .collect()
    };
    let wall_secs = start.elapsed().as_secs_f64();

    // Event count uses an identical, model-specific definition on both sides so
    // events/sec is comparable across tools (see compare/README.md).
    let events: u64 = per_seed
        .iter()
        .map(|body| events_for(&args.model, body, args.n))
        .sum();

    let meta = obj(&[
        ("tool", "\"simu\"".to_string()),
        ("model", format!("\"{}\"", args.model)),
        ("seeds", args.seeds.to_string()),
        ("n", args.n.to_string()),
        ("lambda", num(args.lambda)),
        ("mu", num(args.mu)),
        ("servers", args.servers.to_string()),
        ("events", events.to_string()),
        ("wall_secs", num(wall_secs)),
        ("parallel", args.parallel.to_string()),
        ("per_seed", format!("[{}]", per_seed.join(","))),
    ]);
    println!("{meta}");
}

/// Dispatch one replication of `model` at `seed`, returning its per-seed JSON
/// body. Self-contained so it can run either in the sequential loop or as the
/// closure handed to `monte_carlo::run`.
fn run_model(model: &str, seed: u64, n: u64, lambda: f64, mu: f64, servers: usize) -> String {
    match model {
        "mm1" => run_queue(seed, n, lambda, mu, 1),
        "mmc" => run_queue(seed, n, lambda, mu, servers),
        "priority" => run_priority(seed, n, lambda, mu, servers),
        "container" => run_container(seed, n),
        "coffee" => run_coffee(seed, n, lambda, mu, servers),
        "hospital" => run_hospital(seed),
        other => panic!("unknown model: {other}"),
    }
}

/// Model-specific simulated-event tally for one per-seed body (identical
/// definition on the SimPy side, so events/sec is comparable across tools).
fn events_for(model: &str, body: &str, n: u64) -> u64 {
    match model {
        // one arrival + one departure (or renege) per customer
        "mm1" | "mmc" | "priority" | "coffee" => 2 * n,
        "container" => n + field_u64(body, "served"), // arrivals + completed gets
        "hospital" => 2 * count_completed(body),      // two transitions per completed patient
        other => panic!("unknown model: {other}"),
    }
}

/// Read a numeric field out of an already-formatted per-seed JSON body so the
/// event tally stays in lockstep with the metrics actually emitted.
fn field_u64(body: &str, name: &str) -> u64 {
    body.split(&format!("\"{name}\":"))
        .nth(1)
        .and_then(|s| s.split([',', '}']).next())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|x| x as u64)
        .unwrap_or(0)
}

fn count_completed(body: &str) -> u64 {
    field_u64(body, "critical_treated") + field_u64(body, "standard_treated")
}
