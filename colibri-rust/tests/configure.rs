#![cfg(unix)]
use colibri_rust::config::AgentConfig;
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "colibri-configure-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn path(&self) -> PathBuf {
        self.0.join("config.toml")
    }
    fn run(&self, args: &[&str], input: &str, worker: bool) -> std::process::Output {
        let mut c = Command::new(if worker {
            env!("CARGO_BIN_EXE_colibri-worker")
        } else {
            env!("CARGO_BIN_EXE_colibri")
        });
        c.arg("--config")
            .arg(self.path())
            .args(args)
            .env("COLIBRI_HOME", self.0.join("state"))
            .env_remove("COLIBRI_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn missing_user_model_never_defaults_to_fake() {
    let f = Fixture::new();
    fs::write(f.path(), "[memory]\nenabled=false\n").unwrap();
    let cfg = AgentConfig::load(Some(&f.path())).unwrap();
    assert_eq!(cfg.model.provider, "openai_compatible");
    assert!(cfg.model.model.is_empty());
    for args in [
        vec!["repl"],
        vec!["gateway", "start"],
        vec!["gateway", "run"],
    ] {
        let o = f.run(&args, "", false);
        assert!(!o.status.success());
        let text = String::from_utf8(o.stderr).unwrap();
        assert!(text.contains("未配置 AI 模型"), "{text}");
        assert!(text.contains("configure") && text.contains(&*f.path().to_string_lossy()));
        assert!(!f.0.join("state/run/gateway.json").exists());
    }
}
#[test]
fn wizard_saves_private_config_without_disclosing_key() {
    let f = Fixture::new();
    let o = f.run(
        &["configure"],
        "1\nhttps://api.deepseek.com\nfixture-model\nfixture-private-api-key\ny\n",
        false,
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(!String::from_utf8_lossy(&o.stdout).contains("fixture-private-api-key"));
    let cfg = AgentConfig::load(Some(&f.path())).unwrap();
    assert_eq!(cfg.model.model, "fixture-model");
    assert_eq!(cfg.model.api_key, "fixture-private-api-key");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(f.path()).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
#[test]
fn cancelled_wizard_and_worker_entry_create_no_config() {
    for worker in [false, true] {
        for input in [":cancel\n", "2\n\nlocal-model\n\nn\n", ""] {
            let f = Fixture::new();
            let o = f.run(&["configure"], input, worker);
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            assert!(!f.path().exists());
            assert!(!f.0.join("state/config.toml").exists());
        }
    }
}
#[test]
fn wizard_preserves_other_sections_and_existing_secret() {
    let f = Fixture::new();
    fs::write(f.path(),"# keep comment\n[model]\nprovider='openai_compatible'\nbase_url='https://api.deepseek.com'\nmodel='existing-model'\napi_key='existing-private-key'\nmax_output_tokens=2048\n[memory]\nenabled=false\n[channels.weixin]\ntoken='fixture-channel-token'\nenabled=false\n").unwrap();
    let o = f.run(&["configure"], "1\n\n\n\ny\n", false);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = String::from_utf8(o.stdout).unwrap();
    assert!(!text.contains("existing-private-key") && !text.contains("fixture-channel-token"));
    let stored = fs::read_to_string(f.path()).unwrap();
    assert!(stored.contains("# keep comment") && stored.contains("fixture-channel-token"));
    let cfg = AgentConfig::load(Some(&f.path())).unwrap();
    assert_eq!(cfg.model.api_key, "existing-private-key");
    assert_eq!(cfg.model.max_output_tokens, 2048);
    assert!(!cfg.memory.enabled);
}
#[test]
fn switching_endpoint_does_not_carry_old_key() {
    let f = Fixture::new();
    fs::write(f.path(),"[model]\nprovider='openai_compatible'\nbase_url='https://api.deepseek.com'\nmodel='existing-model'\napi_key='old-private-key'\n").unwrap();
    let o = f.run(&["configure"], "2\n\nlocal-model\n\ny\n", false);
    assert!(o.status.success());
    let cfg = AgentConfig::load(Some(&f.path())).unwrap();
    assert_eq!(cfg.model.base_url, "http://127.0.0.1:11434/v1");
    assert_eq!(cfg.model.api_key, "");
}
#[test]
fn invalid_values_reprompt_and_explicit_fake_stays_supported() {
    let f = Fixture::new();
    let o = f.run(
        &["configure"],
        "wrong\n2\nbad url\nhttp://127.0.0.1:9000/v1\n\nlocal-model\n\ny\n",
        false,
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains("模型 ID 不能为空"));
    fs::write(f.path(),"[model]\nprovider='fake'\n[memory]\nenabled=false\n[session]\ntranscript=false\nrestore_transcript=false\n").unwrap();
    let o = f.run(&["ask", "hello"], "", false);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("fake: hello"));
}

#[test]
fn fresh_default_entries_report_unconfigured_model() {
    for args in [
        vec!["repl"],
        vec!["gateway", "start"],
        vec!["gateway", "run"],
    ] {
        let f = Fixture::new();
        let result = Command::new(env!("CARGO_BIN_EXE_colibri"))
            .args(args)
            .env("COLIBRI_HOME", f.0.join("state"))
            .env_remove("COLIBRI_API_KEY")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!result.status.success());
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(
            error.contains("未配置 AI 模型") && error.contains("configure"),
            "{error}"
        );
    }
}

#[test]
fn device_terminal_repl_uses_cli_guidance_before_prompt() {
    let f = Fixture::new();
    let result = Command::new(env!("CARGO_BIN_EXE_colibri-worker"))
        .arg("--home")
        .arg(&f.0)
        .arg("--repl")
        .env_remove("COLIBRI_API_KEY")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!result.status.success());
    let error = String::from_utf8(result.stderr).unwrap();
    assert!(
        error.contains("未配置 AI 模型") && error.contains("configure"),
        "{error}"
    );
    assert!(!error.contains("按 Enter"));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("Colibri REPL"));
}

#[test]
fn explicit_missing_config_path_also_gets_setup_guidance() {
    for args in [
        vec!["repl"],
        vec!["gateway", "start"],
        vec!["gateway", "run"],
    ] {
        let f = Fixture::new();
        let output = f.run(&args, "", false);
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(
            error.contains("未配置 AI 模型") && error.contains("configure"),
            "{error}"
        );
        assert!(!f.path().exists());
    }
}
