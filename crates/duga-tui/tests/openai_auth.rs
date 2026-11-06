//! Configuration and runtime smoke checks; never contacts OpenAI or opens a browser.
use duga_config::Config;
use std::{path::PathBuf, sync::Arc};

fn example_config() -> (tempfile::TempDir, Config) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../duga-openai.yaml");
    let directory = tempfile::tempdir().unwrap();
    // Config validation requires an existing workspace. Keep tests isolated from
    // the user's actual data directory, regardless of cargo's working directory.
    let root = directory.path().to_string_lossy().replace('\\', "/");
    let yaml = std::fs::read_to_string(path)
        .unwrap()
        .replace("./data/openai-workspace", &root);
    let copy = directory.path().join("test.yaml");
    std::fs::write(&copy, yaml).unwrap();
    let config = Config::load(copy).expect("OpenAI OAuth example must be a valid complete config");
    (directory, config)
}
#[test]
fn example_selects_openai_oauth_with_a_safe_workspace() {
    let (_directory, config) = example_config();
    let selection = duga_runtime::providers::resolve_provider(&config).unwrap();
    assert_eq!(selection.provider, "openai");
    assert!(!selection.model.is_empty());
    assert_eq!(selection.model, config.model);
    assert_eq!(config.provider_auth.as_deref(), Some("oauth"));
    assert_eq!(config.sandbox.mode, duga_config::SandboxMode::Capability);
    assert!(config.environment.allowed.is_empty());
}
#[tokio::test]
async fn runtime_provider_errors_are_not_replaced_by_a_dummy_model() {
    let (_directory, mut config) = example_config();
    config.provider = Some("nonexistent-provider".into());
    config.provider_auth = None;
    let (tx, _bridge) = duga_runtime::FrontendEventBridge::new(16);
    let sink = Arc::new(duga_runtime::FrontendEventSink::new(tx));
    let error = duga_tui::runtime::build_runtime(&config, sink, None, None)
        .await
        .err()
        .expect("bad provider must return an actionable error, not a dummy client");
    assert!(format!("{error:#}").contains("unsupported LLM provider"));
}
