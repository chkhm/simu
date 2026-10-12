// SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::cell::RefCell;
use std::rc::Rc;

use rand::Rng;
use rand_distr::Exp;
use simu::{monte_carlo, Resource, SimEnv};

/// One hour at a coffee stand with one barista. Returns the mean wait (minutes).
fn one_hour(seed: u64) -> f64 {
    let mut env = SimEnv::with_seed(seed);
    let barista = Resource::new(1);
    let waits = Rc::new(RefCell::new(Vec::new()));

    let h = env.handle();
    let w = waits.clone();
    env.spawn(async move {
        while h.now() < 60.0 {
            let gap = h.rng().sample(Exp::new(1.0 / 2.5).unwrap()); // a customer every 2.5 min
            h.timeout(gap).await;
            let (hc, b, w) = (h.clone(), barista.clone(), w.clone());
            h.spawn(async move {
                let arrived = hc.now();
                let _barista = b.request().await;
                w.borrow_mut().push(hc.now() - arrived);
                let prep = hc.rng().sample(Exp::new(1.0 / 2.0).unwrap()); // ~2 min per drink
                hc.timeout(prep).await;
            });
        }
    });
    env.run();

    let waits = waits.borrow();
    waits.iter().sum::<f64>() / waits.len().max(1) as f64
}

fn main() {
    // 1,000 independent hours, one seed each, spread over all CPU cores
    let mut results = monte_carlo::run(0..1000, one_hour);

    results.sort_by(f64::total_cmp);
    let mean = results.iter().sum::<f64>() / results.len() as f64;
    println!("average wait:   {mean:.1} min");
    println!("best 5% of days: under {:.1} min", results[50]);
    println!("worst 5% of days: over {:.1} min", results[950]);
}
