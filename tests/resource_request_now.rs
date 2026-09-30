// SPDX-FileCopyrightText: 2026 Clément Péron <peron.clem@gmail.com>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Resource::request_now`: a request that takes its place in the queue when
//! it is made, as SimPy's `resource.request()` does, and `is_granted`.

use std::cell::RefCell;
use std::rc::Rc;

use simu::{any_of, Resource, SimEnv};

type Log = Rc<RefCell<Vec<String>>>;

fn new_log() -> Log {
    Rc::new(RefCell::new(Vec::new()))
}

/// The unit is held until t = 2. At t = 1 the holder makes a request it awaits
/// only at t = 3; just after, in the same instant, another process requests and
/// awaits at once.
fn late_awaiter_against_prompt_one(eager: bool) -> Vec<String> {
    let mut env = SimEnv::with_seed(0);
    let pool = Resource::new(1);
    let log = new_log();

    let held = pool.request_now();
    let (h, p, l) = (env.handle(), pool.clone(), log.clone());
    env.spawn(async move {
        let guard = held.await;
        h.timeout(1.0).await;
        // Made in the holder's turn, before the prompt process runs.
        let late = if eager { p.request_now() } else { p.request() };
        let (h2, l2) = (h.clone(), l.clone());
        h.spawn(async move {
            h2.timeout(2.0).await;
            let _guard = late.await;
            l2.borrow_mut().push(format!("late@{}", h2.now()));
            h2.timeout(1.0).await;
        });
        h.timeout(1.0).await;
        drop(guard);
    });
    let (h, p, l) = (env.handle(), pool.clone(), log.clone());
    env.spawn(async move {
        h.timeout(1.0).await;
        let _guard = p.request().await;
        l.borrow_mut().push(format!("prompt@{}", h.now()));
        h.timeout(1.0).await;
    });

    env.run();
    log.take()
}

#[test]
fn request_now_keeps_its_place_before_it_is_awaited() {
    // Queued at the call, the late request is handed the unit at t = 2 while
    // unawaited, and the prompt one waits behind it.
    assert_eq!(
        late_awaiter_against_prompt_one(true),
        vec!["late@3", "prompt@4"]
    );
    // A lazy request only queues when awaited, so the prompt one goes first.
    assert_eq!(
        late_awaiter_against_prompt_one(false),
        vec!["prompt@2", "late@3"]
    );
}

#[test]
fn requests_are_served_in_the_order_they_were_made() {
    let mut env = SimEnv::with_seed(0);
    let pool = Resource::new(1);
    let log = new_log();

    let holder = pool.request_now();
    let first = pool.request_now();
    let second = pool.request_now();
    assert_eq!(pool.queue_len(), 2);

    let h = env.handle();
    env.spawn(async move {
        let _guard = holder.await;
        h.timeout(1.0).await;
    });
    // Awaited in the reverse order they were made.
    for (name, request) in [("second", second), ("first", first)] {
        let (h, l) = (env.handle(), log.clone());
        env.spawn(async move {
            let _guard = request.await;
            l.borrow_mut().push(format!("{name}@{}", h.now()));
            h.timeout(1.0).await;
        });
    }

    env.run();
    assert_eq!(*log.borrow(), vec!["first@1", "second@2"]);
}

#[test]
fn is_granted_follows_the_handoff() {
    let mut env = SimEnv::with_seed(0);
    let pool = Resource::new(1);
    let log = new_log();

    let holder = pool.request_now();
    let waiter = pool.request_now();
    assert!(holder.is_granted());
    assert!(!waiter.is_granted());
    assert_eq!(pool.in_use(), 1);

    let h = env.handle();
    env.spawn(async move {
        let guard = holder.await;
        h.timeout(2.0).await;
        drop(guard);
    });
    let (h, l) = (env.handle(), log.clone());
    env.spawn(async move {
        for _ in 0..3 {
            l.borrow_mut()
                .push(format!("{}:{}", h.now(), waiter.is_granted()));
            h.timeout(1.0).await;
        }
        let _guard = waiter.await;
        l.borrow_mut().push(format!("held@{}", h.now()));
    });

    env.run();
    assert_eq!(
        *log.borrow(),
        vec!["0:false", "1:false", "2:true", "held@3"]
    );
    assert_eq!(pool.in_use(), 0);
}

#[test]
fn lazy_request_reports_not_granted_until_polled() {
    let pool = Resource::new(1);
    let request = pool.request();
    assert!(!request.is_granted());
    assert_eq!(pool.in_use(), 0);
}

#[test]
fn dropping_a_queued_request_now_withdraws_it() {
    let mut env = SimEnv::with_seed(0);
    let pool = Resource::new(1);
    let log = new_log();

    let holder = pool.request_now();
    let withdrawn = pool.request_now();
    let (h, p, l) = (env.handle(), pool.clone(), log.clone());
    env.spawn(async move {
        let _guard = p.request().await;
        l.borrow_mut().push(format!("next@{}", h.now()));
    });
    assert_eq!(pool.queue_len(), 1); // the lazy request has not been polled

    drop(withdrawn);
    assert_eq!(pool.queue_len(), 0);

    let h = env.handle();
    env.spawn(async move {
        let _guard = holder.await;
        h.timeout(1.0).await;
    });

    env.run();
    assert_eq!(*log.borrow(), vec!["next@1"]);
    assert_eq!(pool.in_use(), 0);
}

#[test]
fn dropping_a_granted_request_now_passes_the_unit_on() {
    let mut env = SimEnv::with_seed(0);
    let pool = Resource::new(1);
    let log = new_log();

    let unused = pool.request_now();
    assert!(unused.is_granted());
    let (h, p, l) = (env.handle(), pool.clone(), log.clone());
    env.spawn(async move {
        let _guard = p.request().await;
        l.borrow_mut().push(format!("next@{}", h.now()));
    });
    let h = env.handle();
    env.spawn(async move {
        h.timeout(2.0).await;
        drop(unused); // never awaited: the unit goes to the waiter
    });

    env.run();
    assert_eq!(*log.borrow(), vec!["next@2"]);
    assert_eq!(pool.in_use(), 0);
}

#[test]
fn losing_request_now_in_any_of_is_withdrawn() {
    let mut env = SimEnv::with_seed(0);
    let pool = Resource::new(1);
    let log = new_log();

    let holder = pool.request_now();
    let h = env.handle();
    env.spawn(async move {
        let _guard = holder.await;
        h.timeout(5.0).await;
    });
    // Gives up at t = 1: the request made for the race is dropped with it.
    let (h, p) = (env.handle(), pool.clone());
    env.spawn(async move {
        let request = p.request_now();
        any_of![h.timeout(1.0), async move {
            let _guard = request.await;
        }]
        .await;
    });
    let (h, p, l) = (env.handle(), pool.clone(), log.clone());
    env.spawn(async move {
        h.timeout(2.0).await;
        let _guard = p.request().await;
        l.borrow_mut().push(format!("next@{}", h.now()));
    });

    env.run();
    assert_eq!(*log.borrow(), vec!["next@5"]);
    assert_eq!(pool.queue_len(), 0);
    assert_eq!(pool.in_use(), 0);
}
