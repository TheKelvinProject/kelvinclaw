use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use tokio::sync::Mutex;

#[derive(Debug)]
pub struct LaneScheduler {
    use_global_lane: bool,
    global_lane: Arc<Mutex<()>>,
    session_lanes: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl Default for LaneScheduler {
    fn default() -> Self {
        Self::new(true)
    }
}

impl LaneScheduler {
    pub fn new(use_global_lane: bool) -> Self {
        Self {
            use_global_lane,
            global_lane: Arc::new(Mutex::new(())),
            session_lanes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn get_session_lane(&self, lane_key: &str) -> Arc<Mutex<()>> {
        let mut lanes = self.session_lanes.lock().await;
        lanes
            .entry(lane_key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn run_in_lane<T, F>(&self, lane_key: &str, task: F) -> T
    where
        F: Future<Output = T>,
    {
        let session_lane = self.get_session_lane(lane_key).await;
        let _session_guard = session_lane.lock().await;

        if self.use_global_lane {
            let _global_guard = self.global_lane.lock().await;
            task.await
        } else {
            task.await
        }
    }
}
