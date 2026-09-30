// SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::cell::RefCell;
use std::rc::Rc;

use simu::SimEnv;

type Log = Rc<RefCell<Vec<String>>>;
fn new_log() -> Log { Rc::new(RefCell::new(Vec::new())) }

#[test]
fn basic_fire_and_wake() {
    let mut env = SimEnv::with_seed(0);
    let (trigger, awaitable) = env.event();
    let log = new_log();

    // Process B: waits on the event, records when it wakes.
    {
        let h = env.handle();
        let log2 = log.clone();
        env.spawn(async move {
            awaitable.await;
            log2.borrow_mut().push(format!("B:{}", h.now()));
        });
    }

    // Process A: waits until t=5, then fires the event.
    {
        let h = env.handle();
        let log2 = log.clone();
        env.spawn(async move {
            h.timeout(5.0).await;
            trigger.fire();
            log2.borrow_mut().push(format!("A:{}", h.now()));
        });
    }

    env.run();

    // A fires then completes; B wakes in the same tick.
    // A logs first (it is currently being polled), then B is picked up in
    // the same poll_ready pass.
    assert_eq!(*log.borrow(), vec!["A:5", "B:5"]);
    assert_eq!(env.now(), 5.0);
}

#[test]
fn fire_before_await_resolves_immediately() {
    // Trigger fires at t=2. A second process polls the awaitable at t=3.
    // The `fired` latch makes it resolve without suspending.
    let mut env = SimEnv::with_seed(0);
    let (trigger, awaitable) = env.event();
    let log = new_log();

    // Process A: fires the trigger at t=2.
    {
        let h = env.handle();
        env.spawn(async move {
            h.timeout(2.0).await;
            trigger.fire();
        });
    }

    // Process B: arrives at t=3, after the trigger has already fired.
    {
        let h = env.handle();
        let log2 = log.clone();
        env.spawn(async move {
            h.timeout(3.0).await;
            awaitable.await; // must resolve immediately — fired latch is set
            log2.borrow_mut().push(format!("B:{}", h.now()));
        });
    }

    env.run();

    // B recorded t=3, not any later time — it did not suspend on the event.
    assert_eq!(*log.borrow(), vec!["B:3"]);
}

#[test]
fn multi_waiter_all_wake() {
    // Three processes await the same event; a fourth fires it at t=7.
    // All three must wake in the same tick.
    let mut env = SimEnv::with_seed(0);
    let (trigger, awaitable) = env.event();
    let log = new_log();

    for label in ["W1", "W2", "W3"] {
        let log2 = log.clone();
        let h = env.handle();
        let aw = awaitable.clone();
        let label = label.to_string();
        env.spawn(async move {
            aw.await;
            log2.borrow_mut().push(format!("{}:{}", label, h.now()));
        });
    }

    {
        let h = env.handle();
        env.spawn(async move {
            h.timeout(7.0).await;
            trigger.fire();
        });
    }

    env.run();

    assert_eq!(env.now(), 7.0);
    // All three waiters must have recorded t=7.
    let entries = log.borrow();
    assert_eq!(entries.len(), 3);
    assert!(entries.contains(&"W1:7".to_string()));
    assert!(entries.contains(&"W2:7".to_string()));
    assert!(entries.contains(&"W3:7".to_string()));
}

#[test]
fn clone_shares_underlying_event() {
    // A cloned EventAwaitable and the original share the same EventState.
    // Firing the trigger must wake both.
    let mut env = SimEnv::with_seed(0);
    let (trigger, awaitable) = env.event();
    let awaitable2 = awaitable.clone();
    let log = new_log();

    for (aw, label) in [(awaitable, "orig"), (awaitable2, "clone")] {
        let h = env.handle();
        let log2 = log.clone();
        let label = label.to_string();
        env.spawn(async move {
            aw.await;
            log2.borrow_mut().push(format!("{}:{}", label, h.now()));
        });
    }

    {
        let h = env.handle();
        env.spawn(async move {
            h.timeout(3.0).await;
            trigger.fire();
        });
    }

    env.run();

    let entries = log.borrow();
    assert_eq!(entries.len(), 2);
    assert!(entries.contains(&"orig:3".to_string()));
    assert!(entries.contains(&"clone:3".to_string()));
}

#[test]
fn envhandle_event_method() {
    // Exercise EnvHandle::event() (called on the handle, not on SimEnv).
    let mut env = SimEnv::with_seed(0);
    let h = env.handle();
    let (trigger, awaitable) = h.event(); // called on EnvHandle
    let log = new_log();

    {
        let h = env.handle();
        let log2 = log.clone();
        env.spawn(async move {
            awaitable.await;
            log2.borrow_mut().push(format!("woken:{}", h.now()));
        });
    }

    {
        let h = env.handle();
        env.spawn(async move {
            h.timeout(4.0).await;
            trigger.fire();
        });
    }

    env.run();

    assert_eq!(*log.borrow(), vec!["woken:4"]);
}

// --- F6: dropping an unfired EventTrigger strands its waiters (documented) ---

#[test]
fn dropped_unfired_trigger_strands_waiter() {
    // A process awaiting an event whose trigger is dropped without firing must
    // never resolve; the run still terminates (no other events) and SimEnv's
    // Drop reclaims the suspended process. This documents the F6 semantics.
    let mut env = SimEnv::with_seed(0);
    let log = new_log();
    let l2 = log.clone();
    let (trigger, awaitable) = env.event();

    env.spawn(async move {
        awaitable.await;
        l2.borrow_mut().push("resolved".to_string());
    });

    // Drop the trigger without firing.
    drop(trigger);
    env.run();

    // The waiter never resolved.
    assert!(log.borrow().is_empty(), "waiter unexpectedly resolved: {:?}", log.borrow());
}

#[test]
fn is_fired_reports_the_trigger() {
    let mut env = SimEnv::with_seed(0);
    let (trigger, signal) = env.event();
    let observer = signal.clone();
    let log = new_log();

    let h = env.handle();
    env.spawn(async move {
        h.timeout(3.0).await;
        trigger.fire();
    });

    // Polls the event without awaiting it, before and after the fire.
    let (h, l) = (env.handle(), log.clone());
    env.spawn(async move {
        for _ in 0..2 {
            h.timeout(2.0).await;
            l.borrow_mut()
                .push(format!("{}:{}", h.now(), observer.is_fired()));
        }
    });

    assert!(!signal.is_fired());
    env.run();
    assert!(signal.is_fired());
    assert_eq!(*log.borrow(), vec!["2:false", "4:true"]);
}

#[test]
fn is_fired_is_false_for_a_dropped_trigger() {
    let env = SimEnv::with_seed(0);
    let (trigger, signal) = env.event();
    drop(trigger);
    assert!(!signal.is_fired());
}
