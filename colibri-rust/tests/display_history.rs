use colibri_rust::display_history::DisplayHistory;
use serde_json::json;
use std::fs;

#[test]
fn pages_reach_both_ends_across_files_and_clear_is_a_display_boundary() {
    let home = std::env::temp_dir().join(format!("colibri-display-pages-{}", std::process::id()));
    fs::create_dir_all(home.join("transcripts")).unwrap();
    for day in 1..=2 {
        let mut log = String::new();
        for n in 0..10 {
            let number = (day - 1) * 10 + n;
            for (kind, text) in [
                ("user_message", format!("question {number}")),
                ("assistant_message", format!("answer {number}")),
            ] {
                log += &(json!({"ts":format!("2026-11-{day:02}T12:{n:02}:00"),"type":kind,"payload":{"text":text,"model_stream":"1-100-0","tool_call_count":0}}).to_string()+"\n");
            }
        }
        // Tool payload larger than the previous entire scan window must not hide conversation text.
        log += &(json!({"type":"tool_result","payload":{"text":"x".repeat(2*1024*1024)}})
            .to_string()
            + "\n");
        for kind in ["user_message", "assistant_message"] {
            log += &(json!({"type":kind,"payload":{"text":"private","channel":"weixin","sender_id":"other","tool_call_count":0}}).to_string()+"\n");
        }
        fs::write(
            home.join(format!("transcripts/2026-11-{day:02}.jsonl")),
            log,
        )
        .unwrap();
    }
    let loader = DisplayHistory::new(home.clone(), "local");
    let last = loader.page("last", 0).unwrap();
    assert_eq!(last["total"], 40);
    assert_eq!(last["start"], 32);
    assert_eq!(last["history"][7]["text"], "answer 19");
    let remote = DisplayHistory::new(home.clone(), "weixin:other")
        .page("last", 0)
        .unwrap();
    assert_eq!(remote["total"], 4);
    assert_eq!(remote["history"][0]["text"], "private");
    let first = loader.page("first", 32).unwrap();
    assert_eq!(first["history"][0]["text"], "question 0");
    let next = loader.page("after", 0).unwrap();
    assert_eq!(next["start"], 4);
    assert_eq!(first["history"][4]["id"], next["history"][0]["id"]);
    assert_eq!(loader.page("before", 4).unwrap()["start"], 0);
    let old = home.join("transcripts/2026-11-01.jsonl");
    fs::write(
        home.join("chat-display-cursor.json"),
        json!({"2026-11-01.jsonl":fs::metadata(&old).unwrap().len()}).to_string(),
    )
    .unwrap();
    let first = loader.page("first", 0).unwrap();
    assert_eq!(first["total"], 20);
    assert_eq!(first["history"][0]["text"], "question 10");
    // An answer arriving after clear must remain visible even if its question is hidden.
    let current = home.join("transcripts/2026-11-02.jsonl");
    let len = fs::metadata(&current).unwrap().len();
    fs::write(
        home.join("chat-display-cursor.json"),
        json!({"2026-11-01.jsonl":fs::metadata(&old).unwrap().len(),"2026-11-02.jsonl":len})
            .to_string(),
    )
    .unwrap();
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(&current).unwrap();
    writeln!(file,"{}",json!({"ts":"2026-11-26T00:00:00","type":"assistant_message","payload":{"text":"late answer","tool_call_count":0}})).unwrap();
    writeln!(file,"{}",json!({"ts":"2026-11-26T00:00:01","type":"user_message","payload":{"text":"pending question"}})).unwrap();
    let page = loader.page("last", 0).unwrap();
    assert_eq!(page["total"], 2);
    assert_eq!(page["history"][0]["text"], "late answer");
    assert_eq!(page["history"][1]["text"], "pending question");
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn stale_cached_offsets_are_rebuilt_without_hiding_messages() {
    let home = std::env::temp_dir().join(format!(
        "colibri-display-stale-cache-{}",
        std::process::id()
    ));
    fs::create_dir_all(home.join("transcripts")).unwrap();
    let path = home.join("transcripts/2026-11-27.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n",
            json!({"type":"user_message","payload":{"text":"visible"}})
        ),
    )
    .unwrap();
    let loader = DisplayHistory::new(home.clone(), "local");
    assert_eq!(loader.page("last", 0).unwrap()["total"], 1);
    let cache = fs::read_dir(home.join("cache/display-history"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut data: serde_json::Value = serde_json::from_slice(&fs::read(&cache).unwrap()).unwrap();
    data["files"][0]["entries"][0]["offset"] = json!(1);
    fs::write(&cache, data.to_string()).unwrap();
    // A different source replaces the in-process slot, forcing the next local read to use disk.
    DisplayHistory::new(home.clone(), "weixin:other")
        .page("last", 0)
        .unwrap();
    assert_eq!(
        loader.page("last", 0).unwrap()["history"][0]["text"],
        "visible"
    );
    fs::remove_dir_all(home).unwrap();
}
