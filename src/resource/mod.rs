// SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

pub mod container;
pub use container::{Container, ContainerGetRequest, ContainerPutRequest};

mod preemptive;
pub use preemptive::{PreemptiveGuard, PreemptiveRequest, PreemptiveResource};
pub mod priority;
pub use priority::{PriorityResource, PriorityResourceGuard, PriorityResourceRequest};

pub(crate) mod wait_queue;
use wait_queue::{WaitQueue, WakerSlot};

/// A cloneable handle to a capacity-limited resource pool.
///
/// Units are acquired by calling [`request`](Resource::request) and awaiting
/// the returned future. If no unit is available the calling process is
/// suspended and woken in FIFO order when one becomes free.
///
/// `Resource` wraps an `Rc<RefCell<>>` internally, so cloning is cheap and
/// all clones share the same pool. It is `!Send + !Sync` — consistent with
/// `SimEnv`.
///
/// Two processes sharing a single-unit pump — the second queues until the
/// first one's guard drops:
///
/// ```
/// use simu::{SimEnv, Resource};
///
/// let mut env = SimEnv::with_seed(0);
/// let pump = Resource::new(1);
///
/// for _ in 0..2 {
///     let h = env.handle();
///     let p = pump.clone(); // same pool — share by cloning, never Arc
///     env.spawn(async move {
///         let _guard = p.request().await; // second process suspends here
///         h.timeout(3.0).await;           // use the pump for 3 time units
///     }); // guard drops → pump handed to the next waiter
/// }
///
/// env.run();
/// assert_eq!(env.now(), 6.0); // servings ran back-to-back, not in parallel
/// ```
///
/// FIFO ordering is the degenerate `WaitQueue<()>` case: every waiter shares
/// the same (unit) key, so they are served purely in insertion order.
#[derive(Clone)]
pub struct Resource {
    state: Rc<RefCell<WaitQueue<()>>>,
}

impl std::fmt::Debug for Resource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("Resource");
        if let Ok(q) = self.state.try_borrow() {
            d.field("in_use", &q.in_use())
                .field("capacity", &q.capacity())
                .field("queue_len", &q.live_waiters());
        }
        d.finish_non_exhaustive()
    }
}

impl Resource {
    /// Create a new resource pool with the given capacity.
    ///
    /// # Panics
    /// Panics if `capacity` is zero.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "Resource capacity must be at least 1");
        Resource {
            state: Rc::new(RefCell::new(WaitQueue::new(capacity))),
        }
    }

    /// Request one unit. Resolves immediately if a unit is available,
    /// otherwise suspends the calling process until one is released.
    ///
    /// The returned [`ResourceGuard`] releases the unit when dropped.
    ///
    /// The request joins the queue when it is first polled, not when this is
    /// called; use [`request_now`](Resource::request_now) to take a place in
    /// the queue at the call.
    #[must_use = "futures do nothing unless awaited"]
    pub fn request(&self) -> ResourceRequest {
        ResourceRequest {
            state: Rc::clone(&self.state),
            registered: false,
            consumed: false,
            canceled: Rc::new(Cell::new(false)),
            granted: Rc::new(Cell::new(false)),
            slot: None,
        }
    }

    /// Request one unit, taking a place in the queue **at the call**, as
    /// SimPy's `resource.request()` does, rather than at the first poll.
    ///
    /// A free unit is granted at once; otherwise the request queues behind
    /// the requests made before it, whether or not those have been awaited
    /// yet. Units are handed over in the order requests were made, and a
    /// request can be granted before it is ever awaited:
    /// [`is_granted`](ResourceRequest::is_granted) says whether it holds one.
    /// Awaiting it yields the [`ResourceGuard`]; dropping it unawaited withdraws
    /// it from the queue, or passes a unit it was granted on to the next waiter.
    ///
    /// Useful when a process asks for several resources at once and waits for
    /// whichever it gets first, or needs to know whether it would have to wait.
    ///
    /// ```
    /// use simu::{Resource, SimEnv};
    ///
    /// let mut env = SimEnv::with_seed(0);
    /// let desk = Resource::new(1);
    ///
    /// let first = desk.request_now(); // free: granted at the call
    /// let second = desk.request_now(); // queued behind `first`
    /// assert!(first.is_granted());
    /// assert!(!second.is_granted());
    /// assert_eq!(desk.queue_len(), 1);
    ///
    /// let h = env.handle();
    /// env.spawn(async move {
    ///     let guard = first.await;
    ///     h.timeout(4.0).await;
    ///     drop(guard); // hands the unit to `second`, which is not awaited yet
    ///     assert!(second.is_granted());
    ///     let _guard = second.await; // resolves at once
    ///     assert_eq!(h.now(), 4.0);
    /// });
    /// env.run();
    /// ```
    #[must_use = "dropping the request withdraws it from the queue"]
    pub fn request_now(&self) -> ResourceRequest {
        let canceled = Rc::new(Cell::new(false));
        let granted = Rc::new(Cell::new(false));
        let slot = {
            let mut state = self.state.borrow_mut();
            if state.try_acquire() {
                granted.set(true);
                None
            } else {
                // Empty until the first poll: a grant before then only sets
                // `granted`.
                let slot = WakerSlot::default();
                state.register_slot(
                    (),
                    Rc::clone(&slot),
                    Rc::clone(&canceled),
                    Rc::clone(&granted),
                );
                Some(slot)
            }
        };
        ResourceRequest {
            state: Rc::clone(&self.state),
            registered: slot.is_some(),
            consumed: false,
            canceled,
            granted,
            slot,
        }
    }

    /// Number of units currently in use.
    #[must_use]
    pub fn in_use(&self) -> usize {
        self.state.borrow().in_use()
    }

    /// Total capacity of this resource pool.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.state.borrow().capacity()
    }

    /// Number of processes currently queued waiting for a unit (SimPy's
    /// `len(resource.queue)`). Excludes abandoned (canceled) requests.
    #[must_use]
    pub fn queue_len(&self) -> usize {
        self.state.borrow().live_waiters()
    }
}

