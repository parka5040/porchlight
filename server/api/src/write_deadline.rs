//! A write-progress deadline at the connection IO layer, not in the response producer.
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    time::Sleep,
};

pub struct WriteDeadline<I> {
    inner: I,
    deadline: Duration,
    pending: Option<Pin<Box<Sleep>>>,
}
impl<I> WriteDeadline<I> {
    pub fn new(inner: I, deadline: Duration) -> Self {
        Self {
            inner,
            deadline,
            pending: None,
        }
    }
    fn expired(&mut self, cx: &mut Context<'_>) -> bool {
        self.pending
            .as_mut()
            .is_some_and(|timer| timer.as_mut().poll(cx).is_ready())
    }
    fn track<T>(
        &mut self,
        cx: &mut Context<'_>,
        result: Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        match result {
            Poll::Ready(result) => {
                self.pending = None;
                Poll::Ready(result)
            }
            Poll::Pending => {
                self.pending
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(self.deadline)));
                if self.expired(cx) {
                    Poll::Ready(Err(stalled()))
                } else {
                    Poll::Pending
                }
            }
        }
    }
}
fn stalled() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "connection write stalled")
}
impl<I: AsyncRead + Unpin> AsyncRead for WriteDeadline<I> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}
impl<I: AsyncWrite + Unpin> AsyncWrite for WriteDeadline<I> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.expired(cx) {
            return Poll::Ready(Err(stalled()));
        }
        let result = Pin::new(&mut this.inner).poll_write(cx, bytes);
        this.track(cx, result)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.expired(cx) {
            return Poll::Ready(Err(stalled()));
        }
        let result = Pin::new(&mut this.inner).poll_write_vectored(cx, bufs);
        this.track(cx, result)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.expired(cx) {
            return Poll::Ready(Err(stalled()));
        }
        let result = Pin::new(&mut this.inner).poll_flush(cx);
        this.track(cx, result)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.expired(cx) {
            return Poll::Ready(Err(stalled()));
        }
        let result = Pin::new(&mut this.inner).poll_shutdown(cx);
        this.track(cx, result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test(start_paused = true)]
    async fn pending_write_expires_but_idle_sse_heartbeats_survive() {
        let (server, mut client) = tokio::io::duplex(1);
        let mut server = WriteDeadline::new(server, Duration::from_secs(1));
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_secs(120)).await;
            server.write_all(b":").await.unwrap();
            server.flush().await.unwrap();
            assert_eq!(client.read_u8().await.unwrap(), b':');
        }
        server.write_all(b"x").await.unwrap();
        let error = server.write_all(b"blocked").await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
    struct Vectored(tokio::io::DuplexStream);
    impl AsyncWrite for Vectored {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &[u8],
        ) -> Poll<io::Result<usize>> {
            panic!("vectored write must not flatten to scalar writes")
        }
        fn is_write_vectored(&self) -> bool {
            true
        }
        fn poll_write_vectored(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bufs: &[io::IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            Pin::new(&mut self.get_mut().0).poll_write_vectored(cx, bufs)
        }
        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().0).poll_flush(cx)
        }
        fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
        }
    }
    #[tokio::test(start_paused = true)]
    async fn vectored_writes_delegate_and_expire() {
        let (server, mut client) = tokio::io::duplex(1);
        let mut server = WriteDeadline::new(Vectored(server), Duration::from_secs(1));
        assert!(
            server.is_write_vectored(),
            "must preserve vectored capability"
        );
        assert_eq!(
            server
                .write_vectored(&[io::IoSlice::new(b"a"), io::IoSlice::new(b"b")])
                .await
                .unwrap(),
            1
        );
        assert_eq!(client.read_u8().await.unwrap(), b'a');
        assert_eq!(
            server
                .write_vectored(&[io::IoSlice::new(b"b")])
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            server
                .write_vectored(&[io::IoSlice::new(b"c")])
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }
    #[tokio::test(start_paused = true)]
    async fn slow_progressing_download_outlives_deadline() {
        let (server, mut client) = tokio::io::duplex(1);
        let mut server = WriteDeadline::new(server, Duration::from_secs(1));
        let started = tokio::time::Instant::now();
        let reader = tokio::spawn(async move {
            let mut bytes = Vec::new();
            for _ in 0..16 {
                tokio::time::sleep(Duration::from_millis(250)).await;
                bytes.push(client.read_u8().await.unwrap());
            }
            bytes
        });
        server.write_all(b"progressing-body").await.unwrap();
        server.shutdown().await.unwrap();
        assert_eq!(reader.await.unwrap(), b"progressing-body");
        assert!(started.elapsed() > Duration::from_secs(1));
    }
    struct PendingFlush;
    impl AsyncWrite for PendingFlush {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            b: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(b.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    #[tokio::test(start_paused = true)]
    async fn pending_flush_has_its_own_wakeup() {
        let mut stream = WriteDeadline::new(PendingFlush, Duration::from_millis(10));
        assert_eq!(
            stream.flush().await.unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }
}
