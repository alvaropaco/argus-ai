//! Concurrency and serialization bounds for privileged invocations.
//!
//! At most `max_concurrent_privileged` privileged invocations run at once, and
//! invocations targeting the same resource are serialized so two conflicting
//! operations never interleave. Serialization is per resource rather than global,
//! so operations on unrelated resources are not needlessly blocked (FR-047).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

/// Why a privileged invocation never reached the executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PrivilegedError {
    /// The concurrency cap was full for longer than the wait budget allowed.
    #[error("privileged execution is at its concurrency limit and the wait budget expired")]
    QueueBudgetExpired,

    /// The operation exceeded its timeout and was abandoned mid-flight.
    ///
    /// This is reported as a failure rather than a success: the operation may or
    /// may not have taken effect, so the only honest answer is that its state is
    /// unknown (ADR-0022 §5).
    #[error("the operation exceeded its timeout and its outcome is unknown")]
    TimedOut,
}

/// Bounds how many privileged invocations run at once and serializes them per
/// target resource.
pub struct PrivilegedLimiter {
    cap: usize,
    slots: Arc<Semaphore>,
    resource_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl PrivilegedLimiter {
    pub fn new(max_concurrent: u32) -> Self {
        let cap = max_concurrent.max(1) as usize;
        Self {
            cap,
            slots: Arc::new(Semaphore::new(cap)),
            resource_locks: Mutex::new(HashMap::new()),
        }
    }

    /// The configured concurrency ceiling.
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Acquires one of the concurrency slots, waiting at most `budget`.
    pub async fn acquire(&self, budget: Duration) -> Result<OwnedSemaphorePermit, PrivilegedError> {
        let slots = Arc::clone(&self.slots);
        match tokio::time::timeout(budget, slots.acquire_owned()).await {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) | Err(_) => Err(PrivilegedError::QueueBudgetExpired),
        }
    }

    fn lock_for(&self, resource: &str) -> Arc<AsyncMutex<()>> {
        let mut locks = self
            .resource_locks
            .lock()
            .expect("the resource lock table is not poisoned");
        Arc::clone(locks.entry(resource.to_string()).or_default())
    }

    /// Waits for exclusive access to one resource.
    pub async fn lock_resource(&self, resource: &str) -> OwnedMutexGuard<()> {
        self.lock_for(resource).lock_owned().await
    }

    /// Runs `operation` under the cap, the target resource's lock, and a timeout.
    ///
    /// The operation's own value is returned unchanged, so the caller keeps its
    /// error type. Only the concerns this module owns — the queue budget and the
    /// timeout — are reported here.
    pub async fn run<T, F>(
        &self,
        resource: &str,
        queue_budget: Duration,
        timeout_budget: Duration,
        operation: F,
    ) -> Result<T, PrivilegedError>
    where
        T: Send,
        F: std::future::Future<Output = T> + Send,
    {
        let _slot = self.acquire(queue_budget).await?;
        let lock = self.lock_for(resource);
        let _guard = lock.lock_owned().await;

        match tokio::time::timeout(timeout_budget, operation).await {
            Ok(value) => Ok(value),
            Err(_elapsed) => Err(PrivilegedError::TimedOut),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn the_cap_bounds_concurrent_acquisition() {
        let limiter = PrivilegedLimiter::new(1);
        assert_eq!(limiter.cap(), 1);

        let first = limiter.acquire(Duration::from_millis(50)).await.unwrap();
        let contended = limiter.acquire(Duration::from_millis(20)).await;
        assert_eq!(contended.unwrap_err(), PrivilegedError::QueueBudgetExpired);

        drop(first);
        assert!(limiter.acquire(Duration::from_millis(50)).await.is_ok());
    }

    #[tokio::test]
    async fn a_zero_cap_is_treated_as_one_rather_than_deadlocking() {
        let limiter = PrivilegedLimiter::new(0);
        assert_eq!(limiter.cap(), 1);
        assert!(limiter.acquire(Duration::from_millis(50)).await.is_ok());
    }

    #[tokio::test]
    async fn two_operations_on_one_resource_do_not_interleave() {
        let limiter = PrivilegedLimiter::new(2);
        let held = limiter.lock_resource("nginx.service").await;

        let contended = tokio::time::timeout(
            Duration::from_millis(30),
            limiter.lock_resource("nginx.service"),
        )
        .await;
        assert!(contended.is_err(), "the same resource must serialize");

        let unrelated = tokio::time::timeout(
            Duration::from_millis(30),
            limiter.lock_resource("postgres.service"),
        )
        .await;
        assert!(
            unrelated.is_ok(),
            "an unrelated resource must not be blocked"
        );

        drop(held);
    }

    #[tokio::test]
    async fn run_returns_the_operations_value() {
        let limiter = PrivilegedLimiter::new(4);
        let value = limiter
            .run(
                "nginx.service",
                Duration::from_millis(50),
                Duration::from_millis(50),
                async { 7u8 },
            )
            .await;
        assert_eq!(value.unwrap(), 7);
    }

    #[tokio::test]
    async fn a_timeout_is_reported_and_the_operation_abandoned() {
        let limiter = PrivilegedLimiter::new(1);
        let result: Result<(), PrivilegedError> = limiter
            .run(
                "nginx.service",
                Duration::from_millis(50),
                Duration::from_millis(20),
                async {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                },
            )
            .await;

        assert_eq!(result.unwrap_err(), PrivilegedError::TimedOut);
    }

    #[tokio::test]
    async fn conflicting_operations_on_one_resource_run_strictly_one_at_a_time() {
        let limiter = Arc::new(PrivilegedLimiter::new(2));
        let concurrent = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..4 {
            let limiter = Arc::clone(&limiter);
            let concurrent = Arc::clone(&concurrent);
            let peak = Arc::clone(&peak);
            handles.push(tokio::spawn(async move {
                limiter
                    .run(
                        "nginx.service",
                        Duration::from_secs(1),
                        Duration::from_secs(1),
                        async move {
                            let now = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                            peak.fetch_max(now, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(10)).await;
                            concurrent.fetch_sub(1, Ordering::SeqCst);
                        },
                    )
                    .await
            }));
        }

        for handle in handles {
            handle.await.unwrap().expect("every operation runs");
        }

        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "operations on one resource must never overlap"
        );
    }
}
