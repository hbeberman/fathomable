//! Behaviour of the `HEAD` diff base (ADR 0006) against a real repository
//! built with `gix`, so the tests need no host `git`.

use std::error::Error;
#[cfg(unix)]
use std::ffi::OsString;
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::path::Path;

use fathomable_core::annotations::FullFileDigest;
use fathomable_core::config::LimitsConfig;
use fathomable_core::diff::{FileMode, PathState};
use fathomable_core::workspace::{
    CommitId, ComparisonEndpoint, ExactFile, ExactFileMatch, HeadState, HeadTransition,
    IndexManifestCapture, IndexManifestUnavailable, Workspace,
};
use fathomable_testing::TempDir;
use fathomable_testing::git::{
    commit_and_stage as fixture_commit_and_stage, init, open_options, stage, write_tree,
};

type TestResult = Result<(), Box<dyn Error>>;

fn write_loose_object(root: &Path, id: &str, compressed: &[u8]) -> TestResult {
    let objects = root.join(".git/objects").join(&id[..2]);
    fs::create_dir_all(&objects)?;
    let object = objects.join(&id[2..]);
    if object.exists() {
        fs::remove_file(&object)?;
    }
    fs::write(object, compressed)?;
    Ok(())
}

/// Commit `files` (root-relative path, content) as the only tree of `HEAD`.
fn commit(root: &Path, files: &[(&str, &str)]) -> Result<(), Box<dyn Error>> {
    let repo = gix::open_opts(root, open_options())?;
    let tree = write_tree(&repo, files)?;
    let signature = gix::actor::SignatureRef {
        name: "test".into(),
        email: "test@example.com".into(),
        time: "0 +0000",
    };
    let parent = repo.head_id().ok().map(gix::Id::detach);
    repo.commit_as(signature, signature, "HEAD", "commit", tree, parent)?;
    Ok(())
}

#[test]
fn typed_head_observation_distinguishes_all_states_and_transitions() -> TestResult {
    let plain = TempDir::new("typed-head-plain")?;
    let plain_workspace = Workspace::discover(&plain.0)?;
    assert!(matches!(
        plain_workspace.observe_head(1).state(),
        HeadState::Unavailable { .. }
    ));

    let dir = TempDir::new("typed-head")?;
    init(&dir.0)?;
    let workspace = Workspace::discover(&dir.0)?;
    let unborn = workspace.observe_head(2);
    assert_eq!(unborn.generation(), 2);
    assert!(matches!(unborn.state(), HeadState::Unborn));
    assert_eq!(unborn.checkout(), &workspace.identity());

    fixture_commit_and_stage(&dir.0, &[("a.md", "one\n")])?;
    let workspace = Workspace::discover(&dir.0)?;
    let symbolic = workspace.observe_head(3);
    let commit = symbolic
        .state()
        .commit()
        .ok_or("symbolic HEAD lacks commit")?
        .clone();
    assert!(matches!(
        symbolic.state(),
        HeadState::Symbolic { reference, .. } if reference.starts_with("refs/heads/")
    ));

    fs::write(dir.0.join(".git/HEAD"), format!("{commit}\n"))?;
    let workspace = Workspace::discover(&dir.0)?;
    let detached = workspace.observe_head(4);
    assert!(matches!(
        detached.state(),
        HeadState::Detached { commit: actual } if actual == &commit
    ));
    let transition = HeadTransition::between(&symbolic, &detached).ok_or("transition missing")?;
    assert_eq!(transition.generation(), 4);
    assert_eq!(transition.previous(), symbolic.state());
    assert_eq!(transition.current(), detached.state());

    fs::write(dir.0.join(".git/HEAD"), "not a valid HEAD\n")?;
    assert!(matches!(
        workspace.observe_head(5).state(),
        HeadState::Unavailable { .. }
    ));
    Ok(())
}

#[test]
fn exact_file_matching_is_bounded_and_reports_each_path() -> TestResult {
    let dir = TempDir::new("exact-file-matches")?;
    init(&dir.0)?;
    fixture_commit_and_stage(&dir.0, &[("a.md", "one\n"), ("b.md", "two\n")])?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let commit = workspace.exact_head_commit()?.id();
    let checkout = workspace.identity();
    let exact = ExactFile::new(
        checkout.clone(),
        "a.md",
        FullFileDigest::from_bytes(b"one\n"),
    );
    let candidates = [
        exact,
        ExactFile::new(
            checkout.clone(),
            "a.md",
            FullFileDigest::from_bytes(b"not one\n"),
        ),
        ExactFile::new(
            checkout.clone(),
            "b.md",
            FullFileDigest::from_bytes(b"different\n"),
        ),
        ExactFile::new(
            checkout.clone(),
            "missing.md",
            FullFileDigest::from_bytes(b""),
        ),
    ];
    let results = workspace.match_commit_files(&commit, &candidates)?;
    assert!(matches!(
        results.results()[0].result(),
        ExactFileMatch::Match
    ));
    assert!(matches!(
        results.results()[1].result(),
        ExactFileMatch::Mismatch
    ));
    assert!(matches!(
        results.results()[2].result(),
        ExactFileMatch::Mismatch
    ));
    assert!(matches!(
        results.results()[3].result(),
        ExactFileMatch::Missing
    ));

    let foreign = TempDir::new("exact-file-foreign")?;
    let foreign_identity = Workspace::discover(&foreign.0)?.identity();
    assert!(
        workspace
            .match_commit_files(
                &commit,
                &[ExactFile::new(
                    foreign_identity,
                    "a.md",
                    FullFileDigest::from_bytes(b"one\n"),
                )],
            )
            .is_err_and(|error| error.to_string().contains("another checkout"))
    );

    workspace.set_limits(LimitsConfig {
        comparison_paths: 1,
        ..LimitsConfig::default()
    });
    assert!(
        workspace
            .match_commit_files(&commit, &candidates[..2])
            .is_err_and(|error| error.to_string().contains("path"))
    );

    workspace.set_limits(LimitsConfig {
        comparison_paths: 10,
        comparison_bytes: 5,
        ..LimitsConfig::default()
    });
    let results = workspace.match_commit_files(
        &commit,
        &[
            ExactFile::new(
                checkout.clone(),
                "a.md",
                FullFileDigest::from_bytes(b"one\n"),
            ),
            ExactFile::new(
                checkout.clone(),
                "b.md",
                FullFileDigest::from_bytes(b"second\n"),
            ),
        ],
    )?;
    assert!(matches!(
        results.results()[0].result(),
        ExactFileMatch::Match
    ));
    assert_eq!(
        results.progress(),
        fathomable_core::workspace::ExactFileProgress::ContinueAt { next: 1 }
    );

    Ok(())
}

#[test]
fn oversized_exact_file_does_not_block_a_later_candidate() -> TestResult {
    let dir = TempDir::new("exact-file-oversized")?;
    init(&dir.0)?;
    fixture_commit_and_stage(&dir.0, &[("big.md", "123456"), ("small.md", "one\n")])?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let commit = workspace.exact_head_commit()?.id();
    let checkout = workspace.identity();
    workspace.set_limits(LimitsConfig {
        comparison_paths: 10,
        comparison_bytes: 5,
        ..LimitsConfig::default()
    });
    let results = workspace.match_commit_files(
        &commit,
        &[
            ExactFile::new(
                checkout.clone(),
                "big.md",
                FullFileDigest::from_bytes(b"123456"),
            ),
            ExactFile::new(checkout, "small.md", FullFileDigest::from_bytes(b"one\n")),
        ],
    )?;
    assert_eq!(
        results.progress(),
        fathomable_core::workspace::ExactFileProgress::Complete
    );
    assert!(matches!(
        results.results()[0].result(),
        ExactFileMatch::Unavailable(message) if message.contains("limit")
    ));
    assert!(matches!(
        results.results()[1].result(),
        ExactFileMatch::Match
    ));
    Ok(())
}

#[test]
fn index_manifest_is_immutable_and_compares_exact_tree_identity() -> TestResult {
    let dir = TempDir::new("index-manifest")?;
    init(&dir.0)?;
    fixture_commit_and_stage(&dir.0, &[("a.md", "one\n"), ("nested/b.bin", "\0binary\n")])?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let head = workspace.exact_head_commit()?.id();
    let IndexManifestCapture::Available(manifest) = workspace.index_manifest()? else {
        return Err("complete index unexpectedly unavailable".into());
    };
    assert_eq!(manifest.entries().len(), 2);
    assert_eq!(manifest.identity().len(), 64);
    assert!(workspace.index_manifest_matches_commit(&manifest, &head)?);
    assert_eq!(
        workspace.index_manifest_bytes(&manifest, Path::new("a.md"), 4)?,
        Some(b"one\n".to_vec())
    );

    stage(
        &dir.0,
        &[("a.md", "changed\n"), ("nested/b.bin", "\0binary\n")],
    )?;
    assert_eq!(
        workspace.index_manifest_bytes(&manifest, Path::new("a.md"), 4)?,
        Some(b"one\n".to_vec()),
        "captured object IDs must survive live-index mutation"
    );
    let captured = workspace.compare_with_index_manifest(
        ComparisonEndpoint::Index,
        ComparisonEndpoint::Commit(head.clone()),
        &manifest,
    )?;
    assert!(
        captured.changes().is_empty(),
        "comparison must enumerate and read the captured manifest, not the live index"
    );
    let IndexManifestCapture::Available(changed) = workspace.index_manifest()? else {
        return Err("changed index unexpectedly unavailable".into());
    };
    assert_ne!(manifest.identity(), changed.identity());
    assert!(!workspace.index_manifest_matches_commit(&changed, &head)?);

    let mut bounded = Workspace::discover(&dir.0)?;
    bounded.set_limits(LimitsConfig {
        comparison_paths: 1,
        ..LimitsConfig::default()
    });
    bounded
        .index_manifest()
        .err()
        .ok_or("index path limit was not enforced")?;
    bounded.set_limits(LimitsConfig {
        comparison_paths: 10,
        comparison_bytes: 1,
        ..LimitsConfig::default()
    });
    bounded
        .index_manifest()
        .err()
        .ok_or("index byte limit was not enforced")?;
    Ok(())
}

