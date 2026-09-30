// SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared waiter bookkeeping for the unit-pool resources.
//!
//! `Resource`, `PriorityResource` (and the post-MVP `PreemptiveResource`) all
//! follow the same **direct-handoff** protocol: a request takes a free unit
//! immediately if one is available, otherwise it parks a waker in an ordered
//! queue; releasing a unit *transfers* it to the next live waiter (setting the
//! waiter's `granted` flag and waking it) without the unit ever becoming
//! observably free — see [`WaitQueue::release`]. The only thing that differs
//! between the resource types is the *order* in which parked waiters are served.
//!
//! [`WaitQueue`] captures that protocol once. Waiters are served in ascending
//! `(key, seq)` order — smallest key first, ties broken FIFO by an internal
//! monotonic sequence. FIFO scheduling is therefore just the degenerate case
//! `K = ()` (every key equal → pure insertion order); priority scheduling uses
//! `K = u32` (lower number = higher priority).
//!
//! Extracting this here keeps the `registered` / `canceled` / `granted` /
//! drop-cancel invariants (see `SPEC.md §4.7`) in a single place rather than
//! copied across every resource type.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::rc::Rc;
use std::task::Waker;

/// A waker a queued request can refill on each poll. Empty for a request that
/// was queued when it was made and has not been polled yet
/// (`Resource::request_now`): a grant then only sets its `granted` flag, and
/// its first poll finds it.
pub(crate) type WakerSlot = Rc<RefCell<Option<Waker>>>;

/// How a parked waiter is woken when it is granted a unit.
enum Wakeup {
    /// Parked by its first poll, with the waker it had then. The common case,
    /// kept allocation-free.
    Waker(Waker),
    /// Queued at the call: the slot its request keeps current.
    Slot(WakerSlot),
}

impl Wakeup {
    fn wake(self) {
        match self {
            Wakeup::Waker(waker) => waker.wake(),
            Wakeup::Slot(slot) => {
                // Take the waker before waking so the slot is not borrowed
                // while the executor runs.
                let waker = slot.take();
                if let Some(waker) = waker {
                    waker.wake();
                }
            }
        }
    }
}

/// One suspended requester parked in a [`WaitQueue`].
struct Entry<K> {
    key: K,
    /// Monotonic tie-breaker guaranteeing FIFO order within equal keys.
    seq: u64,
    wakeup: Wakeup,
    /// Shared with the owning request future. Its `Drop` sets this to `true`
    /// when abandoned before being granted; [`WaitQueue::release`] skips such
    /// entries so live waiters behind them are still served.
    canceled: Rc<Cell<bool>>,
    /// Shared with the owning request future. [`WaitQueue::release`] sets this
    /// to `true` when it hands the unit *directly* to this waiter, so the
    /// waiter's next poll returns `Ready` without re-checking capacity — the
    /// unit is never observably free, so no later request can steal it. This is
    /// the commit-at-wake half of the direct-handoff protocol (mirrors
    /// `Container`'s `done` flag).
    granted: Rc<Cell<bool>>,
}

// `BinaryHeap` is a max-heap, but we want the *smallest* `(key, seq)` served
// first, so every comparison is reversed (compare `other` to `self`).
impl<K: Ord> Ord for Entry<K> {
    fn cmp(&self, other: &Self) -> Ordering {
        other.key.cmp(&self.key).then(other.seq.cmp(&self.seq))
    }
}

impl<K: Ord> PartialOrd for Entry<K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: Ord> PartialEq for Entry<K> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.seq == other.seq
    }
}

impl<K: Ord> Eq for Entry<K> {}

/// A capacity-limited pool plus its ordered queue of blocked waiters.
///
/// Generic over the ordering key `K`: `WaitQueue<()>` is FIFO, `WaitQueue<u32>`
/// is priority-ordered (lower key first, FIFO within a level). All methods
/// operate on `&mut self`; the resource wraps this in `Rc<RefCell<…>>` and
/// shares it by cloning, exactly like the rest of the crate.
pub(crate) struct WaitQueue<K: Ord> {
    capacity: usize,
    in_use: usize,
    /// Monotonic counter assigning each waiter its FIFO tie-breaker.
    next_seq: u64,
    waiters: BinaryHeap<Entry<K>>,
}

