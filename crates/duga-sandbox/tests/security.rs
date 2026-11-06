use duga_sandbox::{CancellationToken, Workspace, run_captured};
use duga_types::{config::OutputLimits, error::ToolError};
use std::{
    collections::HashMap,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

#[test]
fn allowed_files_reject_traversal_in_every_namespace() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let aux = outside.path().join("nested/aux");
    std::fs::create_dir_all(&aux).unwrap();
    std::fs::write(outside.path().join("secret.txt"), "private").unwrap();
    std::fs::write(outside.path().join("nested/secret.txt"), "private").unwrap();
    let ws = Workspace::open(root.path()).unwrap();
    std::fs::write(aux.join("ok.txt"), "allowed").unwrap();
    let roots = vec![aux.clone()];
    assert!(ws.open_allowed_file(Path::new("ok.txt"), &roots).unwrap().1);
    assert!(
        ws.open_allowed_file(Path::new("/workspace/ok.txt"), &roots)
            .is_ok()
    );
    for path in ["../secret.txt", "/workspace/../../secret.txt"] {
        assert!(
            ws.open_allowed_file(Path::new(path), &roots).is_err(),
            "{path}"
        );
    }
    let absolute_traversal = aux.join("../secret.txt");
    assert!(absolute_traversal.is_file(), "attack fixture must exist");
    assert!(ws.open_allowed_file(&absolute_traversal, &roots).is_err());
    assert!(
        ws.open_allowed_file(&outside.path().join("secret.txt"), &roots)
            .is_err()
    );
    assert!(
        ws.open_allowed_file(root.path(), &roots).is_err(),
        "directories are not files"
    );
}

#[test]
fn independent_chat_workspaces_cannot_read_sibling_data() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("workspaces/chats/11");
    let second = dir.path().join("workspaces/chats/22");
    let data_first = dir.path().join("data/11");
    let data_second = dir.path().join("data/22");
    for path in [&first, &second, &data_first, &data_second] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::write(second.join("secret.txt"), "other user's file").unwrap();
    std::fs::write(data_second.join("session.jsonl"), "other user's history").unwrap();
    let workspace = Workspace::open(first).unwrap();
    for path in [
        second.join("secret.txt"),
        data_second.join("session.jsonl"),
        Path::new("../22/secret.txt").to_path_buf(),
        Path::new("/workspace/../22/secret.txt").to_path_buf(),
    ] {
        assert!(
            workspace
                .open_allowed_file(&path, &[data_first.clone()])
                .is_err(),
            "{}",
            path.display()
        );
    }
}

#[cfg(unix)]
#[test]
fn auxiliary_symlink_cannot_escape_and_open_handle_survives_replacement() {
    use std::{io::Read, os::unix::fs::symlink};
    let root = tempfile::tempdir().unwrap();
    let aux = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), "secret").unwrap();
    symlink(outside.path().join("secret"), aux.path().join("link")).unwrap();
    let ws = Workspace::open(root.path()).unwrap();
    let roots = vec![aux.path().to_path_buf()];
    assert!(ws.open_allowed_file(Path::new("link"), &roots).is_err());
    std::fs::write(aux.path().join("safe"), "safe").unwrap();
    let (_, _, _, mut file) = ws.open_allowed_file(Path::new("safe"), &roots).unwrap();
    std::fs::remove_file(aux.path().join("safe")).unwrap();
    symlink(outside.path().join("secret"), aux.path().join("safe")).unwrap();
    let mut content = String::new();
    file.read_to_string(&mut content).unwrap();
    assert_eq!(content, "safe");
}

// A real child process using this test binary: portable across Unix and Windows.
#[test]
fn process_helper() {
    let Ok(role) = std::env::var("DUGA_TEST_ROLE") else {
        return;
    };
    if role == "child" {
        std::thread::sleep(Duration::from_millis(600));
        std::fs::write(
            std::env::var("DUGA_TEST_MARKER").unwrap(),
            "orphan survived",
        )
        .unwrap();
    } else {
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "process_helper", "--nocapture"])
            .env("DUGA_TEST_ROLE", "child")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        if role == "parent" {
            std::thread::sleep(Duration::from_secs(10));
        }
    }
}

async fn run_helper(
    ws: &Workspace,
    marker: &Path,
    role: &str,
    timeout: Duration,
    cancel: CancellationToken,
) -> Result<duga_types::tool_result::ToolResult, ToolError> {
    let env = HashMap::from([
        ("DUGA_TEST_ROLE".into(), role.into()),
        ("DUGA_TEST_MARKER".into(), marker.display().to_string()),
    ]);
    run_captured(
        &std::env::current_exe().unwrap(),
        &[
            "--exact".into(),
            "process_helper".into(),
            "--nocapture".into(),
        ],
        ws,
        Path::new("."),
        &env,
        &OutputLimits::default(),
        timeout,
        cancel,
    )
    .await
}

#[tokio::test]
async fn timeout_kills_descendants() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::open(dir.path()).unwrap();
    let marker = dir.path().join("orphan");
    let result = run_helper(
        &ws,
        &marker,
        "parent",
        Duration::from_millis(150),
        CancellationToken::new(),
    )
    .await;
    assert!(matches!(result, Err(ToolError::Timeout)), "{result:?}");
    tokio::time::sleep(Duration::from_millis(850)).await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn parent_exit_does_not_hang_on_descendant_pipes() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::open(dir.path()).unwrap();
    let marker = dir.path().join("orphan");
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        run_helper(
            &ws,
            &marker,
            "early_exit",
            Duration::from_millis(300),
            CancellationToken::new(),
        ),
    )
    .await;
    assert!(result.is_ok(), "pipe drain escaped the deadline");
    assert!(result.unwrap().is_ok());
    tokio::time::sleep(Duration::from_millis(850)).await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn cancellation_and_future_drop_kill_descendants() {
    for abort in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let marker = dir.path().join("orphan");
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task_marker = marker.clone();
        let handle = tokio::spawn(async move {
            run_helper(
                &ws,
                &task_marker,
                "parent",
                Duration::from_secs(10),
                task_cancel,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
        if abort {
            handle.abort();
            assert!(handle.await.unwrap_err().is_cancelled());
        } else {
            cancel.cancel();
            assert!(matches!(handle.await.unwrap(), Err(ToolError::Cancelled)));
        }
        tokio::time::sleep(Duration::from_millis(850)).await;
        assert!(!marker.exists(), "descendant survived (abort={abort})");
    }
}
