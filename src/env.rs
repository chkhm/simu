// SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::cell::RefCell;
use std::cmp::Reverse;
use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Waker};

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use crate::event::{new_event, EventAwaitable, EventTrigger};
use crate::executor::{make_waker, ProcessEntry, SimState};
use crate::process::{spawn_with_handle, ProcessHandle};
use crate::rng::RandomSource;
use crate::timeout::Timeout;

/// The boxed, pluggable randomness source shared by a `SimEnv` and its handles.
type SharedRng = Rc<RefCell<Box<dyn RandomSource>>>;

/// The simulation environment. Central coordinator for a single simulation run.
///
/// `SimEnv` is `!Send + !Sync` (via `Rc`) and must live on one thread.
/// For Monte Carlo parallelism, create independent `SimEnv` instances on
/// separate threads.
///
/// The typical shape of every simulation: create the env, pass
/// [`handle()`](SimEnv::handle) clones into spawned processes, run, read out
/// results.
///
/// ```
/// use simu::SimEnv;
///
/// let mut env = SimEnv::with_seed(1);
/// let h = env.handle();
/// env.spawn(async move {
///     h.timeout(10.0).await; // suspend for 10 simulated time units
/// });
/// env.run(); // drive the event loop until the queue drains
/// assert_eq!(env.now(), 10.0);
/// ```
pub struct SimEnv {
    state: Rc<RefCell<SimState>>,
    rng: SharedRng,
}

/// A lightweight handle to the simulation environment, intended to be cloned
/// and passed into spawned processes.
///
/// Both `SimEnv` and all `EnvHandle` clones share the same underlying
/// `SimState` and the same [`RandomSource`](crate::rng::RandomSource).
#[derive(Clone)]
pub struct EnvHandle {
    state: Rc<RefCell<SimState>>,
    rng: SharedRng,
}

impl Default for SimEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SimEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("SimEnv");
        if let Ok(state) = self.state.try_borrow() {
            let live = state.processes.iter().filter(|p| p.is_some()).count();
            d.field("now", &state.current_time)
                .field("queued_events", &state.event_queue.len())
                .field("processes", &live);
        }
        d.finish_non_exhaustive()
    }
}

impl std::fmt::Debug for EnvHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("EnvHandle");
        if let Ok(state) = self.state.try_borrow() {
            d.field("now", &state.current_time);
        }
        d.finish_non_exhaustive()
    }
}

impl SimEnv {
    /// Create a new environment seeded from OS entropy.
    ///
    /// Use [`with_seed`](SimEnv::with_seed) when reproducibility is required.
    #[must_use]
    pub fn new() -> Self {
        SimEnv::from_source(Box::new(StdRng::from_os_rng()))
    }

    /// Create a new environment with a fixed RNG seed.
    ///
    /// Given the same seed and process logic the simulation will produce
    /// identical results across runs. Uses `rand`'s `StdRng`; for a portable,
    /// cross-language stream use [`with_source`](SimEnv::with_source) with a
    /// [`SplitMix64`](crate::rng::SplitMix64) feed instead.
    #[must_use]
    pub fn with_seed(seed: u64) -> Self {
        SimEnv::from_source(Box::new(StdRng::seed_from_u64(seed)))
    }