impl<K: Ord> WaitQueue<K> {
    /// Create a pool with the given capacity.
    ///
    /// Callers (the resource constructors) are responsible for rejecting a
    /// zero capacity with a type-specific panic message before calling this.
    pub(crate) fn new(capacity: usize) -> Self {
        WaitQueue {
            capacity,
            in_use: 0,
            next_seq: 0,
            waiters: BinaryHeap::new(),
        }
    }

    /// Units currently held.
    pub(crate) fn in_use(&self) -> usize {
        self.in_use
    }

    /// Total capacity.
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of live (non-canceled) waiters currently parked in the queue.
    ///
    /// Canceled entries are excluded because they hold no claim on capacity and
    /// are lazily discarded on the next `release`.
    pub(crate) fn live_waiters(&self) -> usize {
        self.waiters.iter().filter(|e| !e.canceled.get()).count()
    }

    /// Take a genuinely free unit if capacity allows, returning whether it was
    /// granted.
    ///
    /// Called only for a request's *initial* attempt (a fresh, not-yet-parked
    /// requester). A woken waiter does **not** come back through here — it is
    /// handed its unit directly by [`release`](WaitQueue::release) and returns
    /// via its `granted` flag. Because `release` transfers the unit without ever
    /// decrementing `in_use` while any live waiter is queued, a fresh requester
    /// polled between a release and the woken waiter's re-poll always finds
    /// `in_use == capacity` here and correctly queues *behind* the waiter — this
    /// is what preserves FIFO/priority and prevents the woken waiter from being
    /// stranded (see `reviews/2026-07-01-implementation-review.md`, Finding 1).
    pub(crate) fn try_acquire(&mut self) -> bool {
        if self.in_use < self.capacity {
            self.in_use += 1;
            true
        } else {
            false
        }
    }

    /// Park a waiter in the queue. Call only once per request (guarded by the
    /// request's `registered` flag) to avoid double-queuing across polls.
    pub(crate) fn register(
        &mut self,
        key: K,
        waker: Waker,
        canceled: Rc<Cell<bool>>,
        granted: Rc<Cell<bool>>,
    ) {
        self.park(key, Wakeup::Waker(waker), canceled, granted);
    }

    /// Park a waiter whose waker is kept in a slot its request can refill on
    /// each poll — or leave empty until then, for a request queued when it was
    /// made. Same once-per-request rule as [`register`](WaitQueue::register).
    pub(crate) fn register_slot(
        &mut self,
        key: K,
        slot: WakerSlot,
        canceled: Rc<Cell<bool>>,
        granted: Rc<Cell<bool>>,
    ) {
        self.park(key, Wakeup::Slot(slot), canceled, granted);
    }