#[test]
fn index_manifest_rejects_intent_to_add_and_conflict_stages() -> TestResult {
    let dir = TempDir::new("index-manifest-unavailable")?;
    init(&dir.0)?;
    fixture_commit_and_stage(&dir.0, &[("a.md", "one\n")])?;
    let repo = gix::open_opts(&dir.0, open_options())?;

    let mut index = repo.open_index()?;
    index.entries_mut()[0].flags.insert(
        gix::index::entry::Flags::INTENT_TO_ADD
            | gix::index::entry::Flags::from_bits_retain(1 << 14),
    );
    index.write(gix::index::write::Options::default())?;
    let workspace = Workspace::discover(&dir.0)?;
    assert!(matches!(
        workspace.index_manifest()?,
        IndexManifestCapture::Unavailable(IndexManifestUnavailable::IntentToAdd { .. })
    ));

    stage(&dir.0, &[("a.md", "one\n")])?;
    let mut index = repo.open_index()?;
    index.entries_mut()[0].flags |= gix::index::entry::Flags::from_bits_retain(1 << 12);
    index.write(gix::index::write::Options::default())?;
    let workspace = Workspace::discover(&dir.0)?;
    assert!(matches!(
        workspace.index_manifest()?,
        IndexManifestCapture::Unavailable(IndexManifestUnavailable::Conflict { .. })
    ));
    Ok(())
}

#[test]
fn index_manifest_keeps_missing_gitlinks_as_metadata() -> TestResult {
    let dir = TempDir::new("index-manifest-missing-gitlink")?;
    init(&dir.0)?;
    fixture_commit_and_stage(&dir.0, &[("module", "placeholder\n")])?;
    let repo = gix::open_opts(&dir.0, open_options())?;
    let mut index = repo.open_index()?;
    let entry = &mut index.entries_mut()[0];
    entry.mode = gix::index::entry::Mode::COMMIT;
    entry.id = gix::ObjectId::from_hex(b"1111111111111111111111111111111111111111")?;
    index.write(gix::index::write::Options::default())?;

    let mut workspace = Workspace::discover(&dir.0)?;
    let IndexManifestCapture::Available(manifest) = workspace.index_manifest()? else {
        return Err("gitlink manifest unexpectedly unavailable".into());
    };
    let comparison = workspace.compare_with_index_manifest(
        ComparisonEndpoint::EmptyTree,
        ComparisonEndpoint::Index,
        &manifest,
    )?;
    let change = comparison
        .changes()
        .first()
        .ok_or("missing gitlink comparison change")?;
    assert_eq!(change.path(), Path::new("module"));
    let PathState::Present(target) = change.target() else {
        return Err("gitlink target is not present".into());
    };
    assert_eq!(target.mode(), FileMode::Submodule);
    assert!(!target.is_supported());
    assert_eq!(target.size(), None);
    Ok(())
}

#[cfg(unix)]
#[test]
fn index_manifest_preserves_tree_modes_and_index_only_flags() -> TestResult {
    let dir = TempDir::new("index-manifest-modes")?;
    init(&dir.0)?;
    commit_and_stage(
        &dir.0,
        &[
            (
                "executable",
                "#!/bin/sh\n",
                gix::objs::tree::EntryKind::BlobExecutable,
            ),
            ("link", "target", gix::objs::tree::EntryKind::Link),
            ("module", "opaque", gix::objs::tree::EntryKind::Commit),
            ("regular", "text\n", gix::objs::tree::EntryKind::Blob),
        ],
    )?;
    let repo = gix::open_opts(&dir.0, open_options())?;
    let mut index = repo.open_index()?;
    let at = index
        .entry_index_by_path("regular".into())
        .ok()
        .ok_or("regular index entry missing")?;
    index.entries_mut()[at].flags.insert(
        gix::index::entry::Flags::ASSUME_VALID
            | gix::index::entry::Flags::SKIP_WORKTREE
            | gix::index::entry::Flags::from_bits_retain(1 << 14),
    );
    index.write(gix::index::write::Options::default())?;

    let workspace = Workspace::discover(&dir.0)?;
    let head = workspace.exact_head_commit()?.id();
    let IndexManifestCapture::Available(manifest) = workspace.index_manifest()? else {
        return Err("mode manifest unexpectedly unavailable".into());
    };
    let modes: Vec<_> = manifest
        .entries()
        .iter()
        .map(|entry| (entry.path_bytes(), entry.mode()))
        .collect();
    assert_eq!(
        modes,
        vec![
            (b"executable".as_slice(), FileMode::Executable),
            (b"link".as_slice(), FileMode::Symlink),
            (b"module".as_slice(), FileMode::Submodule),
            (b"regular".as_slice(), FileMode::Regular),
        ]
    );
    let regular = manifest.entry(b"regular").ok_or("regular missing")?;
    assert!(regular.assume_unchanged());
    assert!(regular.skip_worktree());
    assert!(workspace.index_manifest_matches_commit(&manifest, &head)?);

    index.entries_mut()[at].mode = gix::index::entry::Mode::FILE_EXECUTABLE;
    index.write(gix::index::write::Options::default())?;
    let workspace = Workspace::discover(&dir.0)?;
    let IndexManifestCapture::Available(mode_changed) = workspace.index_manifest()? else {
        return Err("mode-changed manifest unexpectedly unavailable".into());
    };
    assert!(!workspace.index_manifest_matches_commit(&mode_changed, &head)?);
    Ok(())
}

#[test]
fn commit_manifest_bounds_shared_empty_tree_traversal() -> TestResult {
    let dir = TempDir::new("commit-manifest-shared-trees")?;
    init(&dir.0)?;
    fixture_commit_and_stage(&dir.0, &[("tracked", "base\n")])?;
    let repo = gix::open_opts(&dir.0, open_options())?;
    let parent = repo.head_id()?.detach();
    let mut tree = repo
        .write_object(gix::objs::Tree {
            entries: Vec::new(),
        })?
        .detach();
    for _ in 0..8 {
        tree = repo
            .write_object(gix::objs::Tree {
                entries: vec![
                    gix::objs::tree::Entry {
                        mode: gix::objs::tree::EntryKind::Tree.into(),
                        filename: "left".into(),
                        oid: tree,
                    },
                    gix::objs::tree::Entry {
                        mode: gix::objs::tree::EntryKind::Tree.into(),
                        filename: "right".into(),
                        oid: tree,
                    },
                ],
            })?
            .detach();
    }
    let signature = gix::actor::SignatureRef {
        name: "test".into(),
        email: "test@example.com".into(),
        time: "1 +0000",
    };
    let commit = repo
        .commit_as(
            signature,
            signature,
            "HEAD",
            "shared tree",
            tree,
            Some(parent),
        )?
        .detach();
    let commit = CommitId::parse(commit.to_hex().to_string())?;

    let mut workspace = Workspace::discover(&dir.0)?;
    workspace.set_limits(LimitsConfig {
        retained_paths: 32,
        comparison_paths: 32,
        ..LimitsConfig::default()
    });
    let IndexManifestCapture::Available(manifest) = workspace.index_manifest()? else {
        return Err("index manifest unexpectedly unavailable".into());
    };
    let error = workspace
        .index_manifest_matches_commit(&manifest, &commit)
        .err()
        .ok_or("shared tree traversal unexpectedly completed")?;
    assert!(
        error
            .to_string()
            .contains("traversal limited by item budget")
    );
    Ok(())
}

fn commit_source_modes(root: &Path) -> Result<CommitId, Box<dyn Error>> {
    let repo = gix::open_opts(root, open_options())?;
    let parent = repo.head_id()?.detach();
    let regular = repo.write_blob(b"regular\n")?.detach();
    let executable = repo.write_blob(b"#!/bin/sh\n")?.detach();
    let symlink = repo.write_blob(b"regular.md")?.detach();
    let nested = repo.write_blob(b"nested\n")?.detach();
    let subtree = repo
        .write_object(gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: "nested.md".into(),
                oid: nested,
            }],
        })?
        .detach();
    let mut entries = vec![
        gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Blob.into(),
            filename: "regular.md".into(),
            oid: regular,
        },
        gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::BlobExecutable.into(),
            filename: "executable.sh".into(),
            oid: executable,
        },
        gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Link.into(),
            filename: "link.md".into(),
            oid: symlink,
        },
        gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Tree.into(),
            filename: "dir".into(),
            oid: subtree,
        },
        gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Commit.into(),
            filename: "module".into(),
            oid: parent,
        },
        gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Blob.into(),
            filename: "wrong-kind.md".into(),
            oid: subtree,
        },
        gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Tree.into(),
            filename: "wrong-tree".into(),
            oid: regular,
        },
    ];
    entries.sort();
    let tree = repo.write_object(gix::objs::Tree { entries })?.detach();
    let signature = gix::actor::SignatureRef {
        name: "test".into(),
        email: "test@example.com".into(),
        time: "1 +0000",
    };
    let selected = repo
        .commit_as(
            signature,
            signature,
            "HEAD",
            "source modes",
            tree,
            Some(parent),
        )?
        .detach();
    Ok(CommitId::parse(selected.to_hex().to_string())?)
}

