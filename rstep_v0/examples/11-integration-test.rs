//! Example 11: Integration Test with Replay
//!
//! Demonstrates how to write tests using recorded fixtures.
//!
//! This example shows the pattern for:
//! 1. Loading a recorded session fixture
//! 2. Setting up a temp directory with a template
//! 3. Replaying the recording (applying edits)
//! 4. Verifying the result matches expectations
//! 5. Running verification (cargo test)
//!
//! Key benefit:
//!   - Tests are FREE (no API calls)
//!   - Tests are FAST (no network latency)
//!   - Tests are DETERMINISTIC (same result every time)
//!
//! The pattern here can be adapted to actual #[test] functions.
//!
//! Run with:
//!   cargo run --example 11-integration-test

use anyhow::{anyhow, Result};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::Path;
use std::process::Command;
use std::time::Instant;
use tempfile::tempdir;

// ============================================================
// TYPES (same as ex-10)
// ============================================================

/// Recording format
#[derive(Debug, Serialize, Deserialize)]
struct Recording {
    metadata: RecordingMetadata,
    messages: Vec<RecordedMessage>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RecordingMetadata {
    recorded_at: String,
    model: String,
    task: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RecordedMessage {
    timestamp: String,
    message: serde_json::Value,
}

/// Structured result from Claude
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(tag = "result")]
enum StepResult {
    #[serde(rename = "success")]
    Success { summary: String },
    #[serde(rename = "failed")]
    Failed {
        reason: String,
        next_steps: Vec<String>,
    },
    #[serde(rename = "blocked")]
    Blocked { blocker: String },
}

impl StepResult {
    fn is_success(&self) -> bool {
        matches!(self, StepResult::Success { .. })
    }
}

#[derive(Debug)]
struct EditOperation {
    file_path: String,
    old_string: String,
    new_string: String,
}

// ============================================================
// TEST RESULT TYPE
// ============================================================

/// Result of running a test
#[derive(Debug)]
struct TestResult {
    name: String,
    passed: bool,
    duration_ms: u64,
    message: Option<String>,
}

impl TestResult {
    fn pass(name: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            name: name.into(),
            passed: true,
            duration_ms,
            message: None,
        }
    }

    fn fail(name: impl Into<String>, duration_ms: u64, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            passed: false,
            duration_ms,
            message: Some(message.into()),
        }
    }
}

// ============================================================
// HELPER FUNCTIONS (reused from ex-10)
// ============================================================

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if ty.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn run_cargo_test(dir: &Path) -> Result<(bool, String)> {
    let output = Command::new("cargo")
        .arg("test")
        .current_dir(dir)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    Ok((output.status.success(), format!("{}{}", stdout, stderr)))
}

fn extract_json(text: &str) -> Option<String> {
    // Try code blocks first
    let code_block_patterns = [(r"```json\s*", r"\s*```"), (r"```\s*", r"\s*```")];

    for (start_pattern, end_pattern) in code_block_patterns {
        let re_start = regex::Regex::new(start_pattern).ok()?;
        let re_end = regex::Regex::new(end_pattern).ok()?;

        let mut last_json = None;
        let mut search_text = text;

        while let Some(start_match) = re_start.find(search_text) {
            let after_start = &search_text[start_match.end()..];
            if let Some(end_match) = re_end.find(after_start) {
                let potential_json = &after_start[..end_match.start()];
                if serde_json::from_str::<serde_json::Value>(potential_json.trim()).is_ok() {
                    last_json = Some(potential_json.trim().to_string());
                }
                search_text = &after_start[end_match.end()..];
            } else {
                break;
            }
        }

        if last_json.is_some() {
            return last_json;
        }
    }

    // Fall back to raw JSON
    let mut last_json = None;
    let mut depth = 0;
    let mut start_idx = None;

    for (i, c) in text.char_indices() {
        match c {
            '{' => {
                if depth == 0 {
                    start_idx = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = start_idx {
                        let potential_json = &text[start..=i];
                        if serde_json::from_str::<serde_json::Value>(potential_json).is_ok() {
                            last_json = Some(potential_json.to_string());
                        }
                    }
                    start_idx = None;
                }
            }
            _ => {}
        }
    }

    last_json
}