    /// Create a new environment driven by a custom [`RandomSource`].
    ///
    /// Use this to plug in an external feed — e.g. the portable
    /// [`SplitMix64`](crate::rng::SplitMix64) generator that the SimPy
    /// comparison harness re-implements in Python:
    ///
    /// ```
    /// use simu::{SimEnv, rng::SplitMix64};
    /// let env = SimEnv::with_source(SplitMix64::new(42));
    /// ```
    #[must_use]
    pub fn with_source<R: RandomSource + 'static>(source: R) -> Self {
        SimEnv::from_source(Box::new(source))
    }

    fn from_source(source: Box<dyn RandomSource>) -> Self {
        SimEnv {
            state: Rc::new(RefCell::new(SimState::new())),
            rng: Rc::new(RefCell::new(source)),
        }
    }

    /// Re-seed the environment's randomness source, restarting its stream.
    ///
    /// Takes `&mut self` for consistency with [`run`](SimEnv::run) — reseeding
    /// mid-run would change the draw stream, so it is an owner-level operation.
    /// Delegates to [`RandomSource::reseed`]; panics if the active source does
    /// not support reseeding.
    pub fn set_seed(&mut self, seed: u64) {
        self.rng.borrow_mut().reseed(seed);
    }

    /// Return a cloneable handle suitable for passing into spawned processes.
    #[must_use]
    pub fn handle(&self) -> EnvHandle {
        EnvHandle {
            state: Rc::clone(&self.state),
            rng: Rc::clone(&self.rng),
        }
    }

    /// Current simulation time.
    #[must_use]
    pub fn now(&self) -> f64 {
        self.state.borrow().current_time
    }

    /// Spawn a process. The future is queued and will be polled on the next
    /// executor iteration. May be called before or during `run()`.
    ///
    /// Returns a [`ProcessHandle`] that resolves to the process's output when
    /// it finishes. Drop the handle to detach (fire-and-forget).
    pub fn spawn<F>(&self, future: F) -> ProcessHandle<F::Output>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        self.handle().spawn(future)
    }

    /// Create a `Timeout` that resolves after `delay` simulated time units.
    ///
    /// # Panics
    ///
    /// Panics if `delay` is negative or not finite — see
    /// [`EnvHandle::timeout`].
    #[must_use = "futures do nothing unless awaited"]
    pub fn timeout(&self, delay: f64) -> Timeout {
        self.handle().timeout(delay)
    }

    /// Create a paired `(EventTrigger, EventAwaitable)` for inter-process signalling.
    #[must_use]
    pub fn event(&self) -> (EventTrigger, EventAwaitable) {
        new_event()
    }

    /// Run until the event queue is empty.
    pub fn run(&mut self) {
        self.drain_pending_spawns();
        self.poll_ready();

        loop {
            let next = self.state.borrow_mut().event_queue.pop();
            match next {
                None => break,
                Some(Reverse(entry)) => {
                    self.state.borrow_mut().current_time = entry.time;
                    entry.waker.wake();
                    self.drain_pending_spawns();
                    self.poll_ready();
                }
            }
        }
    }

    /// Run until simulated time reaches `until`.
    ///
    /// When the event queue empties (or its next event is beyond `until`) the
    /// clock advances *to* `until` — but never backwards: simulated time is
    /// monotonic, so calling `run_until` with a boundary at or before the
    /// current time is a no-op that leaves `now()` unchanged. An event scheduled
    /// exactly at `until` is not run (its time is not strictly less than the
    /// boundary), yet `now()` will report `until`.
    pub fn run_until(&mut self, until: f64) {
        self.drain_pending_spawns();
        self.poll_ready();

        loop {
            let should_stop = {
                let state = self.state.borrow();
                state
                    .event_queue
                    .peek()
                    .is_none_or(|Reverse(e)| e.time >= until)
            };

            if should_stop {
                // Advance to the boundary, but never rewind: time is monotonic.
                let mut state = self.state.borrow_mut();
                state.current_time = until.max(state.current_time);
                break;
            }

            let next = self.state.borrow_mut().event_queue.pop();
            if let Some(Reverse(entry)) = next {
                self.state.borrow_mut().current_time = entry.time;
                entry.waker.wake();
                self.drain_pending_spawns();
                self.poll_ready();
            }
        }
    }

    /// Move all pending spawns into the process table and mark them ready.
    fn drain_pending_spawns(&self) {
        let spawns: Vec<_> = std::mem::take(&mut self.state.borrow_mut().pending_spawns);
        if spawns.is_empty() {
            return;
        }
        // Collect IDs before mutating processes so we can batch the ready_queue
        // push without holding two borrows of SimState simultaneously.
        let ids: Vec<usize> = spawns.iter().map(|(id, _)| *id).collect();
        {
            let mut state = self.state.borrow_mut();
            let ready_queue = Arc::clone(&state.ready_queue);
            for (id, future) in spawns {
                // Cache one waker per process now, at admission (see ProcessEntry).
                let waker = make_waker(id, Arc::clone(&ready_queue));
                if id >= state.processes.len() {
                    state.processes.resize_with(id + 1, || None);
                }
                state.processes[id] = Some(ProcessEntry { future, waker });
            }
        }
        // Clone the Arc so the Ref<SimState> is dropped before we lock.
        let rq = Arc::clone(&self.state.borrow().ready_queue);
        rq.lock().unwrap().extend(ids);
    }

    /// Poll every process in the ready queue until the queue is empty.
    ///
    /// Each process is taken out of its table slot before polling so that it can
    /// freely borrow `SimState` via its `EnvHandle` without triggering a
    /// `RefCell` panic. Processes that return `Pending` are put back. A slot that
    /// is already `None` (a completed process, or a duplicate wake for one whose
    /// entry is currently taken) is simply skipped — polling a stale id is benign.
    fn poll_ready(&self) {
        // Clone the ready_queue Arc once; it never changes after construction.
        let ready_queue = Arc::clone(&self.state.borrow().ready_queue);

        loop {
            let ready: Vec<usize> = std::mem::take(&mut *ready_queue.lock().unwrap());

            if ready.is_empty() {
                break;
            }

            for id in ready {
                // `id` was allocated by `alloc_process_id`, so the slot exists.
                let entry = self.state.borrow_mut().processes[id].take();

                if let Some(mut entry) = entry {
                    let mut cx = Context::from_waker(&entry.waker);

                    if entry.future.as_mut().poll(&mut cx).is_pending() {
                        self.state.borrow_mut().processes[id] = Some(entry);
                    }

                    // A process may spawn children during its poll.
                    self.drain_pending_spawns();
                }
            }
        }
    }
}

