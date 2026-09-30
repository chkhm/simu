# simu — API Cheat-Sheet

At-a-glance signatures for the `simu` library (crates.io package: **`simu-des`**).
Semantics, examples, and the guided tutorial live on
[docs.rs](https://docs.rs/simu-des) — every public item there has full prose and
runnable doc-tests. For AI assistants there is a denser companion, [`llms.txt`](llms.txt).

## Environment

```rust
SimEnv::new() -> SimEnv                          // seeded from OS entropy (StdRng)
SimEnv::with_seed(seed: u64) -> SimEnv           // fixed seed → deterministic run
SimEnv::with_source(source: impl RandomSource) -> SimEnv  // custom feed (e.g. SplitMix64)

env.handle()       -> EnvHandle          // cheap Clone; pass into spawned processes
env.now()          -> f64
env.set_seed(seed: u64)                  // reseed the active RandomSource
env.spawn(future)  -> ProcessHandle<T>
env.timeout(delay: f64) -> Timeout       // panics if negative/non-finite
env.event()        -> (EventTrigger, EventAwaitable)
env.run()                                // until the event queue drains
env.run_until(t: f64)                    // monotonic — never rewinds

// EnvHandle: same now/timeout/event/spawn, plus
handle.rng() -> impl RngCore + '_        // short borrow — never hold across .await
```

## Events

```rust
let (trigger, awaitable) = env.event();
awaitable.clone().await;                 // Clone = multi-waiter; Future<Output = ()>
trigger.fire();                          // consumes self; latches for late awaiters
awaitable.is_fired()     -> bool          // has the trigger fired? (no await)
```

## Resources (all `Clone` — clones share one pool; all `!Send + !Sync`)

```rust
// FIFO pool
let r = Resource::new(capacity);               // panics if capacity == 0
let guard = r.request().await;                 // ResourceGuard; unit released on drop
let req = r.request_now();                     // queues at the call (SimPy), not at first poll
req.is_granted()     -> bool                   // holds a unit already? (no await)
r.capacity() / r.in_use() / r.queue_len() -> usize

// Priority pool (lower u32 = higher priority; FIFO within a level)
let r = PriorityResource::new(capacity);
let guard = r.request(priority: u32).await;    // PriorityResourceGuard

// Preemptive pool (urgent requests evict strictly-worse-priority holders)
let r = PreemptiveResource::new(capacity);
let guard = r.request(priority: u32).await;    // PreemptiveGuard
guard.preempted()    -> EventAwaitable         // race with your work via any_of!
guard.is_preempted() -> bool

// Continuous quantity (strict head-of-line FIFO on both queues)
let c = Container::new(capacity: f64, initial_level: f64);  // or Container::empty(cap)
c.put(amount: f64).await;                      // panics if amount <= 0 or > capacity
c.get(amount: f64).await;
c.level() / c.capacity() -> f64
c.get_queue_len() / c.put_queue_len() -> usize
```

## Processes & combinators

```rust
let ph: ProcessHandle<T> = handle.spawn(async move { /* … */ value });
let v: T = ph.await;                     // or drop to detach (fire-and-forget)
ph.discard().await;                      // adapt to Output = () for the macros

any_of![fut1, fut2].await;               // first wins; losers are dropped (cancelled)
all_of![fut1, fut2].await;               // barrier; arms must be Future<Output = ()>
```

## Monte Carlo & randomness

```rust
// Always available; `monte-carlo` feature switches std::thread → rayon pool.
let results: Vec<R> = monte_carlo::run(seeds, |seed| { /* build env inside */ });
// results in seed order; a worker panic re-raises on the caller thread

use simu::rng::{sample, SplitMix64};     // portable feed, mirrored in Python
sample::uniform01(&mut rng)              // [0, 1)
sample::exponential(&mut rng, mean)
sample::bernoulli(&mut rng, p)
sample::normal(&mut rng, mu, sigma)
```

## Key rules

| Rule | Detail |
|------|--------|
| `!Send + !Sync` | `SimEnv`, resources etc. cannot cross thread boundaries; use `monte_carlo::run` for parallelism |
| RNG borrow | `handle.rng()` returns a short-lived guard — sample immediately, do not hold across `.await` |
| RAII guards | `ResourceGuard` / `PriorityResourceGuard` / `PreemptiveGuard` release their unit on drop; drop early to free sooner (a *preempted* `PreemptiveGuard` drop is a no-op) |
| Cancelled requests | Dropping a `request().await` future mid-suspension marks its queue entry canceled; the entry is skipped on the next release and removed lazily (not eagerly) |
| Determinism | Same seed + same logic → identical event sequence every run |
| Panics | Programming misuse (wrong capacity, empty combinator, negative delay, etc.) panics; don't catch them |
