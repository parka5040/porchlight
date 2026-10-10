//! One LISTEN per process, regardless of the number of open SSE streams.
use sqlx::{PgPool, postgres::PgListener};
use std::{future::Future, time::Duration};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct StreamHub(broadcast::Sender<()>, tokio::sync::watch::Sender<bool>);
impl Default for StreamHub {
    fn default() -> Self {
        Self(
            broadcast::channel(128).0,
            tokio::sync::watch::channel(false).0,
        )
    }
}
impl StreamHub {
    /// Persistent shutdown state: a subscriber joining after shutdown cannot miss it.
    pub fn drain(&self) {
        self.1.send_replace(true);
    }
    pub fn is_draining(&self) -> bool {
        *self.1.borrow()
    }
    pub async fn draining(&self) {
        let mut rx = self.1.subscribe();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                break;
            }
        }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.0.subscribe()
    }
    /// Payloads are deliberately ignored: NOTIFY only wakes authoritative reads.
    pub async fn run(
        &self,
        pool: PgPool,
        shutdown: impl Future<Output = ()>,
    ) -> anyhow::Result<()> {
        self.run_ready(pool, shutdown, None).await
    }
    pub async fn run_ready(
        &self,
        pool: PgPool,
        shutdown: impl Future<Output = ()>,
        mut ready: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> anyhow::Result<()> {
        tokio::pin!(shutdown);
        let listeners = crate::db::listener_pool(&pool, "porchlight-stream-listener");
        let mut backoff = Duration::from_secs(1);
        loop {
            let connected = tokio::select! {
                ()=shutdown.as_mut()=>{ self.drain(); return Ok(()); },
                r=async { let mut l=PgListener::connect_with(&listeners).await?; l.listen("porchlight_stream").await?; Ok::<_,sqlx::Error>(l) }=>r,
            };
            if let Ok(mut listener) = connected {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(());
                }
                backoff = Duration::from_secs(1);
                // Repair missed notifications at reconnect. Heartbeats provide an independent repair path.
                let _ = self.0.send(());
                loop {
                    let notification = tokio::select! { ()=shutdown.as_mut()=>{ self.drain(); return Ok(()); }, r=listener.try_recv()=>r };
                    if notification.is_err() {
                        break;
                    }
                    backoff = Duration::from_secs(1);
                    let _ = self.0.send(());
                }
            }
            // Never log error details (connection options may contain credentials).
            tracing::warn!(?backoff, "stream hub reconnecting");
            tokio::select! { ()=shutdown.as_mut()=>{ self.drain(); return Ok(()); }, ()=tokio::time::sleep(backoff)=>{} }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn drain_survives_absent_and_late_subscribers() {
        let hub = super::StreamHub::default();
        hub.drain();
        let late = hub.clone();
        assert!(late.is_draining());
        tokio::time::timeout(std::time::Duration::from_millis(100), late.draining())
            .await
            .unwrap();
        // Repeated drain and notification subscription cannot clear the shutdown state.
        hub.drain();
        let _ = late.subscribe();
        assert!(late.is_draining());
    }
}
