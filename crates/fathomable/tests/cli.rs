//! Command-line configuration overrides.

use std::fs;
use std::process::Command;

use fathomable_testing::TempDir;

type Result = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn limit_flags_override_the_file_in_config_show() -> Result {
    let fixture = TempDir::new("cli-limits")?;
    let config = fixture.0.join("config.kdl");
    fs::write(
        &config,
        "limits { discovery-entries 1; workspace-watches 2; retained-paths 3; comparison-paths 4; comparison-bytes 5; pending-events 6 }\n",
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_fathomable"))
        .args([
            "--config",
            config.to_str().ok_or("config path is not UTF-8")?,
            "--config-show",
            "--discovery-entries",
            "101",
            "--workspace-watches",
            "102",
            "--retained-paths",
            "103",
            "--comparison-paths",
            "104",
            "--comparison-bytes",
            "105",
            "--pending-events",
            "106",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    for setting in [
        "discovery-entries 101",
        "workspace-watches 102",
        "retained-paths 103",
        "comparison-paths 104",
        "comparison-bytes 105",
        "pending-events 106",
    ] {
        assert!(stdout.contains(setting), "missing {setting}:\n{stdout}");
    }
    Ok(())
}

#[test]
fn limit_flags_reject_zero_and_repetition() -> Result {
    for flag in [
        "--discovery-entries",
        "--workspace-watches",
        "--retained-paths",
        "--comparison-paths",
        "--comparison-bytes",
        "--pending-events",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_fathomable"))
            .args(["--config-show", flag, "0"])
            .output()?;
        assert!(!output.status.success(), "{flag} accepted zero");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("invalid value"),
            "{flag}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let repeated = Command::new(env!("CARGO_BIN_EXE_fathomable"))
        .args([
            "--config-show",
            "--pending-events",
            "1",
            "--pending-events",
            "2",
        ])
        .output()?;
    assert!(!repeated.status.success(), "repeated flag was accepted");
    Ok(())
}
