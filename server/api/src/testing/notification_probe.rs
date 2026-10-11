//! Task-local decoded-source rows per query; independent of global tracing callsite interest.
tokio::task_local! {
    static SOURCE_QUERIES: std::cell::RefCell<Vec<usize>>;
}
pub async fn with_notification_source_probe<F: std::future::Future>(
    future: F,
) -> (F::Output, Vec<usize>) {
    SOURCE_QUERIES
        .scope(std::cell::RefCell::new(Vec::new()), async {
            let output = future.await;
            (output, SOURCE_QUERIES.with(|p| p.take()))
        })
        .await
}
pub(crate) fn record_notification_source_query(rows: usize) {
    let _ = SOURCE_QUERIES.try_with(|p| p.borrow_mut().push(rows));
}