impl Drop for SimEnv {
    /// Break the `SimState` ↔ process reference cycle on teardown.
    ///
    /// A suspended process future captures an `EnvHandle`, which holds an
    /// `Rc<RefCell<SimState>>` — so `SimState → processes → future → EnvHandle →
    /// SimState` is a cycle. If a simulation ends with processes still suspended
    /// (e.g. one blocked forever on a resource that never frees), that cycle
    /// keeps the whole `SimState` alive and leaks it; across many replications
    /// the leak accumulates. Clearing the process tables drops those futures,
    /// releasing their handles so `SimState` can be reclaimed.
    fn drop(&mut self) {
        // Move the tables out from under the borrow, then drop them *after*
        // releasing it: a suspended future's destructor may itself touch the
        // env (e.g. deregistering a waiter), which would re-enter the borrow.
        let leftovers = self.state.try_borrow_mut().ok().map(|mut state| {
            (
                std::mem::take(&mut state.processes),
                std::mem::take(&mut state.pending_spawns),
            )
        });
        drop(leftovers);
    }
}

impl EnvHandle {
    /// Current simulation time.
    #[must_use]
    pub fn now(&self) -> f64 {
        self.state.borrow().current_time
    }

    /// Borrow the shared RNG mutably.
    ///
    /// The returned guard implements `rand::RngCore`, so it works with the
    /// [`rng::sample`](crate::rng::sample) transforms and with `rand_distr`
    /// distributions alike. Sample *before* awaiting — the guard cannot be held
    /// across an `.await` point.
    ///
    /// ```
    /// use simu::{SimEnv, rng::sample};
    /// let env = SimEnv::with_seed(0);
    /// let h = env.handle();
    /// let duration = sample::exponential(&mut h.rng(), 20.0); // mean = 20
    /// assert!(duration >= 0.0);
    /// ```
    #[must_use = "an RngGuard holds a mutable borrow of the env RNG; bind or use it directly"]
    pub fn rng(&self) -> impl rand::RngCore + '_ {
        RngGuard(self.rng.borrow_mut())
    }

    /// Create a `Timeout` that resolves after `delay` simulated time units.
    ///
    /// The deadline is computed **when the `Timeout` is created**
    /// (`now() + delay`), not when it is first awaited — relevant when a
    /// `Timeout` is stored and raced later inside `any_of!`.
    ///
    /// # Panics
    ///
    /// Panics if `delay` is negative or not finite (NaN / infinity). Simulated
    /// time is monotonic, so a negative delay is a programming error; a zero
    /// delay is allowed and fires on the next event-loop iteration.
    #[must_use = "futures do nothing unless awaited"]
    pub fn timeout(&self, delay: f64) -> Timeout {
        assert!(
            delay >= 0.0 && delay.is_finite(),
            "timeout delay must be finite and non-negative (got {delay})"
        );
        let deadline = self.state.borrow().current_time + delay;
        Timeout::new(deadline, self.clone())
    }

    /// Create a paired `(EventTrigger, EventAwaitable)` for inter-process signalling.
    #[must_use]
    pub fn event(&self) -> (EventTrigger, EventAwaitable) {
        new_event()
    }

    /// Spawn a child process from within a running process.
    ///
    /// Returns a [`ProcessHandle`] that resolves to the process's output when
    /// it finishes. Drop the handle to detach (fire-and-forget).
    pub fn spawn<F>(&self, future: F) -> ProcessHandle<F::Output>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        let (wrapped, handle) = spawn_with_handle(future);
        let mut state = self.state.borrow_mut();
        let id = state.alloc_process_id();
        state.pending_spawns.push((id, wrapped));
        handle
    }

    /// Schedule a wakeup at `deadline` in the event queue.
    ///
    /// Called by [`Timeout`] — the only crate-internal user that needs direct
    /// access to the event queue.
    pub(crate) fn schedule_wakeup(&self, deadline: f64, waker: Waker) {
        self.state.borrow_mut().schedule_wakeup(deadline, waker);
    }
}

/// Newtype wrapper so `EnvHandle::rng()` can return an `impl RngCore + '_`
/// without exposing `RefMut` or the boxed source in the public API.
struct RngGuard<'a>(std::cell::RefMut<'a, Box<dyn RandomSource>>);

impl RngCore for RngGuard<'_> {
    fn next_u32(&mut self) -> u32 {
        (**self.0).next_u32()
    }
    fn next_u64(&mut self) -> u64 {
        (**self.0).next_u64()
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        (**self.0).fill_bytes(dest)
    }
}
