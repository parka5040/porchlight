//! Installer write fence: readiness/reads remain available while rollback is armed.
use crate::{
    error::{ApiError, Problem},
    state::AppState,
};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use std::{
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Debug, PartialEq)]
enum Phase {
    Absent,
    Pending,
    Committed,
    Unknown,
}
fn phase(path: &std::path::Path) -> Phase {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Phase::Absent,
        Ok(metadata) if metadata.is_file() && metadata.len() <= 128 => {}
        _ => return Phase::Unknown,
    }
    match std::fs::read_to_string(path).as_deref() {
        Ok("porchlight-upgrade-v1 pending\n" | "upgrade pending\n") => Phase::Pending,
        Ok("porchlight-upgrade-v1 committed\n") => Phase::Committed,
        _ => Phase::Unknown,
    }
}

pub struct Fence {
    path: PathBuf,
    cached: Mutex<Option<(Instant, bool)>>,
}
impl Fence {
    pub fn new(path: PathBuf) -> Self {
        if phase(&path) == Phase::Committed {
            tracing::warn!(
                "committed maintenance fence: recovery is forward-only; never restore the pre-upgrade dump"
            );
        }
        Self {
            path,
            cached: Mutex::new(None),
        }
    }
    pub(crate) fn active(&self) -> bool {
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((checked, active)) = *cached
            && checked.elapsed() < Duration::from_millis(250)
        {
            return active;
        }
        // Fail closed on inaccessible paths; only definite absence permits mutations.
        let active = phase(&self.path) != Phase::Absent;
        *cached = Some((Instant::now(), active));
        active
    }
}

pub async fn require_unfenced(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if !req.method().is_safe() && state.maintenance.active() {
        return Err(ApiError::new(Problem::Maintenance).retry(1));
    }
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_reads_durable_phase_and_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("maintenance");
        assert_eq!(phase(&path), Phase::Absent);
        for (text, expected) in [
            ("porchlight-upgrade-v1 pending\n", Phase::Pending),
            ("porchlight-upgrade-v1 committed\n", Phase::Committed),
            ("upgrade pending\n", Phase::Pending),
            ("", Phase::Unknown),
            ("unknown phase", Phase::Unknown),
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(phase(&path), expected);
            assert!(Fence::new(path.clone()).active());
        }
    }
}
