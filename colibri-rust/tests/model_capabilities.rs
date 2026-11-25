use colibri_rust::config::{AgentConfig, ModelConfig};
use colibri_rust::model_capabilities::resolve;

fn model(base: &str, name: &str) -> ModelConfig {
    let mut config = AgentConfig::default().model;
    config.provider = "openai_compatible".into();
    config.base_url = base.into();
    config.model = name.into();
    config
}

#[test]
fn official_models_resolve_and_change_without_a_window_setting() {
    let mut config = model("https://api.deepseek.com/v1", "deepseek-flash");
    assert_eq!(resolve(&config).context_window, 1_048_576);
    assert_eq!(resolve(&config).source, "catalog");
    config.base_url = "https://api.openai.com/v1".into();
    config.model = "gpt-4o".into();
    assert_eq!(resolve(&config).context_window, 128_000);
    config.model = "gpt-4.1".into();
    assert_eq!(resolve(&config).context_window, 1_047_576);
    config.model = "gpt-5.5".into();
    assert_eq!(resolve(&config).context_window, 1_050_000);
}

#[test]
fn custom_and_lookalike_origins_have_conservative_defaults() {
    for base in [
        "https://api.deepseek.com.example/v1",
        "http://localhost:8000/v1",
    ] {
        let limits = resolve(&model(base, "deepseek-flash"));
        assert_eq!(limits.context_window, 48_000);
        assert_eq!(limits.source, "fallback");
    }
    assert_eq!(
        resolve(&model("https://api.deepseek.com", "custom")).source,
        "fallback"
    );
}

#[test]
fn custom_overrides_work_and_known_capacity_cannot_be_exceeded() {
    let mut config = model("http://localhost:8000/v1", "local");
    config.input_context_tokens = 8192;
    assert_eq!(resolve(&config).context_window, 8192);
    assert_eq!(resolve(&config).max_output_tokens, 4096);
    config.base_url = "https://api.openai.com/v1".into();
    config.model = "gpt-4o".into();
    config.input_context_tokens = 1_000_000;
    assert_eq!(resolve(&config).context_window, 128_000);
}

#[test]
fn pressure_reserves_actual_output_and_remains_within_window() {
    let mut config = model("https://api.openai.com/v1", "gpt-4o");
    config.max_output_tokens = 100_000;
    let caps = resolve(&config);
    assert_eq!(caps.max_output_tokens, 16_384);
    assert!(caps.compaction_threshold() <= caps.context_window * 4 / 5);
    assert!(caps.compaction_threshold() + caps.max_output_tokens < caps.context_window);
}
