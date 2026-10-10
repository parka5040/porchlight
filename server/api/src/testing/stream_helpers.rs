//! Incremental SSE test client; dropping the stream closes its connection.
use super::*;
use futures_util::Stream;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Debug, Default)]
pub struct SseFrame {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: String,
    pub comment: Option<String>,
}
pub struct TestStream {
    pub status: u16,
    pub headers: HeaderMap,
    frames: tokio_stream::wrappers::ReceiverStream<SseFrame>,
    task: JoinHandle<()>,
    connection: JoinHandle<Result<(), hyper::Error>>,
}
impl Stream for TestStream {
    type Item = SseFrame;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.frames).poll_next(cx) {
            Poll::Ready(None) => Pin::new(&mut self.task).poll(cx).map(|result| {
                result.expect("SSE reader failed instead of reaching body EOF");
                None
            }),
            other => other,
        }
    }
}
impl Drop for TestStream {
    fn drop(&mut self) {
        self.task.abort();
        self.connection.abort();
    }
}
impl TestClient {
    pub async fn stream(
        &self,
        addr: SocketAddr,
        req: Request<Full<Bytes>>,
    ) -> Result<TestStream, TestClientError> {
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(self.connect(addr).await?)).await?;
        let connection = tokio::spawn(connection);
        let response =
            tokio::time::timeout(Duration::from_secs(5), sender.send_request(req)).await??;
        let (parts, mut body) = response.into_parts();
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        let task = tokio::spawn(async move {
            let _sender = sender;
            let mut buffer = Vec::new();
            while let Some(chunk) = body.frame().await {
                let chunk = chunk.expect("SSE HTTP body error");
                if let Ok(data) = chunk.into_data() {
                    buffer.extend_from_slice(&data);
                }
                while let Some(end) = buffer.windows(2).position(|w| w == b"\n\n") {
                    let text = String::from_utf8(buffer.drain(..end + 2).collect()).unwrap();
                    let mut frame = SseFrame::default();
                    for line in text.lines() {
                        if let Some(value) = line.strip_prefix(':') {
                            frame.comment = Some(value.trim_start().to_owned());
                        } else if let Some((key, value)) = line.split_once(':') {
                            let value = value.strip_prefix(' ').unwrap_or(value);
                            match key {
                                "id" => frame.id = Some(value.into()),
                                "event" => frame.event = Some(value.into()),
                                "data" => {
                                    if !frame.data.is_empty() {
                                        frame.data.push('\n');
                                    }
                                    frame.data.push_str(value);
                                }
                                _ => {}
                            }
                        }
                    }
                    if tx.send(frame).await.is_err() {
                        return;
                    }
                }
            }
        });
        Ok(TestStream {
            status: parts.status.as_u16(),
            headers: parts.headers,
            frames: tokio_stream::wrappers::ReceiverStream::new(rx),
            task,
            connection,
        })
    }
}