#[test]
fn git_overrides_do_not_redirect_fixtures_or_workspaces() -> TestResult {
    const CHILD: &str = "FATHOMABLE_TEST_GIT_OVERRIDES";
    if std::env::var_os(CHILD).is_none() {
        let dir = TempDir::new("git-overrides")?;
        let invalid = dir.0.join("not-a-repository");
        let index = dir.0.join("foreign-index");
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "git_overrides_do_not_redirect_fixtures_or_workspaces",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("GIT_DIR", &invalid)
            .env("GIT_WORK_TREE", &invalid)
            .env("GIT_COMMON_DIR", &invalid)
            .env("GIT_OBJECT_DIRECTORY", &invalid)
            .env("GIT_INDEX_FILE", &index)
            .output()?;
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!invalid.exists());
        assert!(!index.exists());
        return Ok(());
    }

    let dir = TempDir::new("git-overrides-child")?;
    init(&dir.0)?;
    let files = [("a.md", "own repository\n")];
    commit(&dir.0, &files)?;
    stage(&dir.0, &files)?;
    fs::write(dir.0.join("a.md"), files[0].1)?;
    let mut workspace = Workspace::discover(&dir.0)?;
    assert!(workspace.is_git());
    assert_eq!(
        workspace.head_text(Path::new("a.md"))?.as_deref(),
        Some(files[0].1)
    );
    assert!(workspace.status()?.is_empty());
    Ok(())
}

#[test]
fn plain_directory_has_no_diff_base() -> TestResult {
    let dir = TempDir::new("git-plain")?;
    fs::write(dir.0.join("a.md"), "x\n")?;
    let workspace = Workspace::discover(&dir.0)?;
    assert!(!workspace.is_git());
    assert_eq!(workspace.head_text(Path::new("a.md"))?, None);
    assert_eq!(workspace.head_text_bounded(Path::new("a.md"), 1)?, None);
    Ok(())
}

#[test]
fn commit_source_exact_lookup_does_not_peel_objects() -> TestResult {
    let dir = TempDir::new("git-exact-commit")?;
    init(&dir.0)?;
    commit(&dir.0, &[("a.md", "one\n")])?;
    let repo = gix::open_opts(&dir.0, open_options())?;
    let head = repo.head_id()?.detach();
    let tag = repo
        .write_object(gix::objs::Tag {
            target: head,
            target_kind: gix::objs::Kind::Commit,
            name: "selected".into(),
            tagger: None,
            message: "selected source\n".into(),
            signature: None,
        })?
        .detach();
    let blob = repo.write_blob(b"not a commit")?.detach();
    let workspace = Workspace::discover(&dir.0)?;
    let head = CommitId::parse(head.to_hex().to_string())?;

    assert_eq!(workspace.commit(&head)?.id(), head);
    for object in [tag, blob] {
        let id = CommitId::parse(object.to_hex().to_string())?;
        let error = workspace
            .commit(&id)
            .err()
            .ok_or("non-commit object was accepted")?;
        assert!(error.to_string().contains("commit object"), "{error}");
    }
    Ok(())
}

#[test]
fn commit_source_rejects_blob_commit_ids_before_loading_their_bodies() -> TestResult {
    let dir = TempDir::new("git-selected-noncommit-header")?;
    init(&dir.0)?;
    let id = CommitId::parse("1111111111111111111111111111111111111111")?;
    // A valid zlib stream containing only "blob 4096\0", with no blob body.
    write_loose_object(
        &dir.0,
        id.as_str(),
        &[
            120, 156, 75, 202, 201, 79, 82, 48, 49, 176, 52, 99, 0, 0, 17, 107, 2, 147,
        ],
    )?;
    fs::write(dir.0.join(".git/HEAD"), format!("{id}\n"))?;
    let mut workspace = Workspace::discover(&dir.0)?;
    for result in [
        workspace.commit(&id).map(|_| ()),
        workspace.exact_head_commit().map(|_| ()),
        workspace.commit_blob(&id, Path::new("a.md"), 0).map(|_| ()),
    ] {
        let error = result.err().ok_or("blob accepted as a commit")?;
        assert!(
            error.to_string().contains("not a commit object"),
            "the header must reject this blob before its missing body is read: {error}"
        );
    }
    Ok(())
}

#[test]
fn commit_source_rejects_oversized_commit_before_loading_its_body() -> TestResult {
    let dir = TempDir::new("git-selected-oversized-commit")?;
    init(&dir.0)?;
    let id = CommitId::parse("2222222222222222222222222222222222222222")?;
    // A valid zlib stream containing only "commit 67108865\0".
    write_loose_object(
        &dir.0,
        id.as_str(),
        &[
            120, 156, 75, 206, 207, 205, 205, 44, 81, 48, 51, 55, 52, 176, 176, 48, 51, 101, 0, 0,
            44, 129, 4, 83,
        ],
    )?;
    fs::write(dir.0.join(".git/HEAD"), format!("{id}\n"))?;
    let mut workspace = Workspace::discover(&dir.0)?;
    for result in [
        workspace.commit(&id).map(|_| ()),
        workspace.exact_head_commit().map(|_| ()),
        workspace.commit_blob(&id, Path::new("a.md"), 0).map(|_| ()),
    ] {
        let error = result.err().ok_or("oversized commit was accepted")?;
        assert!(
            error
                .to_string()
                .contains("commit object exceeds the 67108864-byte metadata limit"),
            "the header must reject this commit before its missing body is read: {error}"
        );
    }
    Ok(())
}

#[test]
fn commit_source_rejects_oversized_tree_before_loading_its_body() -> TestResult {
    let dir = TempDir::new("git-selected-oversized-tree")?;
    init(&dir.0)?;
    commit(&dir.0, &[("a.md", "one\n")])?;
    let repo = gix::open_opts(&dir.0, open_options())?;
    let commit = repo.head_commit()?;
    let selected = CommitId::parse(commit.id.to_hex().to_string())?;
    let tree = commit.tree_id()?.to_hex().to_string();
    drop(commit);
    drop(repo);
    // A valid zlib stream containing only "tree 67108865\0".
    write_loose_object(
        &dir.0,
        &tree,
        &[
            120, 156, 43, 41, 74, 77, 85, 48, 51, 55, 52, 176, 176, 48, 51, 101, 0, 0, 31, 156, 3,
            122,
        ],
    )?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let error = workspace
        .commit_blob(&selected, Path::new("a.md"), 64)
        .err()
        .ok_or("oversized tree was accepted")?;
    assert!(
        error
            .to_string()
            .contains("commit tree exceeds the 67108864-byte metadata limit"),
        "the header must reject this tree before its missing body is read: {error}"
    );
    Ok(())
}

#[test]
fn commit_source_blob_loader_enforces_modes_kind_and_bound() -> TestResult {
    let dir = TempDir::new("git-commit-blob")?;
    init(&dir.0)?;
    commit(&dir.0, &[("parent.md", "parent\n")])?;
    let selected = commit_source_modes(&dir.0)?;
    let mut workspace = Workspace::discover(&dir.0)?;

    let blob = workspace
        .commit_blob(&selected, Path::new("regular.md"), 8)?
        .ok_or("regular blob is missing")?;
    assert_eq!(blob.mode(), FileMode::Regular);
    assert_eq!(blob.size(), 8);
    assert_eq!(blob.object().len(), 40);
    assert_eq!(blob.bytes(), b"regular\n");
    assert_eq!(
        workspace
            .commit_blob(&selected, Path::new("executable.sh"), 64)?
            .map(|blob| blob.mode()),
        Some(FileMode::Executable)
    );
    assert!(
        workspace
            .commit_blob(&selected, Path::new("missing.md"), 64)?
            .is_none()
    );

    let oversized = workspace
        .commit_blob(&selected, Path::new("regular.md"), 7)
        .err()
        .ok_or("oversized blob was accepted")?;
    assert!(oversized.to_string().contains("7-byte limit"));
    for (path, mode) in [
        ("link.md", "Symlink"),
        ("dir", "Directory"),
        ("module", "Submodule"),
    ] {
        let error = workspace
            .commit_blob(&selected, Path::new(path), 64)
            .err()
            .ok_or("unsupported mode was accepted")?;
        assert!(error.to_string().contains(mode), "{path}: {error}");
    }
    let wrong_kind = workspace
        .commit_blob(&selected, Path::new("wrong-kind.md"), 64)
        .err()
        .ok_or("tree object was accepted as a blob")?;
    assert!(wrong_kind.to_string().contains("non-blob"), "{wrong_kind}");
    assert!(
        workspace
            .commit_blob(&selected, Path::new("link.md/nested.md"), 64)
            .is_err(),
        "an intermediate symlink must not be traversed"
    );
    Ok(())
}

