//! Command-line configuration overrides.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

use fathomable_core::XdgDirs;
use fathomable_core::session::{Id, Marker, Record};
use fathomable_testing::TempDir;
use fathomable_testing::git;

type Result = std::result::Result<(), Box<dyn std::error::Error>>;

struct BoundedChild(Child);

impl BoundedChild {
    fn spawn(command: &mut Command) -> std::io::Result<Self> {
        command.spawn().map(Self)
    }

    fn wait(&mut self, timeout: Duration) -> std::io::Result<Option<ExitStatus>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.0.try_wait()? {
                return Ok(Some(status));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for BoundedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn dirs(fixture: &TempDir) -> XdgDirs {
    XdgDirs::resolve(|name| match name {
        "XDG_CONFIG_HOME" => Some(OsString::from(fixture.0.join("config"))),
        "XDG_STATE_HOME" => Some(OsString::from(fixture.0.join("state"))),
        _ => None,
    })
}

fn command(fixture: &TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fathomable"));
    command
        .env("XDG_CONFIG_HOME", fixture.0.join("config"))
        .env("XDG_STATE_HOME", fixture.0.join("state"));
    command
}

#[test]
fn mutable_mcp_root_requires_mcp() -> Result {
    let fixture = TempDir::new("cli-mutable-mcp-root")?;
    let without_mcp = command(&fixture).arg("--allow-mutable-mcp-root").output()?;
    assert!(!without_mcp.status.success());
    assert!(
        String::from_utf8(without_mcp.stderr)?.contains("--mcp"),
        "the error should name the required flag"
    );
    Ok(())
}

fn viewers(fixture: &TempDir) -> std::io::Result<Output> {
    command(fixture).arg("--viewers").output()
}

fn section(stdout: &str, heading: &Path) -> Option<String> {
    let heading = heading.to_string_lossy();
    let start = stdout
        .lines()
        .position(|line| line == heading)?
        .checked_add(1)?;
    let lines: Vec<_> = stdout.lines().collect();
    let end = lines[start..]
        .iter()
        .position(|line| !line.starts_with("  "))
        .map_or(lines.len(), |offset| start + offset);
    Some(lines[start..end].join("\n"))
}

#[test]
fn limit_flags_override_the_file_in_config_show() -> Result {
    let fixture = TempDir::new("cli-limits")?;
    let config = fixture.0.join("config.kdl");
    fs::write(
        &config,
        "limits { discovery-entries 1; workspace-watches 2; retained-paths 3; comparison-paths 4; comparison-bytes 5; pending-events 6 }\n",
    )?;
    let output = command(&fixture)
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
    assert!(
        stdout.starts_with(&format!("config {}\n", config.display())),
        "the explicit configuration path was not reported:\n{stdout}"
    );
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
fn viewer_startup_rejects_an_invalid_explicit_config() -> Result {
    let fixture = TempDir::new("cli-invalid-config")?;
    let config = fixture.0.join("invalid.kdl");
    fs::write(&config, "layout {")?;

    let output = command(&fixture)
        .args([
            "--config",
            config.to_str().ok_or("config path is not UTF-8")?,
        ])
        .current_dir(&fixture.0)
        .output()?;

    assert!(
        !output.status.success(),
        "invalid config started the viewer"
    );
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains(&config.display().to_string()),
        "error did not identify the invalid config:\n{stderr}"
    );
    Ok(())
}

#[test]
fn viewer_startup_marks_every_linked_worktree_before_entering_the_terminal() -> Result {
    let fixture = TempDir::new("cli-worktree-marker")?;
    let main = fixture.0.join("main");
    fs::create_dir(&main)?;
    git::init(&main)?;
    git::commit_and_stage(&main, &[("README.md", "main\n")])?;
    let linked = fixture.0.join("linked");
    git::worktree_add(&main, &linked, "linked")?;

    let mut detached = Command::new("setsid");
    detached
        .arg(env!("CARGO_BIN_EXE_fathomable"))
        .arg(&main)
        .env("XDG_CONFIG_HOME", fixture.0.join("config"))
        .env("XDG_STATE_HOME", fixture.0.join("state"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = BoundedChild::spawn(&mut detached)?;
    let status = child
        .wait(Duration::from_secs(5))?
        .ok_or("viewer did not stop without a controlling terminal")?;

    assert!(
        !status.success(),
        "viewer unexpectedly entered a terminal-free session"
    );
    let markers = Marker::list(&dirs(&fixture));
    assert_eq!(markers.len(), 1);
    assert_eq!(
        markers[0].roots(),
        [main.canonicalize()?, linked.canonicalize()?]
    );
    Ok(())
}

#[test]
fn viewers_reports_an_empty_state() -> Result {
    let fixture = TempDir::new("cli-viewers-empty")?;
    let output = viewers(&fixture)?;

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout)?, "no workspaces\n");
    Ok(())
}

#[test]
fn viewers_reports_a_marker_without_running_viewers() -> Result {
    let fixture = TempDir::new("cli-viewers-marker")?;
    let root = fixture.0.join("marked-workspace");
    Marker::new(root.clone(), vec![root.clone()]).write(&dirs(&fixture))?;

    let output = viewers(&fixture)?;

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!("{}\n  (no viewers)\n", root.display())
    );
    Ok(())
}

#[test]
fn viewers_reports_a_record_without_a_marker() -> Result {
    let fixture = TempDir::new("cli-viewers-record")?;
    let root = fixture.0.join("recorded-workspace");
    let id: Id = "1700000000-101".parse()?;
    Record::new(id.clone(), root.clone(), root.clone()).write(&dirs(&fixture))?;

    let output = viewers(&fixture)?;

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!("{}\n  {id}\tlive\n", root.display())
    );
    Ok(())
}