/// Future returned by [`Resource::request`] and [`Resource::request_now`].
///
/// Resolves to a [`ResourceGuard`] once a unit is acquired.
pub struct ResourceRequest {
    state: Rc<RefCell<WaitQueue<()>>>,
    /// Whether this request has already been enqueued in the wait queue.
    /// Prevents double-queuing on repeated polls.
    registered: bool,
    /// Set once this request has turned a granted/acquired unit into a
    /// `ResourceGuard`. Once consumed, `Drop` must not release: the guard owns
    /// the unit and will release it itself.
    consumed: bool,
    /// Shared with the queue entry; set to `true` on drop if the request was
    /// registered but never granted, so the release loop skips it.
    canceled: Rc<Cell<bool>>,
    /// Shared with the queue entry; set to `true` by `WaitQueue::release` when
    /// the unit is handed directly to this request. Checked first in `poll`.
    granted: Rc<Cell<bool>>,
    /// For a request queued at the call (`request_now`): the queue entry's
    /// waker slot, which each poll refills with the current waker.
    slot: Option<WakerSlot>,
}

impl ResourceRequest {
    /// Whether a unit is held for this request, so that awaiting it resolves
    /// without suspending.
    ///
    /// True from the moment a unit is granted to it: at the call for a
    /// [`request_now`](Resource::request_now) that found one free, or when a
    /// release hands one over. A [`request`](Resource::request) that has not
    /// been polled holds nothing, so it reports `false` even if a unit is free.
    #[must_use]
    pub fn is_granted(&self) -> bool {
        self.granted.get()
    }
}

impl std::fmt::Debug for ResourceRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceRequest")
            .field("registered", &self.registered)
            .field("granted", &self.granted.get())
            .finish_non_exhaustive()
    }
}

impl Future for ResourceRequest {
    type Output = ResourceGuard;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<ResourceGuard> {
        // Direct handoff: a released unit was transferred to us. Take it without
        // touching capacity — `in_use` already accounts for this unit.
        if self.granted.get() {
            self.consumed = true;
            return Poll::Ready(ResourceGuard {
                state: Rc::clone(&self.state),
            });
        }
        // Queued at the call: keep the entry's waker current, and wait for the
        // handoff. Never re-try the pool, so the queue order stands.
        if let Some(slot) = &self.slot {
            let mut slot = slot.borrow_mut();
            if !slot.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
                *slot = Some(cx.waker().clone());
            }
            return Poll::Pending;
        }
        // Drop the borrow before writing self.registered to satisfy the borrow checker.
        let acquired = {
            let mut state = self.state.borrow_mut();
            if state.try_acquire() {
                true
            } else {
                if !self.registered {
                    state.register(
                        (),
                        cx.waker().clone(),
                        Rc::clone(&self.canceled),
                        Rc::clone(&self.granted),
                    );
                }
                false
            }
        };
        if acquired {
            self.consumed = true;
            return Poll::Ready(ResourceGuard {
                state: Rc::clone(&self.state),
            });
        }
        self.registered = true;
        Poll::Pending
    }
}

impl Drop for ResourceRequest {
    fn drop(&mut self) {
        if self.consumed {
            return; // the guard owns the unit and will release it
        }
        if self.granted.get() {
            // A unit was handed to us but never turned into a guard (e.g. the
            // future was dropped before its re-poll). Pass it straight on to
            // the next waiter so it is not leaked.
            self.state.borrow_mut().release();
        } else if self.registered {
            self.canceled.set(true);
        }
    }
}

/// RAII guard that holds one unit of a [`Resource`].
///
/// The unit is released automatically when this value is dropped, waking the
/// next suspended requester (if any) in FIFO order.
pub struct ResourceGuard {
    state: Rc<RefCell<WaitQueue<()>>>,
}

impl std::fmt::Debug for ResourceGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceGuard").finish_non_exhaustive()
    }
}

impl Drop for ResourceGuard {
    fn drop(&mut self) {
        // Release one unit and wake the next live (non-canceled) waiter; the
        // WaitQueue skips abandoned entries so live waiters are not stranded.
        self.state.borrow_mut().release();
    }
}
