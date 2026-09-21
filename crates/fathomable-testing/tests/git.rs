//! Git fixture configuration stays independent of the contributor's identity.

use std::error::Error;
use std::fs;
use std::process::Command;

use fathomable_testing::TempDir;
use fathomable_testing::git::{commit_and_stage, init, open_options};

#[test]
fn fixture_configuration_ignores_external_identity() -> Result<(), Box<dyn Error>> {
    const CHILD_REPO: &str = "FATHOMABLE_GIT_FIXTURE_CHILD_REPO";
    if let Some(root) = std::env::var_os(CHILD_REPO) {
        let root = std::path::Path::new(&root);
        let repo = gix::open_opts(root, open_options())?;
        assert!(
            repo.committer().is_none(),
            "external identity must not hide missing fixture setup"
        );
        commit_and_stage(root, &[("a.md", "fixture\n")])?;
        assert_eq!(repo.head_commit()?.committer()?.name, "test");
        return Ok(());
    }

    let fixture = TempDir::new("git-fixture-external-identity")?;
    let root = fixture.0.join("repo");
    fs::create_dir(&root)?;
    init(&root)?;
    let config = fixture.0.join(".gitconfig");
    let identity = "[user]\nname = external\nemail = external@example.com\n";
    fs::write(&config, identity)?;
    let xdg = fixture.0.join("config");
    fs::create_dir_all(xdg.join("git"))?;
    fs::write(xdg.join("git/config"), identity)?;
    let output = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "fixture_configuration_ignores_external_identity",
            "--nocapture",
        ])
        .env(CHILD_REPO, &root)
        .env("HOME", &fixture.0)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("GIT_CONFIG_SYSTEM", &config)
        .env("GIT_COMMITTER_NAME", "external")
        .env("GIT_COMMITTER_EMAIL", "external@example.com")
        .env("EMAIL", "external@example.com")
        .output()?;
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