#[test]
fn commit_source_walk_validates_each_intermediate_entry() -> TestResult {
    let dir = TempDir::new("git-commit-intermediate")?;
    init(&dir.0)?;
    commit(&dir.0, &[("parent.md", "parent\n")])?;
    let selected = commit_source_modes(&dir.0)?;
    let mut workspace = Workspace::discover(&dir.0)?;
    assert_eq!(
        workspace
            .commit_blob(&selected, Path::new("dir/nested.md"), 7)?
            .ok_or("nested blob is missing")?
            .bytes(),
        b"nested\n"
    );
    for (path, reason) in [
        ("wrong-kind.md/nested.md", "Regular intermediate"),
        ("regular.md/child", "Regular intermediate"),
        ("executable.sh/child", "Executable intermediate"),
        ("link.md/child", "Symlink intermediate"),
        ("module/child", "Submodule intermediate"),
        ("wrong-tree/child", "non-tree object"),
    ] {
        let error = workspace
            .commit_blob(&selected, Path::new(path), 0)
            .err()
            .ok_or("invalid intermediate entry was traversed")?;
        assert!(error.to_string().contains(reason), "{path}: {error}");
    }
    assert!(
        workspace
            .commit_blob(&selected, Path::new("absent/child"), 0)?
            .is_none()
    );
    Ok(())
}

#[test]
fn commit_source_rejects_intermediate_mode_before_reading_object() -> TestResult {
    let dir = TempDir::new("git-commit-intermediate-missing")?;
    init(&dir.0)?;
    commit(&dir.0, &[("regular.md", "do not load\n")])?;
    let repo = gix::open_opts(&dir.0, open_options())?;
    let head = repo.head_commit()?;
    let blob = head
        .tree()?
        .lookup_entry_by_path("regular.md")?
        .ok_or("fixture entry missing")?
        .id()
        .to_hex()
        .to_string();
    fs::remove_file(
        repo.git_dir()
            .join("objects")
            .join(&blob[..2])
            .join(&blob[2..]),
    )?;
    let selected = CommitId::parse(head.id.to_hex().to_string())?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let error = workspace
        .commit_blob(&selected, Path::new("regular.md/child"), 0)
        .err()
        .ok_or("invalid intermediate entry was traversed")?;
    assert!(
        error.to_string().contains("Regular intermediate"),
        "{error}"
    );
    Ok(())
}

#[test]
fn commit_source_rejects_non_relative_paths() -> TestResult {
    let dir = TempDir::new("git-commit-invalid-paths")?;
    init(&dir.0)?;
    commit(&dir.0, &[("a.md", "one\n")])?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let selected = CommitId::parse(workspace.head_commit().ok_or("HEAD missing")?)?;
    for path in [
        "",
        "/a.md",
        "../a.md",
        "dir/../a.md",
        "a\0.md",
        ".",
        "./a.md",
        "dir//a.md",
        "a.md/",
    ] {
        let error = workspace
            .commit_blob(&selected, Path::new(path), 64)
            .err()
            .ok_or("invalid path was accepted")?;
        assert!(
            error.to_string().contains("relative path"),
            "{path:?}: {error}"
        );
    }
    Ok(())
}

fn replace_object(repo: &gix::Repository, old: gix::ObjectId, new: gix::ObjectId) -> TestResult {
    let config = repo.git_dir().join("config");
    let mut text = fs::read_to_string(&config)?;
    // gix 0.87.1 enables replacements when this setting is false.
    text.push_str("\n[core]\nuseReplaceRefs = false\n");
    fs::write(config, text)?;
    repo.reference(
        format!("refs/replace/{old}"),
        new,
        gix::refs::transaction::PreviousValue::MustNotExist,
        "test replacement",
    )?;
    Ok(())
}

#[test]
fn commit_source_ignores_commit_tree_and_blob_replacements() -> TestResult {
    for replaced_kind in ["commit", "tree", "subtree", "blob"] {
        let dir = TempDir::new("git-commit-replacements")?;
        init(&dir.0)?;
        commit(&dir.0, &[("dir/nested.md", "original\n")])?;
        let repo = gix::open_opts(&dir.0, open_options())?;
        let original = repo.head_commit()?;
        let tree = original.tree()?;
        let subtree = tree
            .lookup_entry_by_path("dir")?
            .ok_or("fixture subtree missing")?
            .id()
            .detach();
        let blob = tree
            .lookup_entry_by_path("dir/nested.md")?
            .ok_or("fixture blob missing")?
            .id()
            .detach();
        let replacement_id = commit_source_modes(&dir.0)?;
        let replacement =
            repo.find_commit(gix::ObjectId::from_hex(replacement_id.as_str().as_bytes())?)?;
        let replacement_tree = write_tree(&repo, &[("dir/nested.md", "replacement\n")])?;
        let replacement_subtree = write_tree(&repo, &[("nested.md", "replacement\n")])?;
        let replacement_blob = repo.write_blob(b"replacement\n")?.detach();
        let (old, new) = match replaced_kind {
            "commit" => (original.id, replacement.id),
            "tree" => (tree.id, replacement_tree),
            "subtree" => (subtree, replacement_subtree),
            _ => (blob, replacement_blob),
        };
        replace_object(&repo, old, new)?;
        let selected = CommitId::parse(original.id.to_hex().to_string())?;
        fs::write(repo.git_dir().join("HEAD"), format!("{}\n", original.id))?;
        let mut workspace = Workspace::discover(&dir.0)?;
        let exact = workspace.commit(&selected)?;
        assert_eq!(exact.id(), selected);
        assert_eq!(exact.subject(), "commit", "{replaced_kind}");
        assert_eq!(exact.time(), 0, "{replaced_kind}");
        assert_eq!(exact.parents().count(), 0, "{replaced_kind}");
        assert_eq!(workspace.exact_head_commit()?, exact, "{replaced_kind}");
        let source = workspace
            .commit_blob(&selected, Path::new("dir/nested.md"), 9)?
            .ok_or("original blob missing")?;
        assert_eq!(source.bytes(), b"original\n", "{replaced_kind}");
        assert_eq!(
            source.object(),
            blob.to_hex().to_string(),
            "{replaced_kind}"
        );
        assert_eq!(source.size(), 9, "{replaced_kind}");
        if replaced_kind == "commit" {
            assert_eq!(
                workspace.resolve_revision(selected.as_str())?.subject(),
                "source modes",
                "exact reads must not alter the viewer's replacement behavior"
            );
        }
        let hex = old.to_hex().to_string();
        fs::remove_file(
            repo.git_dir()
                .join("objects")
                .join(&hex[..2])
                .join(&hex[2..]),
        )?;
        assert!(
            workspace
                .commit_blob(&selected, Path::new("dir/nested.md"), 64)
                .is_err(),
            "a missing original {replaced_kind} must not use its replacement"
        );
        if replaced_kind == "commit" {
            assert!(
                workspace.commit(&selected).is_err(),
                "a missing original commit must not use its replacement"
            );
            assert!(
                workspace.exact_head_commit().is_err(),
                "HEAD must not use a replacement for its missing original commit"
            );
        }
    }
    Ok(())
}

#[test]
fn commit_source_original_id_wins_over_a_shadow_ref() -> TestResult {
    let dir = TempDir::new("git-commit-shadow-reference")?;
    init(&dir.0)?;
    commit(&dir.0, &[("a.md", "original\n")])?;
    let repo = gix::open_opts(&dir.0, open_options())?;
    let selected = CommitId::parse(repo.head_id()?.to_hex().to_string())?;
    let shadow = commit_source_modes(&dir.0)?;
    repo.reference(
        format!("refs/heads/{selected}"),
        gix::ObjectId::from_hex(shadow.as_str().as_bytes())?,
        gix::refs::transaction::PreviousValue::MustNotExist,
        "shadow full object ID",
    )?;
    let mut workspace = Workspace::discover(&dir.0)?;
    assert_eq!(workspace.resolve_revision(selected.as_str())?.id(), shadow);
    assert_eq!(workspace.commit(&selected)?.id(), selected);
    assert_eq!(
        workspace
            .commit_blob(&selected, Path::new("a.md"), 64)?
            .ok_or("original blob missing")?
            .bytes(),
        b"original\n"
    );
    Ok(())
}

