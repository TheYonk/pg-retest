use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Handle for a background task (proxy, replay, pipeline).
pub struct TaskHandle {
    pub id: String,
    pub task_type: String,
    pub label: String,
    pub cancel_token: CancellationToken,
    pub join_handle: JoinHandle<()>,
}

/// Summary of a running task (serializable for API responses).
#[derive(Debug, Clone, Serialize)]
pub struct TaskInfo {
    pub id: String,
    pub task_type: String,
    pub label: String,
    pub running: bool,
}

/// Manages background tasks with spawn/cancel/status operations.
#[derive(Default)]
pub struct TaskManager {
    tasks: RwLock<HashMap<String, TaskHandle>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }

    /// Spawn a background task. Returns the task ID.
    pub async fn spawn<F>(self: &Arc<Self>, task_type: &str, label: &str, f: F) -> String
    where
        F: FnOnce(CancellationToken, String) -> tokio::task::JoinHandle<()>,
    {
        let id = uuid::Uuid::new_v4().to_string();
        let cancel_token = CancellationToken::new();
        let join_handle = f(cancel_token.clone(), id.clone());

        let handle = TaskHandle {
            id: id.clone(),
            task_type: task_type.to_string(),
            label: label.to_string(),
            cancel_token,
            join_handle,
        };

        self.tasks.write().await.insert(id.clone(), handle);
        id
    }

    /// Cancel a running task by ID.
    pub async fn cancel(&self, id: &str) -> bool {
        let mut tasks = self.tasks.write().await;
        if let Some(handle) = tasks.remove(id) {
            handle.cancel_token.cancel();
            handle.join_handle.abort();
            true
        } else {
            false
        }
    }

    /// List all active tasks.
    pub async fn list(&self) -> Vec<TaskInfo> {
        let tasks = self.tasks.read().await;
        tasks
            .values()
            .map(|h| TaskInfo {
                id: h.id.clone(),
                task_type: h.task_type.clone(),
                label: h.label.clone(),
                running: !h.join_handle.is_finished(),
            })
            .collect()
    }

    /// Remove finished tasks from the map.
    pub async fn cleanup(&self) {
        let mut tasks = self.tasks.write().await;
        tasks.retain(|_, h| !h.join_handle.is_finished());
    }

    /// Check if a task of a given type is currently running.
    pub async fn has_running(&self, task_type: &str) -> bool {
        let tasks = self.tasks.read().await;
        tasks
            .values()
            .any(|h| h.task_type == task_type && !h.join_handle.is_finished())
    }

    /// Cancel all running tasks (used during graceful shutdown).
    pub async fn cancel_all(&self) {
        let mut tasks = self.tasks.write().await;
        for (_id, handle) in tasks.drain() {
            handle.cancel_token.cancel();
            handle.join_handle.abort();
        }
    }

    /// Get a specific task's info.
    pub async fn get(&self, id: &str) -> Option<TaskInfo> {
        let tasks = self.tasks.read().await;
        tasks.get(id).map(|h| TaskInfo {
            id: h.id.clone(),
            task_type: h.task_type.clone(),
            label: h.label.clone(),
            running: !h.join_handle.is_finished(),
        })
    }
}
