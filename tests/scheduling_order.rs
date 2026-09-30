// SPDX-FileCopyrightText: 2026 Clément Péron <peron.clem@gmail.com>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Ordering guarantees the rest of the suite does not pin down: when a
//! timeout enters the queue, when a spawned process first runs, how an event's
//! wake orders against a timeout due at the same instant, and what a process
//! sees when the losing arm of an `any_of!` wakes it later.
//!
//! Models written against SimPy lean on these; each test states what simu does
//! so a change to it is deliberate.

use std::cell::RefCell;
use std::rc::Rc;

use simu::{any_of, SimEnv};

type Log = Rc<RefCell<Vec<String>>>;

fn new_log() -> Log {
    Rc::new(RefCell::new(Vec::new()))
}

/// A timeout's deadline is fixed when it is created, but it enters the event
/// queue when it is first polled. Two timeouts with the same deadline therefore
/// fire in the order they were first awaited, not the order they were created.
#[test]
fn timeout_is_queued_at_first_poll_with_its_creation_deadline() {
    let mut env = SimEnv::with_seed(0);
    let log = new_log();
    let h = env.handle();
    let first = h.timeout(5.0);
    let second = h.timeout(5.0);

    // The second timeout is awaited first, at t = 0.
    let (h2, log2) = (h.clone(), log.clone());
    env.spawn(async move {
        second.await;
        log2.borrow_mut().push(format!("second@{}", h2.now()));
    });
    // The first is awaited only at t = 2, but its deadline stays t = 5.
    let (h1, log1) = (h.clone(), log.clone());
    env.spawn(async move {
        h1.timeout(2.0).await;
        first.await;
        log1.borrow_mut().push(format!("first@{}", h1.now()));
    });

    env.run();
    assert_eq!(*log.borrow(), vec!["second@5", "first@5"]);
}

/// A process spawned by a running process starts in the same instant, before
/// the clock moves on to the next queued event.
#[test]
fn process_spawned_mid_run_starts_in_the_same_instant() {
    let mut env = SimEnv::with_seed(0);
    let log = new_log();

    let (h, l) = (env.handle(), log.clone());
    env.spawn(async move {
        h.timeout(4.0).await;
        l.borrow_mut().push(format!("later@{}", h.now()));
    });

    let (h, l) = (env.handle(), log.clone());
    env.spawn(async move {
        h.timeout(3.0).await;
        let (child_h, child_l) = (h.clone(), l.clone());
        h.spawn(async move {
            child_l
                .borrow_mut()
                .push(format!("child@{}", child_h.now()));
        });
        l.borrow_mut().push(format!("parent@{}", h.now()));
    });

    env.run();
    assert_eq!(*log.borrow(), vec!["parent@3", "child@3", "later@4"]);
}

/// Firing an event resumes its waiters within the same pass of the ready
/// queue, before a timeout that is due at the same instant but was queued
/// earlier.
///
/// This differs from SimPy, where `succeed()` schedules the waiters' resumption
/// behind the events already queued for the instant, so there the order would
/// be `firer, timeout, waiter`.
#[test]
fn fired_event_resumes_waiters_before_a_same_instant_timeout() {
    let mut env = SimEnv::with_seed(0);
    let log = new_log();
    let (trigger, signal) = env.event();

    let (h, l) = (env.handle(), log.clone());
    env.spawn(async move {
        h.timeout(5.0).await;
        l.borrow_mut().push(format!("firer@{}", h.now()));
        trigger.fire();
    });
    let (h, l) = (env.handle(), log.clone());
    env.spawn(async move {
        h.timeout(5.0).await;
        l.borrow_mut().push(format!("timeout@{}", h.now()));
    });
    let (h, l) = (env.handle(), log.clone());
    env.spawn(async move {
        signal.await;
        l.borrow_mut().push(format!("waiter@{}", h.now()));
    });

    env.run();
    assert_eq!(*log.borrow(), vec!["firer@5", "waiter@5", "timeout@5"]);
}

/// The losing timeout of an `any_of!` still wakes its process at its deadline.
/// The process is by then awaiting something else, which must stay pending.
#[test]
fn losing_timeout_wakes_the_process_spuriously_and_is_ignored() {
    let mut env = SimEnv::with_seed(0);
    let log = new_log();
    let (trigger, signal) = env.event();

    let h = env.handle();
    env.spawn(async move {
        h.timeout(2.0).await;
        trigger.fire();
    });

    let (h, l) = (env.handle(), log.clone());
    env.spawn(async move {
        any_of![h.timeout(10.0), signal].await; // the event wins at t = 2
        l.borrow_mut().push(format!("won@{}", h.now()));
        h.timeout(20.0).await; // woken spuriously at t = 10 by the loser
        l.borrow_mut().push(format!("done@{}", h.now()));
    });

    env.run();
    assert_eq!(*log.borrow(), vec!["won@2", "done@22"]);
}

/// The losing event of an `any_of!` keeps its waiter's waker until it fires,
/// and firing it then wakes the process spuriously. The event the process is
/// awaiting by then must stay pending until it fires itself.
#[test]
fn losing_event_wakes_the_process_spuriously_and_is_ignored() {
    let mut env = SimEnv::with_seed(0);
    let log = new_log();
    let (lost_trigger, lost) = env.event();
    let (next_trigger, next) = env.event();

    let h = env.handle();
    env.spawn(async move {
        h.timeout(3.0).await;
        lost_trigger.fire(); // wakes the racer, which awaits `next` by now
        h.timeout(2.0).await;
        next_trigger.fire();
    });

    let (h, l) = (env.handle(), log.clone());
    env.spawn(async move {
        any_of![h.timeout(1.0), lost].await; // the timeout wins at t = 1
        l.borrow_mut().push(format!("won@{}", h.now()));
        next.await;
        l.borrow_mut().push(format!("done@{}", h.now()));
    });

    env.run();
    assert_eq!(*log.borrow(), vec!["won@1", "done@5"]);
}
