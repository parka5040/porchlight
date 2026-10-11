use super::mutation_concurrency::SqlstateLogs;

#[test]
fn scoped_sqlstate_capture_excludes_concurrent_dispatcher() {
    // Use the same callsite on both threads, keeping both dispatchers active
    // across every emission. No elapsed-time assumption controls the overlap.
    fn emit(sqlstate: &str) {
        tracing::warn!(sqlstate, "scoped SQLSTATE fixture");
    }
    let logs = SqlstateLogs::default();
    let target = logs.dispatch();
    let unrelated = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(std::io::sink)
            .finish(),
    );
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            tracing::dispatcher::with_default(&unrelated, || {
                for _ in 0..64 {
                    barrier.wait();
                    emit("40P01");
                    barrier.wait();
                }
            })
        });
        tracing::dispatcher::with_default(&target, || {
            for _ in 0..64 {
                barrier.wait();
                emit("55P03");
                barrier.wait();
            }
        });
    });
    logs.assert_only_nowait(64);
}