#[test]
fn viewers_associates_records_and_worktrees_with_their_workspace() -> Result {
    let fixture = TempDir::new("cli-viewers-workspaces")?;
    let dirs = dirs(&fixture);
    let alpha_key = fixture.0.join("alpha.git");
    let alpha_main = fixture.0.join("alpha");
    let alpha_linked = fixture.0.join("alpha-linked");
    let alpha_new = fixture.0.join("alpha-new");
    let beta_key = fixture.0.join("beta.git");
    let beta_root = fixture.0.join("beta");
    Marker::new(
        alpha_key.clone(),
        vec![alpha_main.clone(), alpha_linked.clone()],
    )
    .write(&dirs)?;
    Marker::new(beta_key, vec![beta_root.clone()]).write(&dirs)?;
    let main_id: Id = "1700000000-201".parse()?;
    let new_id: Id = "1700000000-202".parse()?;
    Record::new(main_id.clone(), alpha_key.clone(), alpha_main.clone()).write(&dirs)?;
    Record::new(new_id.clone(), alpha_key.clone(), alpha_new.clone()).write(&dirs)?;

    let output = viewers(&fixture)?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    let alpha = section(&stdout, &alpha_key).ok_or("missing alpha workspace")?;
    let main_line = format!("  worktree {}", alpha_main.display());
    assert_eq!(
        stdout.lines().filter(|line| *line == main_line).count(),
        1,
        "the marker root was duplicated:\n{stdout}"
    );
    for root in [&alpha_main, &alpha_linked, &alpha_new] {
        assert!(
            alpha.contains(&format!("  worktree {}", root.display())),
            "alpha omitted worktree {}:\n{stdout}",
            root.display()
        );
    }
    for (id, root) in [(&main_id, &alpha_main), (&new_id, &alpha_new)] {
        assert!(
            alpha.contains(&format!("  {id}\tlive\ton {}", root.display())),
            "viewer {id} was not associated with alpha:\n{stdout}"
        );
    }
    let beta = section(&stdout, &beta_root).ok_or("missing beta workspace")?;
    assert_eq!(beta, "  (no viewers)", "{stdout}");
    Ok(())
}

#[test]
fn limit_flags_reject_zero_and_repetition() -> Result {
    let fixture = TempDir::new("cli-limit-errors")?;
    for flag in [
        "--discovery-entries",
        "--workspace-watches",
        "--retained-paths",
        "--comparison-paths",
        "--comparison-bytes",
        "--pending-events",
    ] {
        let output = command(&fixture)
            .args(["--config-show", flag, "0"])
            .output()?;
        assert!(!output.status.success(), "{flag} accepted zero");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("invalid value"),
            "{flag}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let repeated = command(&fixture)
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
