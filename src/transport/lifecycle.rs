use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::JoinSet;
use tokio::time::timeout;

pub fn try_acquire_connection(
    limiter: &Arc<Semaphore>,
    listener: &'static str,
    peer: Option<SocketAddr>,
) -> Option<OwnedSemaphorePermit> {
    match limiter.clone().try_acquire_owned() {
        Ok(permit) => Some(permit),
        Err(_) => {
            tracing::warn!(listener, peer = ?peer, "Connection limit reached; dropping connection");
            None
        }
    }
}

#[derive(Clone)]
pub struct ConnectionSupervisor {
    limiter: Arc<Semaphore>,
    tracked_tasks: Arc<TrackedTasks>,
}

struct TrackedTasks {
    active: AtomicUsize,
    updates: watch::Sender<usize>,
    _anchor: watch::Receiver<usize>,
}

impl TrackedTasks {
    fn new() -> Self {
        let (updates, anchor) = watch::channel(0);
        Self {
            active: AtomicUsize::new(0),
            updates,
            _anchor: anchor,
        }
    }

    fn increment(&self) {
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.updates.send(active);
    }

    fn decrement(&self) {
        let active = self.active.fetch_sub(1, Ordering::AcqRel).saturating_sub(1);
        let _ = self.updates.send(active);
    }
}

struct TrackedTaskGuard {
    tasks: Arc<TrackedTasks>,
}

impl Drop for TrackedTaskGuard {
    fn drop(&mut self) {
        self.tasks.decrement();
    }
}

impl ConnectionSupervisor {
    pub fn new(max_connections: usize) -> Self {
        Self {
            limiter: Arc::new(Semaphore::new(max_connections)),
            tracked_tasks: Arc::new(TrackedTasks::new()),
        }
    }

    pub fn limiter(&self) -> Arc<Semaphore> {
        self.limiter.clone()
    }

    pub fn try_acquire(
        &self,
        listener: &'static str,
        peer: Option<SocketAddr>,
    ) -> Option<OwnedSemaphorePermit> {
        try_acquire_connection(&self.limiter, listener, peer)
    }

    pub fn spawn_connection<F>(
        &self,
        listener: &'static str,
        peer: Option<SocketAddr>,
        permit: OwnedSemaphorePermit,
        future: F,
    ) where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(async move {
            let _conn_permit = permit;
            tracing::trace!(listener, peer = ?peer, "connection task started");
            future.await;
            tracing::trace!(listener, peer = ?peer, "connection task finished");
        });
    }

    pub fn spawn_tracked<F>(&self, task: &'static str, peer: Option<SocketAddr>, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.tracked_tasks.increment();
        let tasks = self.tracked_tasks.clone();
        tokio::spawn(async move {
            let _guard = TrackedTaskGuard { tasks };
            tracing::trace!(task, peer = ?peer, "transport task started");
            future.await;
            tracing::trace!(task, peer = ?peer, "transport task finished");
        });
    }

    pub async fn wait_for_tracked_tasks(&self) {
        let mut updates = self.tracked_tasks.updates.subscribe();
        loop {
            if *updates.borrow() == 0 {
                break;
            }
            if updates.changed().await.is_err() {
                break;
            }
        }
    }
}

pub async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    let _ = shutdown.changed().await;
}

pub async fn drain_runtime(
    listener_tasks: &mut JoinSet<()>,
    connections: ConnectionSupervisor,
    max_connections: usize,
    grace: Duration,
) {
    let permits_to_wait = u32::try_from(max_connections).unwrap_or(u32::MAX);
    let drain = async {
        while listener_tasks.join_next().await.is_some() {}
        if permits_to_wait > 0 {
            match connections
                .limiter()
                .clone()
                .acquire_many_owned(permits_to_wait)
                .await
            {
                Ok(permits) => drop(permits),
                Err(e) => tracing::debug!(error=%e, "Connection limiter closed during shutdown"),
            }
        }
        connections.wait_for_tracked_tasks().await;
    };

    if timeout(grace, drain).await.is_err() {
        tracing::warn!(
            grace_secs = grace.as_secs(),
            "Timed out waiting for listener tasks, accepted connections, and tracked transport tasks to drain"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn tracked_task_waits_until_task_finishes() {
        let supervisor = ConnectionSupervisor::new(1);
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();

        supervisor.spawn_tracked("test-task", None, async move {
            let _ = started_tx.send(());
            let _ = release_rx.await;
        });

        started_rx.await.expect("tracked task should start");
        assert!(
            timeout(
                Duration::from_millis(20),
                supervisor.wait_for_tracked_tasks()
            )
            .await
            .is_err(),
            "tracked task drain should wait while task is active"
        );

        let _ = release_tx.send(());
        timeout(Duration::from_secs(1), supervisor.wait_for_tracked_tasks())
            .await
            .expect("tracked task drain should finish");
    }
}