#[test]
fn commit_source_exact_head_follows_only_symbolic_references() -> TestResult {
    let dir = TempDir::new("git-exact-head")?;
    let workspace = Workspace::discover(&dir.0)?;
    assert!(
        workspace.exact_head_commit().is_err(),
        "a plain directory has no HEAD commit"
    );
    init(&dir.0)?;
    let workspace = Workspace::discover(&dir.0)?;
    assert!(
        workspace.exact_head_commit().is_err(),
        "an unborn HEAD has no commit"
    );
    commit(&dir.0, &[("a.md", "original\n")])?;
    let repo = gix::open_opts(&dir.0, open_options())?;
    let original = repo.head_id()?.detach();
    let selected = CommitId::parse(original.to_hex().to_string())?;
    let workspace = Workspace::discover(&dir.0)?;
    assert_eq!(workspace.exact_head_commit()?.id(), selected);
    fs::write(
        repo.git_dir().join("refs/heads/indirect"),
        fs::read(repo.git_dir().join("HEAD"))?,
    )?;
    fs::write(repo.git_dir().join("HEAD"), "ref: refs/heads/indirect\n")?;
    let workspace = Workspace::discover(&dir.0)?;
    assert_eq!(workspace.exact_head_commit()?.id(), selected);

    let tag = repo
        .write_object(gix::objs::Tag {
            target: original,
            target_kind: gix::objs::Kind::Commit,
            name: "redirect".into(),
            tagger: None,
            message: "must not peel\n".into(),
            signature: None,
        })?
        .detach();
    let blob = repo.write_blob(b"not a commit")?.detach();
    let missing = gix::ObjectId::from_hex(b"0123456789012345678901234567890123456789")?;
    for object in [blob, missing] {
        replace_object(&repo, object, tag)?;
    }
    for object in [tag, blob, missing] {
        fs::write(repo.git_dir().join("HEAD"), format!("{object}\n"))?;
        let workspace = Workspace::discover(&dir.0)?;
        assert!(
            workspace.exact_head_commit().is_err(),
            "{object} must not peel through a tag or replacement"
        );
    }
    Ok(())
}

#[test]
fn commit_source_reads_the_shared_store_from_a_linked_worktree() -> TestResult {
    let dir = TempDir::new("git-selected-linked-worktree")?;
    let main = dir.0.join("main");
    let linked = dir.0.join("linked");
    fs::create_dir(&main)?;
    init(&main)?;
    commit(&main, &[("a.md", "shared\n")])?;
    fathomable_testing::git::worktree_add(&main, &linked, "feature")?;

    let mut workspace = Workspace::discover(&linked)?;
    let selected = workspace.exact_head_commit()?.id();
    assert_eq!(
        workspace
            .commit_blob(&selected, Path::new("a.md"), 7)?
            .ok_or("linked-worktree blob is missing")?
            .bytes(),
        b"shared\n"
    );
    Ok(())
}

#[test]
fn commit_source_requires_original_commit_despite_replacement_or_shadow_ref() -> TestResult {
    let dir = TempDir::new("git-commit-original-required")?;
    init(&dir.0)?;
    commit(&dir.0, &[("a.md", "one\n")])?;
    let repo = gix::open_opts(&dir.0, open_options())?;
    let original = repo.head_id()?.detach();
    let blob = repo.write_blob(b"not a commit")?.detach();
    let missing = gix::ObjectId::from_hex(b"0123456789012345678901234567890123456789")?;
    for object in [blob, missing] {
        replace_object(&repo, object, original)?;
        repo.reference(
            format!("refs/heads/{object}"),
            original,
            gix::refs::transaction::PreviousValue::MustNotExist,
            "shadow object ID",
        )?;
    }
    let mut workspace = Workspace::discover(&dir.0)?;
    for object in [blob, missing] {
        let id = CommitId::parse(object.to_hex().to_string())?;
        assert!(workspace.commit(&id).is_err(), "{id}");
        assert!(
            workspace.commit_blob(&id, Path::new("a.md"), 64).is_err(),
            "{id}"
        );
    }
    Ok(())
}

#[test]
fn worktree_watch_paths_are_existing_directories() -> TestResult {
    let dir = TempDir::new("git-worktree-watch-paths")?;
    let main = dir.0.join("main");
    fs::create_dir(&main)?;
    init(&main)?;
    commit(&main, &[("a.md", "one\n")])?;
    let workspace = Workspace::discover(&main)?;
    let common = workspace.key().to_path_buf();
    let registry = common.join("worktrees");

    assert!(!registry.exists(), "one worktree needs no registry");
    let paths = workspace.worktree_watch_paths();
    assert!(paths.contains(&common));
    assert!(paths.contains(&common.join("refs")));
    assert!(!paths.contains(&registry));
    assert!(paths.iter().all(|path| path.is_dir()), "{paths:?}");

    let linked = dir.0.join("linked");
    fathomable_testing::git::worktree_add(&main, &linked, "feature")?;
    let paths = workspace.worktree_watch_paths();
    assert!(paths.contains(&registry));
    assert!(!paths.contains(&registry.join("linked")));
    assert!(
        paths.len() <= 4,
        "registry children are discovered by the bounded worker"
    );
    assert!(paths.iter().all(|path| path.is_dir()), "{paths:?}");
    let linked_workspace = Workspace::discover(&linked)?;
    let linked_paths = linked_workspace.worktree_watch_paths();
    assert_eq!(linked_paths.first(), Some(&registry.join("linked")));
    for i in 0..40 {
        fs::create_dir(registry.join(format!("extra-{i}")))?;
    }
    assert_eq!(workspace.worktree_watch_paths(), paths);
    Ok(())
}

#[test]
fn unborn_head_and_untracked_files_have_an_empty_base() -> TestResult {
    let dir = TempDir::new("git-unborn")?;
    init(&dir.0)?;
    fs::write(dir.0.join("a.md"), "x\n")?;
    let workspace = Workspace::discover(&dir.0)?;
    assert!(workspace.is_git());
    assert_eq!(
        workspace.head_text(Path::new("a.md"))?,
        Some(String::new()),
        "unborn HEAD"
    );
    assert_eq!(
        workspace.head_text_bounded(Path::new("a.md"), 0)?,
        Some(String::new()),
        "an unborn HEAD needs no blob allocation"
    );
    commit(&dir.0, &[("other.md", "o\n")])?;
    let workspace = Workspace::discover(&dir.0)?;
    assert_eq!(
        workspace.head_text(Path::new("a.md"))?,
        Some(String::new()),
        "not in HEAD"
    );
    assert_eq!(
        workspace.head_text_bounded(Path::new("other.md"), 1)?,
        None,
        "an over-limit blob is unavailable without loading it"
    );
    assert_eq!(
        workspace.head_text_bounded(Path::new("other.md"), 2)?,
        Some("o\n".to_owned())
    );
    Ok(())
}

#[test]
fn working_tree_comparison_prunes_ignored_output_but_keeps_tracked_files() -> TestResult {
    use fathomable_core::workspace::{CommitId, ComparisonEndpoint};

    let dir = TempDir::new("git-comparison-ignored-output")?;
    init(&dir.0)?;
    let committed = [
        (".gitignore", "build/\n"),
        ("build/tracked.md", "tracked\n"),
    ];
    commit(&dir.0, &committed)?;
    stage(&dir.0, &committed)?;
    fs::create_dir_all(dir.0.join("build/deep"))?;
    for (path, content) in committed {
        fs::write(dir.0.join(path), content)?;
    }
    fs::write(dir.0.join("build/deep/untracked.md"), "ignored\n")?;
    #[cfg(unix)]
    fs::write(
        dir.0
            .join("build/deep")
            .join(OsString::from_vec(vec![0xff])),
        "ignored\n",
    )?;
    fs::write(dir.0.join("build/tracked.md"), "changed\n")?;

    let mut workspace = Workspace::discover(&dir.0)?;
    let head = workspace
        .head_commit()
        .ok_or("comparison fixture has no HEAD")?;
    let comparison = workspace.compare(
        ComparisonEndpoint::Commit(CommitId::parse(&head)?),
        ComparisonEndpoint::WorkingTree,
    )?;
    let paths: Vec<_> = comparison
        .changes()
        .iter()
        .map(fathomable_core::diff::PathChange::path)
        .collect();

    assert_eq!(paths, [Path::new("build/tracked.md")]);
    Ok(())
}

