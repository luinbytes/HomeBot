use crate::ToolActivity;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

#[async_trait]
pub trait ActivitySink: Send + Sync {
    async fn emit(&self, activity: ToolActivity);
}

#[derive(Debug, Default)]
pub struct NoopActivitySink;

#[async_trait]
impl ActivitySink for NoopActivitySink {
    async fn emit(&self, _activity: ToolActivity) {}
}

#[derive(Clone, Debug, Default)]
pub struct RecordingActivitySink {
    activities: Arc<Mutex<Vec<ToolActivity>>>,
}

impl RecordingActivitySink {
    #[must_use]
    pub async fn snapshot(&self) -> Vec<ToolActivity> {
        self.activities.lock().await.clone()
    }
}

#[async_trait]
impl ActivitySink for RecordingActivitySink {
    async fn emit(&self, activity: ToolActivity) {
        self.activities.lock().await.push(activity);
    }
}