fn parse_step_result(text: &str) -> Result<StepResult> {
    let json_str = extract_json(text).ok_or_else(|| anyhow!("No JSON found in response"))?;
    serde_json::from_str(&json_str).map_err(|e| anyhow!("Failed to parse StepResult: {}", e))
}

fn find_latest_recording() -> Result<std::path::PathBuf> {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/recorded");

    let mut recordings: Vec<_> = fs::read_dir(&fixtures_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.path().extension().is_some_and(|ext| ext == "json")
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("recording-")
        })
        .collect();

    recordings.sort_by_key(|e| e.file_name());

    recordings
        .last()
        .map(|e| e.path())
        .ok_or_else(|| anyhow!("No recordings found in {:?}", fixtures_dir))
}

fn load_recording(path: &Path) -> Result<Recording> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let recording: Recording = serde_json::from_reader(reader)?;
    Ok(recording)
}

fn extract_edits(messages: &[RecordedMessage]) -> Vec<EditOperation> {
    let mut edits = Vec::new();

    for msg in messages {
        if msg.message.get("type").and_then(|v| v.as_str()) == Some("assistant") {
            if let Some(content) = msg.message.get("message").and_then(|m| m.get("content")) {
                if let Some(blocks) = content.as_array() {
                    for block in blocks {
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                            if block.get("name").and_then(|n| n.as_str()) == Some("Edit") {
                                if let Some(input) = block.get("input") {
                                    let file_path = input
                                        .get("file_path")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let old_string = input
                                        .get("old_string")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let new_string = input
                                        .get("new_string")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();

                                    if !old_string.is_empty() && !new_string.is_empty() {
                                        edits.push(EditOperation {
                                            file_path,
                                            old_string,
                                            new_string,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    edits
}

fn extract_response_text(messages: &[RecordedMessage]) -> String {
    let mut text = String::new();

    for msg in messages {
        if msg.message.get("type").and_then(|v| v.as_str()) == Some("assistant") {
            if let Some(content) = msg.message.get("message").and_then(|m| m.get("content")) {
                if let Some(blocks) = content.as_array() {
                    for block in blocks {
                        if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                text.push_str(t);
                                text.push('\n');
                            }
                        }
                    }
                }
            }
        }
    }

    text
}

fn apply_edit(edit: &EditOperation, temp_dir: &Path) -> Result<()> {
    let relative_path = if let Some(idx) = edit.file_path.rfind("/src/") {
        &edit.file_path[idx + 1..]
    } else if let Some(idx) = edit.file_path.rfind("/Cargo.toml") {
        &edit.file_path[idx + 1..]
    } else {
        Path::new(&edit.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&edit.file_path)
    };

    let local_path = temp_dir.join(relative_path);

    let content = fs::read_to_string(&local_path)?;
    let new_content = content.replace(&edit.old_string, &edit.new_string);

    if content == new_content {
        return Err(anyhow!(
            "Edit had no effect - old_string not found in {}",
            local_path.display()
        ));
    }

    fs::write(&local_path, new_content)?;
    Ok(())
}

// ============================================================
// TEST FUNCTIONS
// ============================================================

/// Test: Recording can be loaded
fn test_recording_loads(recording_path: &Path) -> TestResult {
    let start = Instant::now();
    let name = "recording_loads";

    match load_recording(recording_path) {
        Ok(recording) => {
            if recording.messages.is_empty() {
                TestResult::fail(name, start.elapsed().as_millis() as u64, "Recording has no messages")
            } else {
                TestResult::pass(name, start.elapsed().as_millis() as u64)
            }
        }
        Err(e) => TestResult::fail(name, start.elapsed().as_millis() as u64, e.to_string()),
    }
}

/// Test: StepResult can be extracted from recording
fn test_step_result_extractable(recording: &Recording) -> TestResult {
    let start = Instant::now();
    let name = "step_result_extractable";

    let response_text = extract_response_text(&recording.messages);

    match parse_step_result(&response_text) {
        Ok(_) => TestResult::pass(name, start.elapsed().as_millis() as u64),
        Err(e) => TestResult::fail(name, start.elapsed().as_millis() as u64, e.to_string()),
    }
}

/// Test: StepResult is Success
fn test_step_result_is_success(recording: &Recording) -> TestResult {
    let start = Instant::now();
    let name = "step_result_is_success";

    let response_text = extract_response_text(&recording.messages);

    match parse_step_result(&response_text) {
        Ok(result) => {
            if result.is_success() {
                TestResult::pass(name, start.elapsed().as_millis() as u64)
            } else {
                TestResult::fail(
                    name,
                    start.elapsed().as_millis() as u64,
                    format!("Expected Success, got {:?}", result),
                )
            }
        }
        Err(e) => TestResult::fail(name, start.elapsed().as_millis() as u64, e.to_string()),
    }
}

/// Test: Edits can be extracted from recording
fn test_edits_extractable(recording: &Recording) -> TestResult {
    let start = Instant::now();
    let name = "edits_extractable";

    let edits = extract_edits(&recording.messages);

    if edits.is_empty() {
        TestResult::fail(
            name,
            start.elapsed().as_millis() as u64,
            "No edits found in recording",
        )
    } else {
        TestResult::pass(name, start.elapsed().as_millis() as u64)
    }
}

/// Test: Edits can be applied to template
fn test_edits_apply(recording: &Recording, temp_dir: &Path) -> TestResult {
    let start = Instant::now();
    let name = "edits_apply";

    let edits = extract_edits(&recording.messages);

    for edit in &edits {
        if let Err(e) = apply_edit(edit, temp_dir) {
            return TestResult::fail(name, start.elapsed().as_millis() as u64, e.to_string());
        }
    }

    TestResult::pass(name, start.elapsed().as_millis() as u64)
}

/// Test: Verification passes after edits
fn test_verification_passes(temp_dir: &Path) -> TestResult {
    let start = Instant::now();
    let name = "verification_passes";

    match run_cargo_test(temp_dir) {
        Ok((passed, output)) => {
            if passed {
                TestResult::pass(name, start.elapsed().as_millis() as u64)
            } else {
                TestResult::fail(
                    name,
                    start.elapsed().as_millis() as u64,
                    format!("cargo test failed:\n{}", output),
                )
            }
        }
        Err(e) => TestResult::fail(name, start.elapsed().as_millis() as u64, e.to_string()),
    }
}

// ============================================================
// TEST RUNNER
// ============================================================

fn run_tests(results: Vec<TestResult>) -> bool {
    println!("\n{}", "Test Results:".blue().bold());
    println!("{}", "=".repeat(60));

    let mut all_passed = true;
    let mut total_duration_ms = 0u64;

    for result in &results {
        total_duration_ms += result.duration_ms;

        let status = if result.passed {
            "PASS".green().bold()
        } else {
            all_passed = false;
            "FAIL".red().bold()
        };

        let duration = format!("({}ms)", result.duration_ms).dimmed();

        println!("  {} {} {}", status, result.name, duration);

        if let Some(msg) = &result.message {
            // Indent failure message
            for line in msg.lines() {
                println!("       {}", line.red());
            }
        }
    }

    println!("{}", "=".repeat(60));

    let passed_count = results.iter().filter(|r| r.passed).count();
    let total_count = results.len();

    if all_passed {
        println!(
            "\n{} {} tests passed in {}ms",
            "OK".green().bold(),
            total_count,
            total_duration_ms
        );
    } else {
        println!(
            "\n{} {}/{} tests passed in {}ms",
            "FAILED".red().bold(),
            passed_count,
            total_count,
            total_duration_ms
        );
    }

    all_passed
}

// ============================================================
// MAIN - RUNS LIKE A TEST SUITE
// ============================================================

#[tokio::main]
async fn main() -> Result<()> {
    println!(
        "\n{}",
        "+----------------------------------------------------------+"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|              INTEGRATION TEST EXAMPLE                     |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                                                           |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|  This shows how to write tests using recorded fixtures.   |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|  Tests are FREE, FAST, and DETERMINISTIC.                 |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+----------------------------------------------------------+"
            .cyan()
            .bold()
    );

    // ============================================================
    // SETUP
    // ============================================================
    let overall_start = Instant::now();

    println!("\n{}", "Setup:".yellow().bold());
    println!("{}", "-".repeat(60).dimmed());

    // Load recording
    let recording_path = find_latest_recording()?;
    println!(
        "  {} {}",
        "Recording:".dimmed(),
        recording_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .cyan()
    );

    // Create temp directory with template
    let temp = tempdir()?;
    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/bug-adder");
    copy_dir_all(&template_dir, temp.path())?;
    println!(
        "  {} {}",
        "Template:".dimmed(),
        "bug-adder".white().bold()
    );
    println!(
        "  {} {}",
        "Temp dir:".dimmed(),
        temp.path().display().to_string().dimmed()
    );
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // RUN TESTS
    // ============================================================
    println!("\n{}", "Running tests...".yellow().bold());

    let recording = load_recording(&recording_path)?;

    // Collect test results
    // Note: tests run in order, some depend on previous state
    let mut results = Vec::new();

    // Test 1: Recording loads
    results.push(test_recording_loads(&recording_path));

    // Test 2: StepResult can be extracted
    results.push(test_step_result_extractable(&recording));

    // Test 3: StepResult is Success
    results.push(test_step_result_is_success(&recording));

    // Test 4: Edits can be extracted
    results.push(test_edits_extractable(&recording));

    // Test 5: Edits apply successfully
    results.push(test_edits_apply(&recording, temp.path()));

    // Test 6: Verification passes (cargo test)
    results.push(test_verification_passes(temp.path()));

    // ============================================================
    // REPORT
    // ============================================================
    let all_passed = run_tests(results);

    let overall_duration = overall_start.elapsed();

    // ============================================================
    // COMPARISON: REPLAY VS LIVE
    // ============================================================
    println!("\n{}", "Performance Comparison:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!(
        "  {} {}",
        "This test took:".dimmed(),
        format!("{}ms", overall_duration.as_millis())
            .white()
            .bold()
    );
    println!(
        "  {} {}",
        "Live API would take:".dimmed(),
        "~5000-10000ms".yellow()
    );
    println!(
        "  {} {}",
        "Speedup:".dimmed(),
        format!(
            "~{}x faster",
            5000 / overall_duration.as_millis().max(1)
        )
        .green()
        .bold()
    );
    println!(
        "  {} {}",
        "API cost:".dimmed(),
        "$0.00 (FREE!)".cyan().bold()
    );
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // HOW TO ADAPT TO #[test]
    // ============================================================
    println!("\n{}", "Adapting to #[test] functions:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!(
        "{}",
        r#"
  // In tests/integration/replay_test.rs:

  #[test]
  fn test_bug_fix_replay() {
      let recording_path = find_recording("bug-fix-recording.json");
      let recording = load_recording(&recording_path).unwrap();

      let temp = tempdir().unwrap();
      copy_template("bug-adder", temp.path()).unwrap();

      // Apply edits from recording
      let edits = extract_edits(&recording.messages);
      for edit in edits {
          apply_edit(&edit, temp.path()).unwrap();
      }

      // Verify result
      let result = parse_step_result(&extract_response_text(&recording.messages));
      assert!(matches!(result, Ok(StepResult::Success { .. })));

      // Verify code works
      let (passed, _) = run_cargo_test(temp.path()).unwrap();
      assert!(passed, "cargo test should pass after fix");
  }
"#
        .dimmed()
    );
    println!("{}", "-".repeat(60).dimmed());

    // Final status
    if all_passed {
        println!(
            "\n{}",
            "+----------------------------------------------------------+"
                .green()
                .bold()
        );
        println!(
            "{}",
            "|                    ALL TESTS PASSED                       |"
                .green()
                .bold()
        );
        println!(
            "{}",
            "+----------------------------------------------------------+"
                .green()
                .bold()
        );
        Ok(())
    } else {
        println!(
            "\n{}",
            "+----------------------------------------------------------+"
                .red()
                .bold()
        );
        println!(
            "{}",
            "|                    SOME TESTS FAILED                      |"
                .red()
                .bold()
        );
        println!(
            "{}",
            "+----------------------------------------------------------+"
                .red()
                .bold()
        );
        std::process::exit(1);
    }
}
