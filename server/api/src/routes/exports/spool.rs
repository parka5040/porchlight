//! A detached bounded producer owns the spool, so an unpolled body cannot retain its file.
use axum::body::Body;
use futures_util::{StreamExt, stream};
use std::{io, time::Duration};
use tokio::io::AsyncRead;

pub(super) fn body(
    reader: impl AsyncRead + Unpin + Send + 'static,
    idle: Duration,
    guard: impl Send + 'static,
) -> Body {
    let (send, receive) = tokio::sync::mpsc::channel(2);
    let task = tokio::spawn(async move {
        let _guard = guard;
        let mut reader = tokio_util::io::ReaderStream::with_capacity(reader, 64 * 1024);
        while let Some(chunk) = tokio::select! {
            _ = send.closed() => return Ok(()),
            chunk = reader.next() => chunk,
        } {
            send.send_timeout(chunk?, idle)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "export download stalled"))?;
        }
        Ok::<_, io::Error>(())
    });
    let chunks = stream::unfold(receive, |mut receive| async {
        receive
            .recv()
            .await
            .map(|chunk| (Ok::<_, io::Error>(chunk), receive))
    });
    // Propagate timeout/read failure after queued chunks instead of a successful truncated EOF.
    let completion = stream::once(async move {
        task.await
            .unwrap_or_else(|_| Err(io::Error::other("export reader failed")))
            .err()
            .map(Err)
    })
    .filter_map(std::future::ready);
    Body::from_stream(chunks.chain(completion))
}

#[cfg(test)]
mod audit_b_final_server;

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use std::{
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        task::{Context, Poll},
    };
    use tokio::io::ReadBuf;

    struct WatchedReader {
        bytes: std::io::Cursor<Vec<u8>>,
        dropped: Arc<AtomicBool>,
    }
    impl AsyncRead for WatchedReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.bytes).poll_read(cx, buf)
        }
    }
    impl Drop for WatchedReader {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }
    #[tokio::test(start_paused = true)]
    async fn unpolled_body_releases_reader_on_idle_and_reports_incomplete() {
        let dropped = Arc::new(AtomicBool::new(false));
        let body = body(
            WatchedReader {
                bytes: std::io::Cursor::new(vec![0; 1024 * 1024]),
                dropped: dropped.clone(),
            },
            Duration::from_secs(30),
            (),
        );
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(31)).await;
        tokio::task::yield_now().await;
        assert!(
            dropped.load(Ordering::Acquire),
            "unpolled body retained spool after idle deadline"
        );
        assert!(
            body.collect().await.is_err(),
            "stalled spool must not finish successfully"
        );
    }
}
