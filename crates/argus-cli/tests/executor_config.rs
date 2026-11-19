mod e2e_support;

use std::process::Command;

fn startup_error(config_worker: &str, env_worker: Option<&str>) -> String {
    let scratch = e2e_support::Scratch::new("executor-config");
    let config = scratch.path().join("argus.toml");
    std::fs::write(
        &config,
        format!("[execution]\ndefault_worker = '{config_worker}'"),
    )
    .unwrap();
    let missing = scratch.path().join("missing-agent");
    let mut command = Command::new(env!("CARGO_BIN_EXE_argus"));
    command
        .args(["--config", config.to_str().unwrap(), "run"])
        .current_dir(scratch.path())
        .env(
            "ARGUS_DATABASE_URL",
            "postgres://argus_test:argus_test_only_20261013@127.0.0.1:55432/argus_test",
        )
        .env("ARGUS_MASTER_KEY", e2e_support::MASTER_KEY)
        .env("ARGUS_ENV", "development")
        .env("ARGUS_ACTOR_ROLE", "admin")
        .env("ARGUS_AGENT_BINARY_OVERRIDE", &missing)
        .env_remove("ARGUS_RUN_REAL_AGENT_TESTS")
        .env_remove("ARGUS_DEFAULT_WORKER")
        .env_remove("ARGUS_PROVIDERS_CONFIG_PATH");
    if let Some(worker) = env_worker {
        command.env("ARGUS_DEFAULT_WORKER", worker);
    }
    let output = command.output().unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr).to_string();
    if env_worker != Some("invalid") {
        assert!(error.contains(&missing.display().to_string()), "{error}");
    }
    error
}

#[test]
fn test_missing_selected_binary_stops_daemon_startup() {
    assert!(startup_error("codex", None).contains("codex binary"));
    assert!(startup_error("claude_code", None).contains("claude_code binary"));
}

#[test]
fn test_default_worker_environment_overrides_config() {
    assert!(startup_error("claude_code", Some("codex")).contains("codex binary"));
    assert!(startup_error("codex", Some("claude_code")).contains("claude_code binary"));
}

#[test]
fn test_invalid_default_worker_environment_is_rejected() {
    let error = startup_error("codex", Some("invalid"));
    assert!(
        error.contains("ARGUS_DEFAULT_WORKER must be codex or claude_code"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn test_non_utf8_default_worker_environment_is_rejected() {
    use std::os::unix::ffi::OsStringExt;
    let scratch = e2e_support::Scratch::new("invalid-worker");
    let output = Command::new(env!("CARGO_BIN_EXE_argus"))
        .arg("status")
        .current_dir(scratch.path())
        .env(
            "ARGUS_DEFAULT_WORKER",
            std::ffi::OsString::from_vec(vec![0xff]),
        )
        .env(
            "ARGUS_DATABASE_URL",
            "postgres://argus_test:argus_test_only_20261013@127.0.0.1:55432/argus_test",
        )
        .env_remove("ARGUS_RUN_REAL_AGENT_TESTS")
        .env_remove("ARGUS_CONFIG_PATH")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("ARGUS_DEFAULT_WORKER must be codex or claude_code")
    );
}

#[test]
fn test_generated_stub_version_has_no_task_side_effects() {
    let scratch = e2e_support::Scratch::new("stub-version");
    let binary = e2e_support::write_stub_agent(
        scratch.path(),
        "agent.sh",
        "printf task > marker; printf done",
    );
    let output = Command::new(&binary)
        .arg("--version")
        .current_dir(scratch.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"argus-stub 0.0.0\n");
    assert!(!scratch.path().join("marker").exists());
    let task = Command::new(binary)
        .arg("prompt")
        .current_dir(scratch.path())
        .output()
        .unwrap();
    assert!(task.status.success());
    assert_eq!(task.stdout, b"done");
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("marker")).unwrap(),
        "task"
    );
}

#[test]
fn test_smoke_script_has_confirmation_and_valid_shell_syntax() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/smoke_codex.sh");
    assert!(
        Command::new("bash")
            .arg("-n")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains("将消耗 Codex 订阅额度"));
    assert!(text.contains("[[ \"$answer\" == yes ]] || exit 1"));
    assert!(text.find("read -r").unwrap() < text.find("mktemp").unwrap());
}
