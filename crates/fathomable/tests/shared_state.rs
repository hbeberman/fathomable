//! Group-shared external state ancestors at process boundaries.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use fathomable_testing::TempDir;

type Result = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn doctor_warns_without_failure_and_clears_after_group_write_is_removed() -> Result {
    let fixture = TempDir::new("shared-state-doctor")?;
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o755))?;
    let workspace = fixture.0.join("workspace");
    fs::create_dir(&workspace)?;
    let state_home = fixture.0.join("shared-state");
    fs::create_dir(&state_home)?;
    fs::set_permissions(&state_home, fs::Permissions::from_mode(0o775))?;

    let run = || {
        Command::new(env!("CARGO_BIN_EXE_fathomable"))
            .arg("--doctor")
            .env("XDG_STATE_HOME", &state_home)
            .env("XDG_CONFIG_HOME", fixture.0.join("config"))
            .current_dir(&workspace)
            .output()
    };
    let warned = run()?;
    assert!(warned.status.success());
    let stdout = String::from_utf8(warned.stdout)?;
    assert!(stdout.contains("WARN  state ancestor is group-writable:"));
    assert!(stdout.contains(&state_home.display().to_string()));
    assert!(stdout.contains("chmod g-w"));
    assert!(stdout.contains("private XDG_STATE_HOME"));

    fs::set_permissions(&state_home, fs::Permissions::from_mode(0o755))?;
    let clean = run()?;
    assert!(clean.status.success());
    let stdout = String::from_utf8(clean.stdout)?;
    assert!(!stdout.contains("WARN"));
    assert!(stdout.contains("ok    state directory ancestors are not group-writable"));
    Ok(())
}
