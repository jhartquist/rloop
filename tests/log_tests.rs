use rloop::log::{Event, SessionLogger};
use rloop::session::SessionId;
use rloop::task::TaskId;

#[test]
fn logger_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = SessionId("test-session-01".into());
    let mut logger = SessionLogger::new(dir.path(), session_id).unwrap();
    logger.log(&Event::SessionCancelled).unwrap();
    // Check that a .jsonl file was created
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    assert_eq!(entries.len(), 1);
}

#[test]
fn logger_writes_valid_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = SessionId("test-jsonl".into());
    let mut logger = SessionLogger::new(dir.path(), session_id).unwrap();

    logger
        .log(&Event::TaskStarted { task_id: TaskId(0), attempt: 1 })
        .unwrap();
    logger
        .log(&Event::TaskCompleted {
            task_id: TaskId(0),
            attempt: 1,
            summary: "done".into(),
        })
        .unwrap();
    logger.log(&Event::SessionCancelled).unwrap();

    // Read the file and verify each line is valid JSON
    let log_file = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .expect("log file should exist");

    let content = std::fs::read_to_string(log_file.path()).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 3);
    for line in &lines {
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(line);
        assert!(parsed.is_ok(), "Line is not valid JSON: {line}");
    }
}

#[test]
fn separate_session_files() {
    let dir = tempfile::tempdir().unwrap();

    let mut logger1 = SessionLogger::new(dir.path(), SessionId("session-a".into())).unwrap();
    let mut logger2 = SessionLogger::new(dir.path(), SessionId("session-b".into())).unwrap();

    logger1.log(&Event::SessionCancelled).unwrap();
    logger2.log(&Event::SessionCancelled).unwrap();

    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    assert_eq!(
        entries.len(),
        2,
        "Each session should produce a separate log file"
    );
}
