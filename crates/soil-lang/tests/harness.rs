// SPDX-License-Identifier: MIT OR Apache-2.0
use std::path::Path;
use std::process::Command;

#[test]
fn harness_evaluates_main_from_a_file() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(env!("CARGO_BIN_EXE_soil-harness"))
        .args([
            "--prelude",
            dir.join("stdlib/prelude.soil").to_str().unwrap(),
            "--source",
            dir.join("examples/harness.soil").to_str().unwrap(),
        ])
        .output()
        .expect("run harness");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "(:answer 42)"
    );
}
