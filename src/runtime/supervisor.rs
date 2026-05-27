use std::time::Duration;

use tokio::task::JoinSet;

use crate::transport::lifecycle::{ConnectionSupervisor, drain_runtime};

pub(super) struct RuntimeSupervisor {
    connections: ConnectionSupervisor,
    listener_tasks: JoinSet<()>,
    max_connections: usize,
}

impl RuntimeSupervisor {
    pub(super) fn new(max_connections: usize) -> Self {
        Self {
            connections: ConnectionSupervisor::new(max_connections),
            listener_tasks: JoinSet::new(),
            max_connections,
        }
    }

    pub(super) fn connections(&self) -> ConnectionSupervisor {
        self.connections.clone()
    }

    pub(super) fn listener_tasks_mut(&mut self) -> &mut JoinSet<()> {
        &mut self.listener_tasks
    }

    pub(super) fn spawn_listener<F>(&mut self, name: &'static str, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.listener_tasks.spawn(async move {
            tracing::debug!(listener = name, "listener task started");
            future.await;
            tracing::debug!(listener = name, "listener task finished");
        });
    }

    pub(super) async fn drain(&mut self, grace: Duration) {
        drain_runtime(
            &mut self.listener_tasks,
            self.connections.clone(),
            self.max_connections,
            grace,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_supervisor_enforces_limit() {
        let supervisor = RuntimeSupervisor::new(1);
        let connections = supervisor.connections();

        let permit = connections
            .try_acquire("test", None)
            .expect("first permit should be available");
        assert!(connections.try_acquire("test", None).is_none());

        drop(permit);
        assert!(connections.try_acquire("test", None).is_some());
    }

    #[tokio::test]
    async fn runtime_supervisor_drains_listener_tasks() {
        let mut supervisor = RuntimeSupervisor::new(1);
        supervisor.spawn_listener("test-listener", async {});

        supervisor.drain(Duration::from_secs(1)).await;
        assert_eq!(supervisor.listener_tasks.len(), 0);
    }
}