#[test]
fn head_text_is_the_committed_content() -> TestResult {
    let dir = TempDir::new("git-head")?;
    init(&dir.0)?;
    fs::create_dir_all(dir.0.join("docs"))?;
    // Nested paths go through a subtree; build it explicitly.
    let repo = gix::open_opts(&dir.0, open_options())?;
    let readme = repo.write_blob(b"# One\n")?.detach();
    let guide = repo.write_blob(b"# Guide\n\nold\n")?.detach();
    let docs = repo
        .write_object(gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: "guide.md".into(),
                oid: guide,
            }],
        })?
        .detach();
    let mut entries = vec![
        gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Blob.into(),
            filename: "README.md".into(),
            oid: readme,
        },
        gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Tree.into(),
            filename: "docs".into(),
            oid: docs,
        },
    ];
    entries.sort();
    let tree = repo.write_object(gix::objs::Tree { entries })?.detach();
    let signature = gix::actor::SignatureRef {
        name: "test".into(),
        email: "test@example.com".into(),
        time: "0 +0000",
    };
    let parent = repo.head_id().ok().map(gix::Id::detach);
    repo.commit_as(signature, signature, "HEAD", "commit", tree, parent)?;
    drop(repo);

    // The working tree differs; HEAD is what the base reports.
    fs::write(dir.0.join("README.md"), "# One\n\nchanged\n")?;
    fs::write(dir.0.join("docs/guide.md"), "# Guide\n\nnew\n")?;
    let workspace = Workspace::discover(dir.0.join("docs"))?;
    assert_eq!(
        workspace.head_text(Path::new("README.md"))?.as_deref(),
        Some("# One\n")
    );
    assert_eq!(
        workspace.head_text(Path::new("docs/guide.md"))?.as_deref(),
        Some("# Guide\n\nold\n")
    );
    assert!(
        workspace.head_text(Path::new("docs")).is_err(),
        "a tree is not a diff base"
    );
    Ok(())
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario covers every index/worktree status combination"
)]
fn status_tells_staged_unstaged_and_untracked_apart() -> TestResult {
    use fathomable_core::status::{Changes, State};

    let dir = TempDir::new("git-status")?;
    init(&dir.0)?;
    let mut workspace = Workspace::discover(&dir.0)?;
    assert!(workspace.status()?.is_empty(), "empty repo is clean");

    let head = [
        ("clean.md", "c\n"),
        ("edited.md", "one\ntwo\n"),
        ("gone.md", "g\n"),
        ("mixed.md", "m1\n"),
        ("staged.md", "s\n"),
    ];
    commit(&dir.0, &head)?;
    // The index holds HEAD plus a staged edit and a staged new file.
    stage(
        &dir.0,
        &[
            ("clean.md", "c\n"),
            ("edited.md", "one\ntwo\n"),
            ("gone.md", "g\n"),
            ("mixed.md", "m2\n"),
            ("staged.md", "s2\n"),
            ("new.md", "n\n"),
            ("added-deleted.md", "temporary\n"),
        ],
    )?;
    for (name, content) in [
        ("clean.md", "c\n"),
        ("edited.md", "one\nthree\nfour\n"),
        ("mixed.md", "m3\n"),
        ("staged.md", "s2\n"),
        ("new.md", "n\n"),
        ("untracked.md", "u\nu\n"),
        (".gitignore", "ignored.md\n"),
        ("ignored.md", "i\n"),
    ] {
        fs::write(dir.0.join(name), content).map_err(|e| format!("write {name}: {e}"))?;
    }

    let mut workspace = Workspace::discover(&dir.0)?;
    let status = workspace.status().map_err(|e| format!("status: {e}"))?;
    let describe: Vec<(String, Changes, usize, usize)> = status
        .entries()
        .iter()
        .map(|e| {
            (
                e.path().display().to_string(),
                e.changes(),
                e.added(),
                e.removed(),
            )
        })
        .collect();
    assert_eq!(
        describe,
        vec![
            (
                ".gitignore".to_owned(),
                Changes::Unstaged(State::Untracked),
                1,
                0,
            ),
            (
                "added-deleted.md".to_owned(),
                Changes::Both {
                    staged: State::Added,
                    unstaged: State::Deleted,
                },
                0,
                0,
            ),
            (
                "edited.md".to_owned(),
                Changes::Unstaged(State::Modified),
                2,
                1,
            ),
            (
                "gone.md".to_owned(),
                Changes::Unstaged(State::Deleted),
                0,
                1,
            ),
            (
                "mixed.md".to_owned(),
                Changes::Both {
                    staged: State::Modified,
                    unstaged: State::Modified,
                },
                1,
                1,
            ),
            ("new.md".to_owned(), Changes::Staged(State::Added), 1, 0,),
            (
                "staged.md".to_owned(),
                Changes::Staged(State::Modified),
                1,
                1,
            ),
            (
                "untracked.md".to_owned(),
                Changes::Unstaged(State::Untracked),
                2,
                0,
            ),
        ]
    );
    assert_eq!(
        workspace.index_text(Path::new("staged.md"))?.as_deref(),
        Some("s2\n")
    );
    assert_eq!(
        workspace.index_text(Path::new("untracked.md"))?.as_deref(),
        Some("")
    );
    assert_eq!(
        workspace.head_text(Path::new("staged.md"))?.as_deref(),
        Some("s\n")
    );

    // Removing the index entry for a HEAD file is a staged deletion.
    stage(
        &dir.0,
        &[
            ("clean.md", "c\n"),
            ("edited.md", "one\ntwo\n"),
            ("mixed.md", "m2\n"),
            ("staged.md", "s2\n"),
        ],
    )?;
    let status = workspace.status().map_err(|e| format!("status: {e}"))?;
    assert_eq!(
        status
            .get(Path::new("gone.md"))
            .map(fathomable_core::status::Entry::changes),
        Some(Changes::Staged(State::Deleted))
    );
    Ok(())
}

#[test]
fn an_edited_root_ignore_file_takes_effect_on_reload() -> TestResult {
    use fathomable_core::workspace::{Filter, is_rules_file};

    let dir = TempDir::new("git-reload-rules")?;
    init(&dir.0)?;
    commit(&dir.0, &[("kept.md", "k\n")])?;
    stage(&dir.0, &[("kept.md", "k\n")])?;
    fs::write(dir.0.join("kept.md"), "k\n")?;
    fs::write(dir.0.join("scratch.md"), "s\n")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let dirty = |workspace: &mut Workspace| -> Result<Vec<String>, Box<dyn Error>> {
        Ok(workspace
            .status()?
            .entries()
            .iter()
            .map(|e| e.path().display().to_string())
            .collect())
    };
    assert_eq!(dirty(&mut workspace)?, vec!["scratch.md"]);

    // The root's rules were read when the workspace opened; the reload
    // is what makes the new pattern count, for the listing and the set.
    fs::write(dir.0.join(".gitignore"), "scratch.md\n")?;
    assert!(is_rules_file(Path::new(".gitignore")));
    assert!(is_rules_file(Path::new("docs/.gitattributes")));
    assert!(is_rules_file(Path::new(".git/info/exclude")));
    assert!(!is_rules_file(Path::new("docs/ignore.md")));
    workspace.reload_rules()?;
    assert_eq!(dirty(&mut workspace)?, vec![".gitignore"]);
    assert!(
        !workspace
            .walk_files(Filter::Visible)
            .iter()
            .any(|f| f == "scratch.md"),
        "the listing hides it too"
    );

    // Un-ignoring it brings it back the same way.
    fs::write(dir.0.join(".gitignore"), "")?;
    workspace.reload_rules()?;
    assert_eq!(dirty(&mut workspace)?, vec![".gitignore", "scratch.md"]);
    Ok(())
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one walk through every kind of change"
)]
fn status_after_examines_only_the_named_paths() -> TestResult {
    use std::path::PathBuf;

    use fathomable_core::status::{State, Status};

    let dir = TempDir::new("git-status-after")?;
    init(&dir.0)?;
    let tree = [
        ("a.md", "a\n"),
        ("dir/b.md", "b\n"),
        ("dir/c.md", "c\n"),
        ("gone.md", "g\n"),
        (".gitignore", "build/\n"),
    ];
    commit(&dir.0, &tree)?;
    stage(&dir.0, &tree)?;
    for (name, content) in tree {
        let path = dir.0.join(name);
        fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        fs::write(path, content)?;
    }
    fs::write(dir.0.join("untracked.md"), "u\n")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let paths = |names: &[&str]| -> Vec<PathBuf> { names.iter().map(PathBuf::from).collect() };
    let describe = |status: &Status| -> Vec<(String, State, bool)> {
        status
            .entries()
            .iter()
            .map(|e| (e.path().display().to_string(), e.state(), e.is_staged()))
            .collect()
    };
    let mut current = workspace.status()?;
    assert_eq!(
        describe(&current),
        vec![("untracked.md".to_owned(), State::Untracked, false)]
    );

    // Only the named path is examined: the other edit waits for its
    // own event, as the full walk would find it.
    fs::write(dir.0.join("a.md"), "a\nmore\n")?;
    fs::write(dir.0.join("late.md"), "l\n")?;
    current = workspace.status_after(&current, &paths(&["a.md"]))?;
    assert_eq!(
        describe(&current),
        vec![
            ("a.md".to_owned(), State::Modified, false),
            ("untracked.md".to_owned(), State::Untracked, false),
        ]
    );
    assert_eq!(
        current
            .get(Path::new("a.md"))
            .map(|e| (e.added(), e.removed())),
        Some((1, 0)),
        "line counts come with the entry"
    );
    current = workspace.status_after(&current, &paths(&["late.md"]))?;
    assert_eq!(current, workspace.status()?);

    // An event on a directory covers its tracked files, its dirty
    // entries, and what it holds on disk: removed whole, created whole.
    fs::remove_dir_all(dir.0.join("dir"))?;
    current = workspace.status_after(&current, &paths(&["dir"]))?;
    assert_eq!(current, workspace.status()?);
    assert_eq!(
        current
            .get(Path::new("dir/b.md"))
            .map(|e| (e.state(), e.is_staged())),
        Some((State::Deleted, false))
    );
    fs::create_dir(dir.0.join("new"))?;
    fs::write(dir.0.join("new/x.md"), "x\n")?;
    fs::write(dir.0.join("new/y.md"), "y\n")?;
    current = workspace.status_after(&current, &paths(&["new"]))?;
    assert_eq!(current, workspace.status()?);
    assert!(current.contains(Path::new("new/y.md")));

    // Ignored output never joins; an edit undone leaves.
    fs::create_dir(dir.0.join("build"))?;
    fs::write(dir.0.join("build/out.o"), "o\n")?;
    fs::write(dir.0.join("a.md"), "a\n")?;
    current = workspace.status_after(&current, &paths(&["build/out.o", "a.md"]))?;
    assert_eq!(current, workspace.status()?);
    assert!(!current.contains(Path::new("a.md")));
    assert!(!current.contains(Path::new("build/out.o")));

    // A staged deletion and a recreated worktree file remain distinct
    // layers, and the staged deletion remains when it goes again.
    fs::remove_file(dir.0.join("gone.md"))?;
    stage(&dir.0, &tree[..3])?;
    current = workspace.status()?;
    assert_eq!(
        current
            .get(Path::new("gone.md"))
            .map(|e| (e.state(), e.is_staged())),
        Some((State::Deleted, true))
    );
    fs::write(dir.0.join("gone.md"), "g\n")?;
    let untracked_again = workspace.status_after(&current, &paths(&["gone.md"]))?;
    assert_eq!(untracked_again, workspace.status()?);
    assert_eq!(
        untracked_again
            .get(Path::new("gone.md"))
            .map(fathomable_core::status::Entry::changes),
        Some(fathomable_core::status::Changes::Both {
            staged: State::Deleted,
            unstaged: State::Untracked,
        })
    );
    fs::remove_file(dir.0.join("gone.md"))?;
    current = workspace.status_after(&untracked_again, &paths(&["gone.md"]))?;
    assert_eq!(current, workspace.status()?);
    assert_eq!(
        current
            .get(Path::new("gone.md"))
            .map(|e| (e.state(), e.is_staged())),
        Some((State::Deleted, true))
    );

    // A rules file among the paths means the whole tree is walked.
    fs::write(dir.0.join(".gitignore"), "build/\nnew/\n")?;
    fs::write(dir.0.join("unseen.md"), "s\n")?;
    workspace.reload_rules()?;
    current = workspace.status_after(&current, &paths(&[".gitignore"]))?;
    assert_eq!(current, workspace.status()?);
    assert!(!current.contains(Path::new("new/x.md")));
    assert!(
        current.contains(Path::new("unseen.md")),
        "the full walk found it"
    );
    Ok(())
}

