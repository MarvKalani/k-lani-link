//! Admission for the bodies buffered before requests reach a computer's gateway.
use super::failure;
use axum::{
    body::{Body, Bytes},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use k_lani_link_protocol::{MAX_BODY_BYTES, UPLOAD_SECONDS};
use std::{sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub(super) struct Budget {
    slots: Arc<Semaphore>,
    timeout: Duration,
}

pub(super) enum Rejected {
    Capacity,
    Body,
    Timeout,
}

impl IntoResponse for Rejected {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Capacity => (StatusCode::TOO_MANY_REQUESTS, "request capacity exhausted"),
            Self::Body => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body unavailable or too large",
            ),
            Self::Timeout => (StatusCode::REQUEST_TIMEOUT, "request body upload timed out"),
        };
        failure(status, message)
    }
}

impl Default for Budget {
    fn default() -> Self {
        // The panel's gateways allow eight running and sixteen queued requests each.
        // This is a global bound on bodies waiting to be handed to any of them.
        Self::new(24, Duration::from_secs(UPLOAD_SECONDS))
    }
}

impl Budget {
    pub(super) fn new(capacity: usize, timeout: Duration) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(capacity)),
            timeout,
        }
    }

    pub(super) fn reserve(&self) -> Result<OwnedSemaphorePermit, Rejected> {
        self.slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Rejected::Capacity)
    }

    pub(super) async fn read(&self, body: Body) -> Result<Bytes, Rejected> {
        match tokio::time::timeout(self.timeout, axum::body::to_bytes(body, MAX_BODY_BYTES)).await {
            Ok(Ok(bytes)) => Ok(bytes),
            Ok(Err(_)) => Err(Rejected::Body),
            Err(_) => Err(Rejected::Timeout),
        }
    }
}
