use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const SKIPPED_TOOL_RESULT: &str = "Skipped due to queued user message.";
pub const STEERING_QUEUE_MAX: usize = 4;
pub const STEERING_PREVIEW_CHARS: usize = 20;

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
    permission_pending: AtomicBool,
}

impl SteeringState {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            turn_active: AtomicBool::new(false),
            permission_pending: AtomicBool::new(false),
        }
    }

    pub fn steer(&self, text: &str) -> bool {
        let cleaned = text.trim();
        if cleaned.is_empty() || !self.is_turn_active() || self.is_permission_pending() {
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

    pub fn is_turn_active(&self) -> bool {
        self.turn_active.load(Ordering::SeqCst)
    }

    pub fn is_permission_pending(&self) -> bool {
        self.permission_pending.load(Ordering::SeqCst)
    }

    pub fn set_turn_active(&self, active: bool) {
        self.turn_active.store(active, Ordering::SeqCst);
    }

    pub fn set_permission_pending(&self, pending: bool) {
        self.permission_pending.store(pending, Ordering::SeqCst);
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

    pub fn is_turn_active(&self) -> bool {
        self.inner.is_turn_active()
    }

    pub fn is_permission_pending(&self) -> bool {
        self.inner.is_permission_pending()
    }

    pub fn set_turn_active_for_test(&self, active: bool) {
        self.inner.set_turn_active(active);
    }

    pub fn set_permission_pending_for_test(&self, pending: bool) {
        self.inner.set_permission_pending(pending);
    }

    pub fn drain_one_for_test(&self) -> Option<String> {
        self.inner.drain_one()
    }
}