    fn park(&mut self, key: K, wakeup: Wakeup, canceled: Rc<Cell<bool>>, granted: Rc<Cell<bool>>) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.waiters.push(Entry {
            key,
            seq,
            wakeup,
            canceled,
            granted,
        });
    }

    /// Release one held unit, handing it **directly** to the next live waiter
    /// if there is one.
    ///
    /// Rather than marking the unit free (decrementing `in_use`) and letting the
    /// woken waiter race for it, the unit is *transferred*: the next live waiter
    /// in priority/FIFO order has its `granted` flag set and is woken, and
    /// `in_use` is left unchanged — the unit is never observably free, so a
    /// fresh request cannot `try_acquire` it out from under the woken waiter.
    /// Only when no live waiter wants the unit does `in_use` actually drop.
    ///
    /// Canceled entries (abandoned requests) are popped and discarded without
    /// being granted, so the first *live* waiter is the one served — no
    /// starvation behind a dead entry.
    pub(crate) fn release(&mut self) {
        while let Some(entry) = self.waiters.pop() {
            if !entry.canceled.get() {
                entry.granted.set(true);
                entry.wakeup.wake();
                return; // unit transferred; `in_use` deliberately unchanged
            }
        }
        // No live waiter: the unit truly frees.
        self.in_use -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;
    use std::task::Wake;

    /// A waker that records, into a shared counter, the order in which wakers
    /// fire. Each `RecordingWaker` stamps its `slot` with the next tick so the
    /// test can assert *which* waiter was resumed.
    struct RecordingWaker {
        order: Arc<AtomicUsize>,
        slot: Arc<AtomicUsize>,
    }

    impl Wake for RecordingWaker {
        fn wake(self: Arc<Self>) {
            let tick = self.order.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            self.slot.store(tick, AtomicOrdering::SeqCst);
        }
    }

    /// Returns `(waker, slot)`. After the queue wakes it, `slot` holds a
    /// nonzero tick recording the relative wake order (1 = woken first).
    fn recording(order: &Arc<AtomicUsize>) -> (Waker, Arc<AtomicUsize>) {
        let slot = Arc::new(AtomicUsize::new(0));
        let waker = Waker::from(Arc::new(RecordingWaker {
            order: Arc::clone(order),
            slot: Arc::clone(&slot),
        }));
        (waker, slot)
    }

    fn live() -> Rc<Cell<bool>> {
        Rc::new(Cell::new(false))
    }

    /// Register a waiter and return its `(slot, canceled, granted)` handles so a
    /// test can observe both the wake order and the direct-handoff `granted`
    /// flag. The waiter starts live (neither canceled nor granted).
    fn register_waiter<K: Ord>(
        q: &mut WaitQueue<K>,
        key: K,
        order: &Arc<AtomicUsize>,
    ) -> (Arc<AtomicUsize>, Rc<Cell<bool>>) {
        let (waker, slot) = recording(order);
        let canceled = live();
        let granted = live();
        q.register(key, waker, Rc::clone(&canceled), Rc::clone(&granted));
        (slot, granted)
    }

    #[test]
    fn try_acquire_respects_capacity() {
        let mut q: WaitQueue<()> = WaitQueue::new(2);
        assert!(q.try_acquire());
        assert!(q.try_acquire());
        assert!(!q.try_acquire()); // capacity exhausted
        assert_eq!(q.in_use(), 2);
        assert_eq!(q.capacity(), 2);
        // No waiters queued: release genuinely frees the unit.
        q.release();
        assert_eq!(q.in_use(), 1);
        assert!(q.try_acquire());
    }

    #[test]
    fn release_hands_off_directly_without_freeing_the_unit() {
        // The core of the F1 fix: while a live waiter is queued, release
        // transfers the unit to it (setting `granted`) and leaves `in_use`
        // pinned at capacity, so a concurrent `try_acquire` cannot steal it.
        let mut q: WaitQueue<()> = WaitQueue::new(1);
        assert!(q.try_acquire());

        let order = Arc::new(AtomicUsize::new(0));
        let (s0, g0) = register_waiter(&mut q, (), &order);

        q.release();
        assert!(g0.get(), "unit must be handed directly to the waiter");
        assert_eq!(s0.load(AtomicOrdering::SeqCst), 1, "waiter must be woken");
        assert_eq!(q.in_use(), 1, "unit must stay in use — never observably free");
        assert!(!q.try_acquire(), "a fresh request must not be able to steal it");
    }

    #[test]
    fn fifo_order_for_unit_key() {
        let mut q: WaitQueue<()> = WaitQueue::new(1);
        assert!(q.try_acquire());

        let order = Arc::new(AtomicUsize::new(0));
        let (s0, g0) = register_waiter(&mut q, (), &order);
        let (s1, g1) = register_waiter(&mut q, (), &order);
        let (s2, g2) = register_waiter(&mut q, (), &order);

        // Each release hands the unit directly to the next waiter, in insertion
        // order. `in_use` stays at 1 throughout (the unit is transferred, never
        // freed) until the last waiter releases with an empty queue.
        q.release();
        assert!(g0.get() && !g1.get() && !g2.get());
        assert_eq!(q.in_use(), 1);
        q.release();
        assert!(g1.get() && !g2.get());
        assert_eq!(q.in_use(), 1);
        q.release();
        assert!(g2.get());
        assert_eq!(q.in_use(), 1);
        q.release();
        assert_eq!(q.in_use(), 0); // nobody left: the unit finally frees

        assert_eq!(s0.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(s1.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(s2.load(AtomicOrdering::SeqCst), 3);
    }

    #[test]
    fn priority_order_then_fifo_within_level() {
        let mut q: WaitQueue<u32> = WaitQueue::new(1);
        assert!(q.try_acquire());

        let order = Arc::new(AtomicUsize::new(0));
        // Register out of priority order; two share priority 5 (FIFO between them).
        let (s_lo_a, _g_lo_a) = register_waiter(&mut q, 5, &order); // prio 5, first
        let (s_hi, _g_hi) = register_waiter(&mut q, 1, &order); // prio 1, highest
        let (s_lo_b, _g_lo_b) = register_waiter(&mut q, 5, &order); // prio 5, second

        q.release(); // highest priority (1) first
        q.release(); // then prio 5, FIFO: lo_a
        q.release(); // then prio 5: lo_b
        assert_eq!(s_hi.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(s_lo_a.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(s_lo_b.load(AtomicOrdering::SeqCst), 3);
    }

    #[test]
    fn release_skips_canceled_waiter() {
        let mut q: WaitQueue<u32> = WaitQueue::new(1);
        assert!(q.try_acquire());

        let order = Arc::new(AtomicUsize::new(0));
        let (w_dead, s_dead) = recording(&order);
        let dead_flag = live();
        let dead_granted = live();
        q.register(0, w_dead, Rc::clone(&dead_flag), Rc::clone(&dead_granted)); // top prio, canceled
        let (s_live, g_live) = register_waiter(&mut q, 1, &order);

        dead_flag.set(true); // abandon the top-priority waiter

        q.release();
        // The canceled entry must be skipped without being woken or granted; the
        // unit is handed to the next live waiter instead.
        assert_eq!(s_dead.load(AtomicOrdering::SeqCst), 0);
        assert!(!dead_granted.get());
        assert_eq!(s_live.load(AtomicOrdering::SeqCst), 1);
        assert!(g_live.get());
        assert_eq!(q.in_use(), 1); // transferred, still in use
    }

    #[test]
    fn release_grants_a_waiter_with_no_waker_yet() {
        // A request queued at creation holds its place with an empty waker
        // slot; a release hands it the unit without waking anyone.
        let mut q: WaitQueue<()> = WaitQueue::new(1);
        assert!(q.try_acquire());

        let slot: WakerSlot = Rc::new(RefCell::new(None));
        let granted = live();
        q.register_slot((), Rc::clone(&slot), live(), Rc::clone(&granted));
        assert_eq!(q.live_waiters(), 1);

        q.release();
        assert!(granted.get());
        assert!(slot.borrow().is_none());
        assert_eq!(q.in_use(), 1); // transferred, still in use
        assert_eq!(q.live_waiters(), 0);
    }

    #[test]
    fn release_with_only_canceled_waiters_frees_the_unit() {
        let mut q: WaitQueue<()> = WaitQueue::new(1);
        assert!(q.try_acquire());

        let order = Arc::new(AtomicUsize::new(0));
        let (w, s) = recording(&order);
        let flag = live();
        let granted = live();
        q.register((), w, Rc::clone(&flag), Rc::clone(&granted));
        flag.set(true);

        q.release();
        assert_eq!(s.load(AtomicOrdering::SeqCst), 0); // never woken
        assert!(!granted.get()); // never granted
        assert_eq!(q.in_use(), 0); // unit genuinely freed
    }
}