/// Commit `files` (path, content, kind) as `HEAD` and stage the same tree.
#[cfg(unix)]
fn commit_and_stage(
    root: &Path,
    files: &[(&str, &str, gix::objs::tree::EntryKind)],
) -> Result<(), Box<dyn Error>> {
    let repo = gix::open_opts(root, open_options())?;
    let mut entries = Vec::new();
    for (name, content, kind) in files {
        let oid = repo.write_blob(content.as_bytes())?.detach();
        entries.push(gix::objs::tree::Entry {
            mode: (*kind).into(),
            filename: (*name).into(),
            oid,
        });
    }
    entries.sort();
    let tree = repo.write_object(gix::objs::Tree { entries })?.detach();
    let signature = gix::actor::SignatureRef {
        name: "test".into(),
        email: "test@example.com".into(),
        time: "0 +0000",
    };
    let parent = repo.head_id().ok().map(gix::Id::detach);
    repo.commit_as(signature, signature, "HEAD", "commit", tree, parent)?;
    let state = gix::index::State::from_tree(
        &tree,
        &repo.objects,
        gix::validate::path::component::Options::default(),
    )
    .map_err(|e| format!("from_tree: {e}"))?;
    let mut file = gix::index::File::from_state(state, repo.index_path());
    file.write(gix::index::write::Options::default())
        .map_err(|e| format!("index write: {e}"))?;
    Ok(())
}

#[cfg(unix)]
/// A directory event over many tracked files reads the `HEAD` subtree
/// once; the answer is the full walk's, and a change naming the root is
/// the full walk.
#[test]
fn a_directory_event_over_many_files_matches_the_walk() -> TestResult {
    use fathomable_core::status::{State, Status};

    let dir = TempDir::new("git-status-dir")?;
    init(&dir.0)?;
    let names: Vec<String> = (0..24).map(|i| format!("many/f{i:02}.md")).collect();
    let files: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "x\n")).collect();
    commit(&dir.0, &files)?;
    stage(&dir.0, &files)?;
    fs::create_dir_all(dir.0.join("many"))?;
    for name in &names {
        fs::write(dir.0.join(name), "x\n")?;
    }
    fs::write(dir.0.join("many/f03.md"), "y\n")?;
    fs::remove_file(dir.0.join("many/f07.md"))?;
    fs::write(dir.0.join("many/new.md"), "n\n")?;

    let mut workspace = Workspace::discover(&dir.0)?;
    let walked = workspace.status()?;
    let describe = |status: &Status| -> Vec<(String, State)> {
        status
            .entries()
            .iter()
            .map(|e| (e.path().display().to_string(), e.state()))
            .collect()
    };
    assert_eq!(
        describe(&walked),
        vec![
            ("many/f03.md".to_owned(), State::Modified),
            ("many/f07.md".to_owned(), State::Deleted),
            ("many/new.md".to_owned(), State::Untracked),
        ]
    );
    let after = workspace.status_after(&Status::default(), &[Path::new("many").to_path_buf()])?;
    assert_eq!(after, walked);
    let root = workspace.status_after(&Status::default(), &[Path::new("").to_path_buf()])?;
    assert_eq!(root, walked);
    Ok(())
}

/// An index entry whose mtime is not older than the index file's own is
/// "racily clean": git hashes it rather than trust the stat match, since
/// a rewrite to the same size in the same second as the `git add` would
/// otherwise hide. The stat data is planted as git would have written
/// it, and the mtimes pinned, so the race is the same on every run.
#[test]
fn racily_clean_entries_are_hashed_not_trusted() -> TestResult {
    use std::time::Duration;

    use fathomable_core::status::{State, Status};

    let dir = TempDir::new("git-racy")?;
    init(&dir.0)?;
    commit(&dir.0, &[("a.md", "hello\n")])?;
    stage(&dir.0, &[("a.md", "hello\n")])?;
    let file = dir.0.join("a.md");
    fs::write(&file, "hello\n")?;
    let repo = gix::open_opts(&dir.0, open_options()).map_err(|e| format!("open: {e}"))?;
    let mut index = repo.open_index().map_err(|e| format!("open index: {e}"))?;
    let at = index
        .entry_index_by_path("a.md".into())
        .ok()
        .ok_or("a.md is in the index")?;
    index.entries_mut()[at].stat =
        gix::index::entry::Stat::from_fs(&gix::index::fs::Metadata::from_path_no_follow(&file)?)?;
    index
        .write(gix::index::write::Options::default())
        .map_err(|e| format!("write index: {e}"))?;
    let written = fs::metadata(&file)
        .map_err(|e| format!("stat a.md: {e}"))?
        .modified()?;
    let index_path = repo.index_path();
    let pin = |path: &Path, at: std::time::SystemTime| -> TestResult {
        fs::File::options()
            .write(true)
            .open(path)?
            .set_modified(at)?;
        Ok(())
    };

    // Same size, same mtime, index no newer than the file: hashed.
    fs::write(&file, "jello\n")?;
    pin(&file, written)?;
    pin(&index_path, written)?;
    let dirty = |status: Status| -> Vec<(String, State)> {
        status
            .entries()
            .iter()
            .map(|e| (e.path().display().to_string(), e.state()))
            .collect()
    };
    let modified = vec![("a.md".to_owned(), State::Modified)];
    assert_eq!(dirty(Workspace::discover(&dir.0)?.status()?), modified);
    assert_eq!(
        dirty(Workspace::discover(&dir.0)?.status_after(
            &Status::default(),
            &[file.strip_prefix(&dir.0)?.to_path_buf()]
        )?),
        modified
    );

    // An index written well after the file: the stat match is trusted
    // and the file is not read, as git does.
    pin(&index_path, written + Duration::from_secs(2))?;
    assert!(Workspace::discover(&dir.0)?.status()?.is_empty());
    Ok(())
}

#[test]
fn symlinks_diff_by_target_path_not_followed_content() -> TestResult {
    use fathomable_core::status::State;
    use gix::objs::tree::EntryKind;

    let dir = TempDir::new("git-symlink")?;
    init(&dir.0)?;
    fs::write(dir.0.join("a.md"), "one\ntwo\n")?;
    fs::write(dir.0.join("b.md"), "b\n")?;
    std::os::unix::fs::symlink("a.md", dir.0.join("link.md"))?;
    commit_and_stage(
        &dir.0,
        &[
            ("a.md", "one\ntwo\n", EntryKind::Blob),
            ("b.md", "b\n", EntryKind::Blob),
            ("link.md", "a.md", EntryKind::Link),
        ],
    )?;

    let mut workspace = Workspace::discover(&dir.0)?;
    assert!(
        workspace.status()?.is_empty(),
        "an unchanged committed symlink is clean"
    );
    assert_eq!(
        workspace.head_text(Path::new("link.md"))?.as_deref(),
        Some("a.md"),
        "a symlink's diff base is its committed target path"
    );

    // An untracked symlink counts its one-line target, not the file it
    // points at.
    std::os::unix::fs::symlink("a.md", dir.0.join("new-link.md"))?;
    let status = workspace.status()?;
    let entry = status
        .get(Path::new("new-link.md"))
        .ok_or("new-link.md missing")?;
    assert_eq!(
        (entry.state(), entry.added(), entry.removed()),
        (State::Untracked, 1, 0)
    );
    fs::remove_file(dir.0.join("new-link.md"))?;

    // Retargeting the committed symlink is an unstaged modification.
    fs::remove_file(dir.0.join("link.md"))?;
    std::os::unix::fs::symlink("b.md", dir.0.join("link.md"))?;
    let status = workspace.status()?;
    let entry = status.get(Path::new("link.md")).ok_or("link.md missing")?;
    assert_eq!(
        (
            entry.state(),
            entry.is_staged(),
            entry.added(),
            entry.removed()
        ),
        (State::Modified, false, 1, 1)
    );
    assert_eq!(status.len(), 1);
    Ok(())
}

