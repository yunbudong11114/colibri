use colibri_rust::config::AgentConfig;
use colibri_rust::memory::{initialize, MemoryContext};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "colibri-memory-init-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn config(&self) -> AgentConfig {
        let mut config = AgentConfig::default();
        config.memory.root = self.0.join("memory");
        config
    }
    fn cli_config(&self, model: &str) -> PathBuf {
        let path = self.0.join("config.toml");
        fs::write(
            &path,
            format!(
                "[model]\nprovider='{model}'\n[session]\ntranscript=false\n[memory]\nroot='{}'\n",
                self.0.join("memory").display()
            ),
        )
        .unwrap();
        path
    }
    fn command(&self, config: &std::path::Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_colibri"));
        command
            .env("COLIBRI_HOME", self.0.join("state"))
            .arg("--config")
            .arg(config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn fills_missing_files_without_rewriting_user_or_empty_files() {
    let fixture = Fixture::new();
    let config = fixture.config();
    let root = &config.memory.root;
    fs::create_dir(root).unwrap();
    fs::write(root.join("USER.md"), "my own preferences").unwrap();
    fs::write(root.join("SOUL.md"), "").unwrap();
    initialize(&config).unwrap();
    let names = [
        "SOUL.md",
        "USER.md",
        "MEMORY.md",
        "INDEX.md",
        "topics/sample.md",
    ];
    let before: Vec<_> = names
        .iter()
        .map(|n| {
            (
                fs::read(root.join(n)).unwrap(),
                fs::metadata(root.join(n)).unwrap().modified().unwrap(),
            )
        })
        .collect();
    initialize(&config).unwrap();
    for (name, (content, modified)) in names.iter().zip(before) {
        assert_eq!(fs::read(root.join(name)).unwrap(), content);
        assert_eq!(
            fs::metadata(root.join(name)).unwrap().modified().unwrap(),
            modified
        );
    }
    assert_eq!(
        fs::read_to_string(root.join("USER.md")).unwrap(),
        "my own preferences"
    );
    assert!(fs::read(root.join("SOUL.md")).unwrap().is_empty());
}

#[test]
fn disabled_memory_creates_nothing() {
    let fixture = Fixture::new();
    let mut config = fixture.config();
    config.memory.enabled = false;
    initialize(&config).unwrap();
    assert!(!config.memory.root.exists());
}

#[test]
fn initialization_errors_are_visible_to_memory_loaders() {
    let fixture = Fixture::new();
    let config = fixture.config();
    fs::write(&config.memory.root, "not a directory").unwrap();
    let error = MemoryContext::new(config.clone()).load().unwrap_err();
    assert!(error.contains("Memory initialization failed"));
    assert!(error.contains(config.memory.root.to_str().unwrap()));
    assert_eq!(
        fs::read_to_string(config.memory.root).unwrap(),
        "not a directory"
    );
}

#[test]
fn existing_non_file_is_not_replaced() {
    let fixture = Fixture::new();
    let config = fixture.config();
    fs::create_dir_all(config.memory.root.join("SOUL.md")).unwrap();
    assert!(initialize(&config).unwrap_err().contains("SOUL.md"));
    assert!(config.memory.root.join("SOUL.md").is_dir());
}

#[test]
fn concurrent_processes_initialize_before_any_repl_message() {
    let fixture = Fixture::new();
    let config_path = fixture.cli_config("fake");
    let root = fixture.0.join("memory");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("USER.md"), "keep concurrent user data").unwrap();
    let processes: Vec<_> = (0..8)
        .map(|_| fixture.command(&config_path).arg("repl").spawn().unwrap())
        .collect();
    for child in processes {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        fs::read_to_string(root.join("USER.md")).unwrap(),
        "keep concurrent user data"
    );
    for name in ["SOUL.md", "MEMORY.md", "INDEX.md", "topics/sample.md"] {
        assert!(!fs::read(root.join(name)).unwrap().is_empty());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(root.join("SOUL.md"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(!fs::read_dir(root).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".memory-init-")));
}

#[test]
fn query_commands_do_not_initialize_memory() {
    let fixture = Fixture::new();
    let path = fixture.cli_config("fake");
    for args in [
        vec!["version"],
        vec!["--help"],
        vec!["diagnostics"],
        vec!["gateway", "status"],
    ] {
        assert!(fixture
            .command(&path)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
        assert!(!fixture.0.join("memory").exists());
    }
}

#[test]
fn runtime_start_initializes_without_model_or_channel_credentials() {
    for args in [vec!["repl"], vec!["gateway", "run"]] {
        let fixture = Fixture::new();
        let path = fixture.cli_config("openai_compatible");
        let output = fixture.command(&path).args(&args).output().unwrap();
        if args[0] == "gateway" {
            assert!(!output.status.success());
        }
        for name in [
            "SOUL.md",
            "USER.md",
            "MEMORY.md",
            "INDEX.md",
            "topics/sample.md",
        ] {
            assert!(fixture.0.join("memory").join(name).is_file());
        }
    }
}
