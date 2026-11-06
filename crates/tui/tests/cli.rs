use std::process::Command;

#[test]
fn diagnostics_are_pretty_json_on_stderr_for_success_and_error() {
    let binary = env!("CARGO_BIN_EXE_objekte");
    let success = Command::new(binary)
        .args(["--help", "--diagnostic"])
        .output()
        .unwrap();
    assert!(success.status.success());
    assert!(String::from_utf8_lossy(&success.stdout).contains("Usage: objekte"));
    let success_text = String::from_utf8(success.stderr).unwrap();
    assert!(success_text.contains("\n  \""));
    let event: serde_json::Value = serde_json::from_str(&success_text).unwrap();
    assert_eq!(event["level"], "info");
    assert!(event["timestamp_ms"].as_u64().unwrap() > 0);
    assert!(event["elapsed_ms"].as_f64().unwrap() >= 0.0);

    let failure = Command::new(binary)
        .args(["--unknown", "--diagnostics"])
        .output()
        .unwrap();
    assert!(!failure.status.success());
    let event: serde_json::Value = serde_json::from_slice(&failure.stderr).unwrap();
    assert_eq!(event["level"], "error");
    assert!(
        event["message"]
            .as_str()
            .unwrap()
            .contains("unknown option")
    );
    assert!(failure.stdout.is_empty());
}
