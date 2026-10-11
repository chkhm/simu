use simu::SimEnv;
fn main() {
    let mut env = SimEnv::new(42);
    let h = env.handle();
    env.spawn(async move { h.timeout(5.0).await; println!("done at {}", h.now()); });
    env.run();
}