#[cfg(unix)]
#[test]
fn directory_symlinks_browse_as_dirs_but_status_never_descends() -> TestResult {
    use fathomable_core::status::State;
    use gix::objs::tree::EntryKind;

    let dir = TempDir::new("git-dirlink")?;
    init(&dir.0)?;
    fs::create_dir(dir.0.join("real"))?;
    fs::write(dir.0.join("real/inner.md"), "i\n")?;
    std::os::unix::fs::symlink("real", dir.0.join("linkdir"))?;
    // A cycle back to the root must not hang the walk.
    std::os::unix::fs::symlink(".", dir.0.join("loop"))?;
    commit_and_stage(&dir.0, &[("linkdir", "real", EntryKind::Link)])?;

    let mut workspace = Workspace::discover(&dir.0)?;
    let listed: Vec<(String, bool, bool)> = workspace
        .list_dir("")?
        .iter()
        .map(|entry| (entry.name().to_owned(), entry.is_dir(), entry.is_symlink()))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("linkdir".to_owned(), true, true),
            ("loop".to_owned(), true, true),
            ("real".to_owned(), true, false),
        ],
        "a symlink to a directory stays browsable and is flagged as a link"
    );

    let status = workspace.status()?;
    let describe: Vec<(String, State)> = status
        .entries()
        .iter()
        .map(|e| (e.path().display().to_string(), e.state()))
        .collect();
    assert_eq!(
        describe,
        vec![
            ("loop".to_owned(), State::Untracked),
            ("real/inner.md".to_owned(), State::Untracked),
        ],
        "the committed link is clean and nothing behind a link is walked"
    );
    Ok(())
}

/// Commit `files` on `reference` with `parent`, returning the new id.
fn commit_on(
    root: &Path,
    reference: &str,
    parent: Option<gix::ObjectId>,
    files: &[(&str, &str)],
) -> Result<String, Box<dyn Error>> {
    commit_on_at(root, reference, parent, files, "0 +0000")
}

/// [`commit_on`] with the committer `time` (`"<seconds> +0000"`).
fn commit_on_at(
    root: &Path,
    reference: &str,
    parent: Option<gix::ObjectId>,
    files: &[(&str, &str)],
    time: &str,
) -> Result<String, Box<dyn Error>> {
    let repo = gix::open_opts(root, open_options())?;
    let tree = write_tree(&repo, files)?;
    let signature = gix::actor::SignatureRef {
        name: "test".into(),
        email: "test@example.com".into(),
        time,
    };
    let id = repo.commit_as(signature, signature, reference, "commit", tree, parent)?;
    Ok(id.to_hex().to_string())
}

/// `reachable` answers for `HEAD` and its ancestors only, so a thread
/// written on another branch is not on this work (ADR 0024).
#[test]
fn reachable_commits_are_head_and_its_ancestors() -> TestResult {
    let dir = TempDir::new("git-reachable")?;
    init(&dir.0)?;
    let plain = Workspace::discover(&dir.0)?;
    assert_eq!(plain.head_commit(), None, "unborn HEAD has no commit");
    assert_eq!(plain.reachable(["anything"]), None);

    let first = commit_on(&dir.0, "HEAD", None, &[("a.md", "1\n")])?;
    let first_id = gix::ObjectId::from_hex(first.as_bytes())?;
    let second = commit_on(&dir.0, "HEAD", Some(first_id), &[("a.md", "2\n")])?;
    // A branch off the first commit that HEAD does not contain.
    let elsewhere = commit_on(
        &dir.0,
        "refs/heads/elsewhere",
        Some(first_id),
        &[("a.md", "3\n")],
    )?;
    let workspace = Workspace::discover(&dir.0)?;
    assert_eq!(workspace.head_commit().as_deref(), Some(second.as_str()));
    // A well-formed id the object store never held, as after a `gc`.
    let gone = "0123456789abcdef0123456789abcdef01234567";
    let wanted = [
        first.as_str(),
        second.as_str(),
        elsewhere.as_str(),
        gone,
        "nope",
    ];
    let reachable = workspace.reachable(wanted).ok_or("git workspace")?;
    assert!(reachable.contains(&first));
    assert!(reachable.contains(&second));
    assert!(!reachable.contains(&elsewhere));
    assert!(!reachable.contains(gone));
    assert!(!reachable.contains("nope"));
    assert_eq!(
        workspace.reachable(std::iter::empty()).map(|set| set.len()),
        Some(0)
    );
    Ok(())
}

/// The walk is bounded by the oldest wanted commit's time, so a wanted
/// commit far older than `HEAD` is still met, and one only a rewrite
/// dropped is still missed, whatever their dates.
#[test]
fn reachable_finds_a_wanted_commit_much_older_than_head() -> TestResult {
    const MONTH: i64 = 30 * 24 * 60 * 60;
    let dir = TempDir::new("git-reachable-old")?;
    init(&dir.0)?;
    let old = commit_on_at(&dir.0, "HEAD", None, &[("a.md", "1\n")], "0 +0000")?;
    let old_id = gix::ObjectId::from_hex(old.as_bytes())?;
    let dropped = commit_on_at(
        &dir.0,
        "refs/heads/dropped",
        Some(old_id),
        &[("a.md", "2\n")],
        &format!("{MONTH} +0000"),
    )?;
    let mut parent = old_id;
    for (months, text) in (2..).zip(["3\n", "4\n", "5\n"]) {
        let id = commit_on_at(
            &dir.0,
            "HEAD",
            Some(parent),
            &[("a.md", text)],
            &format!("{} +0000", months * MONTH),
        )?;
        parent = gix::ObjectId::from_hex(id.as_bytes())?;
    }
    let workspace = Workspace::discover(&dir.0)?;
    let reachable = workspace
        .reachable([old.as_str(), dropped.as_str()])
        .ok_or("git workspace")?;
    assert!(
        reachable.contains(&old),
        "the root commit is an ancestor of HEAD"
    );
    assert!(
        !reachable.contains(&dropped),
        "a commit off HEAD's line is not"
    );
    Ok(())
}

#[test]
fn binary_files_follow_the_diff_attribute_then_the_nul_sniff() -> TestResult {
    use fathomable_core::content::Attr;
    use fathomable_core::status::State;

    let dir = TempDir::new("git-binary")?;
    init(&dir.0)?;
    let head = [
        (".gitattributes", "*.dat binary\n*.nul diff\n"),
        ("plain.dat", "text by content\n"),
        ("forced.nul", "a\0b\n"),
        ("blob.bin", "\0asm\x01\0\0\0"),
        ("notes.md", "one\n"),
    ];
    commit(&dir.0, &head)?;
    stage(&dir.0, &head)?;
    for (name, content) in head {
        fs::write(dir.0.join(name), content)?;
    }
    let mut workspace = Workspace::discover(&dir.0)?;

    // The attribute answers before any bytes are read.
    assert_eq!(workspace.diff_attr(Path::new("plain.dat")), Attr::Binary);
    assert_eq!(workspace.diff_attr(Path::new("forced.nul")), Attr::Text);
    assert_eq!(
        workspace.diff_attr(Path::new("blob.bin")),
        Attr::Unspecified
    );
    assert_eq!(
        workspace.diff_attr(Path::new("notes.md")),
        Attr::Unspecified
    );

    // HEAD sizes come from the object headers.
    assert_eq!(workspace.head_size(Path::new("blob.bin"))?, Some(8));
    assert_eq!(workspace.head_size(Path::new("missing.bin"))?, None);

    // Edit every file: the status flags the binaries and counts the rest.
    fs::write(dir.0.join("plain.dat"), "text by content, edited\n")?;
    fs::write(dir.0.join("forced.nul"), "a\0b\nc\n")?;
    fs::write(dir.0.join("blob.bin"), "\0asm\x01\0\0\0more")?;
    fs::write(dir.0.join("notes.md"), "one\ntwo\n")?;
    fs::write(dir.0.join("new.png"), b"\x89PNG\r\n\x1a\n\0\0")?;
    let status = workspace.status()?;
    let flags: Vec<(String, State, bool, usize)> = status
        .entries()
        .iter()
        .map(|e| {
            (
                e.path().display().to_string(),
                e.state(),
                e.is_binary(),
                e.added(),
            )
        })
        .collect();
    assert_eq!(
        flags,
        [
            ("blob.bin".to_owned(), State::Modified, true, 0),
            ("forced.nul".to_owned(), State::Modified, false, 1),
            ("new.png".to_owned(), State::Untracked, true, 0),
            ("notes.md".to_owned(), State::Modified, false, 1),
            ("plain.dat".to_owned(), State::Modified, true, 0),
        ]
    );
    Ok(())
}
