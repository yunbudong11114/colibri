//! Cooperative turn cancellation, independent of transport or UI.
use std::cell::RefCell;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
thread_local! { static CURRENT: RefCell<Option<Cancellation>> = const { RefCell::new(None) }; }
pub(crate) struct Scope(Option<Cancellation>);
impl Scope {
    pub(crate) fn enter(token: Cancellation) -> Self {
        Self(CURRENT.with(|c| c.replace(Some(token))))
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|c| c.replace(self.0.take()));
    }
}
pub(crate) fn cancelled() -> bool {
    CURRENT.with(|c| c.borrow().as_ref().is_some_and(Cancellation::is_cancelled))
}

/// Stop waiting without retrying an already sent request. Its transport timeout remains in force.
pub(crate) fn wait<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let token = CURRENT.with(|c| c.borrow().clone());
    let Some(token) = token else {
        return work();
    };
    if token.is_cancelled() {
        return Err("operation_cancelled".into());
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    static ABANDONED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if ABANDONED.load(Ordering::SeqCst) >= 4 {
        return Err("先前请求仍在结束，请稍后重试".into());
    }
    let request_token = token.clone();
    let state = Arc::new(std::sync::Mutex::new((false, false))); // finished, abandoned
    let worker_state = Arc::clone(&state);
    std::thread::spawn(move || {
        let result = if request_token.is_cancelled() {
            Err("operation_cancelled".into())
        } else {
            work()
        };
        {
            let mut s = worker_state.lock().unwrap_or_else(|e| e.into_inner());
            s.0 = true;
            if s.1 {
                ABANDONED.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _ = tx.send(result);
    });
    loop {
        if token.is_cancelled() {
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            if !s.0 {
                s.1 = true;
                ABANDONED.fetch_add(1, Ordering::SeqCst);
            }
            return Err("operation_cancelled; remote outcome may require verification".into());
        }
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(result) => return result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err("request worker disconnected".into()),
        }
    }
}
