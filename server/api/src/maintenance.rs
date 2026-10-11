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

pub struct Fence {
    path: PathBuf,
    cached: Mutex<Option<(Instant, bool)>>,
}
impl Fence {
    pub fn new(path: PathBuf) -> Self {
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
        let active = match std::fs::symlink_metadata(&self.path) {
            Ok(_) => true,
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        };
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
