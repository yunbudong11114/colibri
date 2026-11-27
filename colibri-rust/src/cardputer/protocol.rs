//! Responsive device protocol; execution semantics stay in AgentSession.
use super::*;

pub(super) fn pipe(home: PathBuf) -> Result<(), String> {
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    };
    let notice: Notice = Arc::new(|text| {
        let _ = send(&event("status", json!({"text":text})));
    });
    let history_home = home.clone();
    let mut history_reader: Option<std::thread::JoinHandle<()>> = None;
    let mut app = App::new(home.clone(), Arc::clone(&notice));
    if let Err(error) = app
        .settings
        .config()
        .and_then(|c| crate::memory::initialize(&c))
    {
        send(&event("error", json!({"text":error,"startup":true}))).map_err(|e| e.to_string())?;
    }
    let slot = Arc::new(Mutex::new(None::<crate::steering::SteerHandle>));
    let epoch = Arc::new(AtomicU64::new(0));
    let stop_pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let actor_stop = Arc::clone(&stop_pending);
    let (tx, rx) = mpsc::sync_channel::<(u64, String)>(4);
    let actor_slot = Arc::clone(&slot);
    let actor_epoch = Arc::clone(&epoch);
    let actor = std::thread::spawn(move || {
        let mut agent = App::new(home, notice);
        agent.control = Some(actor_slot);
        agent.activity = Some(Arc::new(|stage| {
            let text = crate::session::activity::label(stage);
            let _ = send(&event("activity", json!({"text":text})));
        }));
        loop {
            if actor_stop.swap(false, Ordering::SeqCst) {
                let value = match agent.ask("/stop") {
                    Ok(text) => event("reply", json!({"text":text})),
                    Err(error) => event("error", json!({"text":error,"agent":true})),
                };
                let _ = send(&value);
            }
            let (generation, text) = match rx.recv_timeout(std::time::Duration::from_millis(30)) {
                Ok(value) => value,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            };
            if generation != actor_epoch.load(Ordering::SeqCst) && text != "/stop" {
                continue;
            }
            agent.execution_epoch = Some((Arc::clone(&actor_epoch), generation));
            let reply = match agent.ask(&text) {
                Ok(text) => event("reply", json!({"text":text})),
                Err(error) => event(
                    "error",
                    json!({"text":app_model_guidance(error),"agent":true}),
                ),
            };
            let _ = send(&reply);
        }
        agent
    });
    let mut transcription = None;
    let mut transcription_cancel: Option<crate::cancellation::Cancellation> = None;
    let mut shutdown = false;
    for line in io::stdin().lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        let request = match serde_json::from_str::<Value>(&line) {
            Ok(v) => v,
            Err(_) => {
                let _ = send(&event("error", json!({"text":"请求格式错误"})));
                continue;
            }
        };
        let kind = request["type"].as_str().unwrap_or("");
        if kind == "history_page" {
            let id = request["request_id"].as_u64().unwrap_or(0);
            if history_reader.as_ref().is_some_and(|t| !t.is_finished()) {
                let _ = send(&event(
                    "history_page",
                    json!({"request_id":id,"error":"历史正在加载，请稍后重试"}),
                ));
                continue;
            }
            if let Some(old) = history_reader.take() {
                let _ = old.join();
            }
            let home = history_home.clone();
            let direction = request["direction"].as_str().unwrap_or("last").to_owned();
            let start = request["start"].as_u64().unwrap_or(0) as usize;
            history_reader = Some(std::thread::spawn(move || {
                let app = App::new(home, Arc::new(|_| {}));
                let result = app.history_page(&direction, start);
                let mut payload = result.unwrap_or_else(|e| json!({"error":e}));
                payload["request_id"] = json!(id);
                let _ = send(&event("history_page", payload));
            }));
            continue;
        }
        if kind == "submit" || kind == "stop" {
            let text = if kind == "stop" {
                "/stop"
            } else {
                request["text"].as_str().unwrap_or("")
            };
            let handle = slot.lock().map_err(|_| "control lock poisoned")?.clone();
            if text.trim() == "/stop" {
                epoch.fetch_add(1, Ordering::SeqCst);
                if handle.as_ref().is_some_and(|h| h.stop()) {
                    let _ = send(&event(
                        "steering_received",
                        json!({"text":crate::steering::format_received("/stop")}),
                    ));
                    continue;
                }
                stop_pending.store(true, Ordering::SeqCst);
                let _ = send(&event(
                    "steering_received",
                    json!({"text":crate::steering::format_received("/stop")}),
                ));
                continue;
            } else if handle.as_ref().is_some_and(|h| h.steer(text)) {
                let _ = send(&event(
                    "steering_received",
                    json!({"text":crate::steering::format_received(text)}),
                ));
                continue;
            }
            match tx.try_send((epoch.load(Ordering::SeqCst), text.to_string())) {
                Ok(()) => {}
                Err(_) => {
                    let _ = send(&event(
                        "error",
                        json!({"text":"待处理消息已满，请稍后发送"}),
                    ));
                }
            }
            continue;
        }
        if kind == "shutdown" {
            shutdown = true;
            if let Some(token) = &transcription_cancel {
                token.cancel();
            }
            epoch.fetch_add(1, Ordering::SeqCst);
            if let Some(h) = slot.lock().map_err(|_| "control lock poisoned")?.as_ref() {
                h.stop();
            }
            break;
        }
        if kind == "quit" {
            break;
        }
        if kind == "voice_stop" {
            if transcription
                .as_ref()
                .is_some_and(|t: &std::thread::JoinHandle<()>| !t.is_finished())
            {
                let _ = send(&event("error", json!({"text":"正在转写上一段录音"})));
                continue;
            }
            if let Some(old) = transcription.take() {
                let _ = old.join();
            }
            let recording = app.recording.take();
            let cancel = crate::cancellation::Cancellation::default();
            transcription_cancel = Some(cancel.clone());
            transcription = Some(std::thread::spawn(move || {
                let _scope = crate::cancellation::Scope::enter(cancel);
                let result = recording.map_or(Ok(String::new()), |r| r.finish(false));
                let value = match result {
                    Ok(text) if !text.is_empty() => event("draft", json!({"text":text})),
                    Ok(_) => event(
                        "error",
                        json!({"text":"未识别到语音，请靠近麦克风重试。","voice":true}),
                    ),
                    Err(error) => event("error", json!({"text":error,"voice":true})),
                };
                let _ = send(&value);
            }));
            continue;
        }
        match handle(&mut app, request) {
            Ok(Some(value)) => {
                let _ = send(&value);
            }
            Ok(None) => break,
            Err(error) => {
                let _ = send(&event("error", json!({"text":app_model_guidance(error)})));
            }
        }
    }
    drop(tx);
    let mut agent = actor.join().map_err(|_| "Agent worker panicked")?;
    if shutdown {
        if let Some(session) = &mut agent.session {
            if session.has_pending_work() {
                let text = session.submit("/stop")?.text;
                let _ = send(&event("reply", json!({"text":text})));
            }
        }
        app.finish_voice(true).ok();
    }
    if let Some(t) = transcription {
        let _ = t.join();
    }
    if let Some(reader) = history_reader {
        let _ = reader.join();
    }
    if shutdown {
        let _ = send(&event("shutdown", json!({})));
    }
    Ok(())
}
