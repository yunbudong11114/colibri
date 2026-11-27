use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const SKIPPED_TOOL_RESULT: &str = "Skipped due to queued user message.";
pub const STEERING_QUEUE_MAX: usize = 4;
pub const STEERING_PREVIEW_CHARS: usize = 20;

pub fn format_received(text: &str) -> &'static str {
    if text.trim() == "/stop" {
        "正在停止，整理阶段性结论…"
    } else {
        "已收到补充，等待处理"
    }
}

pub fn format_steering_ack(skipped: usize, steering_text: &str) -> String {
    let line1 = format!("已改方向，跳过剩余 {skipped} 个工具");
    let stripped = steering_text.trim();
    if stripped.is_empty() {
        return line1;
    }
    let preview: String = stripped.chars().take(STEERING_PREVIEW_CHARS).collect();
    let preview = if stripped.chars().count() > STEERING_PREVIEW_CHARS {
        format!("{preview}…")
    } else {
        preview
    };
    format!("{line1}\n改：{preview}")
}

pub struct SteeringState {
    queue: Mutex<VecDeque<String>>,
    turn_active: AtomicBool,
    cancellation: Mutex<crate::cancellation::Cancellation>,
}

impl SteeringState {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            turn_active: AtomicBool::new(false),
            cancellation: Default::default(),
        }
    }

    pub fn steer(&self, text: &str) -> bool {
        let cleaned = text.trim();
        if cleaned == "/stop" {
            return self.stop();
        }
        if cleaned.is_empty() || !self.is_turn_active() {
            return false;
        }
        let Ok(mut queue) = self.queue.lock() else {
            return false;
        };
        if queue.len() >= STEERING_QUEUE_MAX {
            return false;
        }
        queue.push_back(cleaned.to_string());
        true
    }

    pub fn stop(&self) -> bool {
        let token = self.cancellation.lock().unwrap_or_else(|e| e.into_inner());
        let active = self.is_turn_active();
        token.cancel();
        active
    }

    pub(crate) fn begin_turn(&self) -> crate::cancellation::Cancellation {
        let token = self.cancellation.lock().unwrap_or_else(|e| e.into_inner());
        self.set_turn_active(true);
        token.clone()
    }
    pub(crate) fn end_turn(&self) {
        let mut token = self.cancellation.lock().unwrap_or_else(|e| e.into_inner());
        self.set_turn_active(false);
        *token = Default::default();
        self.clear();
    }
    pub fn is_turn_active(&self) -> bool {
        self.turn_active.load(Ordering::SeqCst)
    }

    pub fn set_turn_active(&self, active: bool) {
        self.turn_active.store(active, Ordering::SeqCst);
    }

    pub fn drain_one(&self) -> Option<String> {
        self.queue.lock().ok()?.pop_front()
    }

    pub fn clear(&self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
    }
}

impl Default for SteeringState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct SteerHandle {
    inner: Arc<SteeringState>,
}

impl SteerHandle {
    pub fn new(inner: Arc<SteeringState>) -> Self {
        Self { inner }
    }

    pub fn steer(&self, text: &str) -> bool {
        self.inner.steer(text)
    }

    pub fn stop(&self) -> bool {
        self.inner.stop()
    }

    pub fn is_turn_active(&self) -> bool {
        self.inner.is_turn_active()
    }

    pub fn set_turn_active_for_test(&self, active: bool) {
        self.inner.set_turn_active(active);
    }

    pub fn drain_one_for_test(&self) -> Option<String> {
        self.inner.drain_one()
    }
}
