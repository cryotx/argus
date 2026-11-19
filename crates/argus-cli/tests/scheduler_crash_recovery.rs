use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires postgres and sends SIGKILL to a probe process"]
fn scheduler_resumes_running_task_after_sigkill_without_duplicate_execution() {
    let binary = env!("CARGO_BIN_EXE_argus");
    let database_url = std::env::var("ARGUS_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .expect("set ARGUS_DATABASE_URL or DATABASE_URL");

    let mut child = Command::new(binary)
        .args(["scheduler-crash-probe", "--phase", "prepare"])
        .env("ARGUS_DATABASE_URL", &database_url)
        .env("ARGUS_MASTER_KEY", "0123456789abcdef0123456789abcdef")
        .env("ARGUS_ACTOR_ROLE", "admin")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn crash probe");

    let stdout = child.stdout.take().expect("probe stdout");
    let mut reader = BufReader::new(stdout);
    let deadline = Instant::now() + Duration::from_secs(30);
    let marker = loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for crash probe"
        );
        let mut line = String::new();
        reader.read_line(&mut line).expect("read probe output");
        if let Some(json) = line.strip_prefix("ARGUS_CRASH_PROBE ") {
            break serde_json::from_str::<serde_json::Value>(json.trim())
                .expect("parse crash marker");
        }
    };

    child.kill().expect("SIGKILL crash probe");
    child.wait().expect("wait for killed probe");

    let task_id = marker["task_id"].as_str().expect("task id");
    let original_execution_id = marker["execution_id"].as_str().expect("execution id");
    let output = Command::new(binary)
        .args([
            "scheduler-crash-probe",
            "--phase",
            "resume",
            "--task-id",
            task_id,
        ])
        .env("ARGUS_DATABASE_URL", &database_url)
        .env("ARGUS_MASTER_KEY", "0123456789abcdef0123456789abcdef")
        .env("ARGUS_ACTOR_ROLE", "admin")
        .output()
        .expect("run resume probe");

    assert!(
        output.status.success(),
        "resume failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find_map(|line| line.strip_prefix("ARGUS_RESUME_PROBE "))
        .expect("resume marker");
    let resumed: serde_json::Value = serde_json::from_str(line).expect("parse resume marker");
    assert_eq!(resumed["execution_id"], original_execution_id);
    assert_eq!(resumed["logical_execution_count"], 1);
    assert_eq!(resumed["state"], "completed");
}
