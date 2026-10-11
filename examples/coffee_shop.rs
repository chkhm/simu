// SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::cell::RefCell;
use std::rc::Rc;

use rand::Rng;
use rand_distr::Exp;
use simu::{any_of, Container, EnvHandle, PriorityResource, Resource, ResourceGuard, SimEnv};

// ---------------------------------------------------------------------------
// Coffee shop: how many baristas does a busy café need?
//
// Customers arrive all day. Walk-ins queue at the cashier, order, then wait
// for a barista; mobile-order customers skip the cashier and go straight to
// the barista queue. Impatient walk-ins give up and leave if the cashier
// queue takes too long. Milk drinks need milk from a small fridge behind the
// counter, topped up every 30 minutes; if it runs dry the barista waits.
//
// The question: with ~100 customers an hour, how do 3, 4, 5 or 6 baristas
// change the time customers spend in the shop and how many walk away?
//
//  • Resource          : cashiers (FIFO queue)
//  • PriorityResource  : baristas serve walk-ins (prio 0) ahead of mobile
//                        orders (prio 1), never interrupting a drink in progress
//  • Container         : the milk fridge (litres); `get` waits when it is dry
//  • any_of!           : race the cashier queue against the customer's patience
//  • monte_carlo::run  : 200 independent days per staffing level, in parallel
//
// The same model, driven by the portable SplitMix64 feed, is the `coffee`
// benchmark in `examples/compare.rs`, where it is checked seed-by-seed against
// an identical SimPy implementation (`compare/models/coffee.py`).
// Units: minutes.
// ---------------------------------------------------------------------------

const OPEN_FOR: f64 = 480.0; // 8-hour day; customers already inside are still served
const ARRIVALS_PER_MIN: f64 = 1.7; // ~100 customers an hour
const MOBILE_SHARE: f64 = 0.3; // ordered ahead in the app
const ORDER_MEAN: f64 = 1.0; // time at the cashier
const PATIENCE_MEAN: f64 = 5.0; // how long a walk-in tolerates the cashier queue
const PREP_MEAN: f64 = 2.0; // barista time per drink
const MILK_SHARE: f64 = 0.6; // drinks that need milk
const MILK_PER_DRINK: f64 = 0.25; // litres
const FRIDGE_PER_BARISTA: f64 = 2.0; // litres of fridge space per barista station
const RESTOCK_EVERY: f64 = 30.0;

const DAYS_PER_SETTING: u64 = 200;

#[derive(Clone)]
struct Shop {
    cashiers: Resource,
    baristas: PriorityResource,
    fridge: Container,
    stats: Rc<RefCell<DayStats>>,
}

#[derive(Default)]
struct DayStats {
    arrived: u32,
    departed: u32, // served or reneged
    served: u32,
    reneged: u32,
    time_in_shop: f64, // summed over served customers
    barista_busy: f64,
    last_departure: f64,
}

/// Queue for a cashier, but give up after `patience` minutes.
/// Returns `None` if the customer walked out.
async fn wait_for_cashier(
    env: &EnvHandle,
    cashiers: &Resource,
    patience: f64,
) -> Option<ResourceGuard> {
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

async fn customer(env: EnvHandle, shop: Shop) {
    // Sample everything up front: an RNG guard must not be held across `.await`.
    let (mobile, order, patience, prep, milk) = {
        let mut rng = env.rng();
        (
            rng.random_bool(MOBILE_SHARE),
            rng.sample(Exp::new(1.0 / ORDER_MEAN).unwrap()),
            rng.sample(Exp::new(1.0 / PATIENCE_MEAN).unwrap()),
            rng.sample(Exp::new(1.0 / PREP_MEAN).unwrap()),
            rng.random_bool(MILK_SHARE),
        )
    };
    let arrival = env.now();

    if !mobile {
        let Some(_cashier) = wait_for_cashier(&env, &shop.cashiers, patience).await else {
            let mut st = shop.stats.borrow_mut();
            st.reneged += 1;
            st.departed += 1;
            return;
        };
        env.timeout(order).await;
    } // _cashier drops here: the next walk-in steps up

    let priority = if mobile { 1 } else { 0 };
    let _barista = shop.baristas.request(priority).await;
    let start = env.now();
    if milk {
        shop.fridge.get(MILK_PER_DRINK).await; // waits if the fridge is dry
    }
    env.timeout(prep).await;

    let mut st = shop.stats.borrow_mut();
    st.served += 1;
    st.departed += 1;
    st.time_in_shop += env.now() - arrival;
    st.barista_busy += env.now() - start;
    st.last_departure = env.now();
}

async fn door(env: EnvHandle, shop: Shop) {
    let gap = Exp::new(ARRIVALS_PER_MIN).unwrap();
    loop {
        let wait = env.rng().sample(gap);
        if env.now() + wait > OPEN_FOR {
            break; // doors close
        }
        env.timeout(wait).await;
        shop.stats.borrow_mut().arrived += 1;
        env.spawn(customer(env.clone(), shop.clone()));
    }
}

/// Top the fridge up every half hour until the last customer has left.
async fn milk_run(env: EnvHandle, shop: Shop) {
    loop {
        env.timeout(RESTOCK_EVERY).await;
        let everyone_left = {
            let st = shop.stats.borrow();
            st.departed == st.arrived
        };
        if env.now() >= OPEN_FOR && everyone_left {
            break;
        }
        let top_up = shop.fridge.capacity() - shop.fridge.level();
        if top_up > 0.0 {
            shop.fridge.put(top_up).await;
        }
    }
}

struct DayResult {
    served: u32,
    reneged: u32,
    mean_time_in_shop: f64,
    utilization: f64,
}

fn simulate_day(seed: u64, baristas: usize) -> DayResult {
    let mut env = SimEnv::with_seed(seed);
    let fridge_size = FRIDGE_PER_BARISTA * baristas as f64;
    let shop = Shop {
        cashiers: Resource::new(baristas.div_ceil(2)),
        baristas: PriorityResource::new(baristas),
        fridge: Container::new(fridge_size, fridge_size),
        stats: Rc::new(RefCell::new(DayStats::default())),
    };
    env.spawn(door(env.handle(), shop.clone()));
    env.spawn(milk_run(env.handle(), shop.clone()));
    env.run();

    let st = shop.stats.borrow();
    DayResult {
        served: st.served,
        reneged: st.reneged,
        mean_time_in_shop: st.time_in_shop / st.served.max(1) as f64,
        utilization: st.barista_busy / (baristas as f64 * st.last_departure),
    }
}

fn main() {
    println!(
        "{DAYS_PER_SETTING} simulated days per staffing level, ~{:.0} customers/hour\n",
        ARRIVALS_PER_MIN * 60.0
    );
    println!(
        "{:>8}  {:>8}  {:>8}  {:>9}  {:>14}  {:>11}",
        "Baristas", "Cashiers", "Served", "Walk-outs", "Time in shop", "Barista use"
    );
    println!("{}", "-".repeat(66));

    for baristas in 3..=6 {
        let days = simu::monte_carlo::run(0..DAYS_PER_SETTING, |seed| simulate_day(seed, baristas));
        let n = days.len() as f64;
        let mean = |f: fn(&DayResult) -> f64| days.iter().map(f).sum::<f64>() / n;
        println!(
            "{:>8}  {:>8}  {:>8.0}  {:>9.1}  {:>10.1} min  {:>10.0}%",
            baristas,
            baristas.div_ceil(2),
            mean(|d| d.served as f64),
            mean(|d| d.reneged as f64),
            mean(|d| d.mean_time_in_shop),
            100.0 * mean(|d| d.utilization),
        );
    }
}
