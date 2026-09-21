//! Resource exhaustion is explicit at workspace discovery and comparison boundaries.

use std::fs;
use std::path::Path;

use fathomable_core::config::LimitsConfig;
use fathomable_core::diff::PathChangeKind;
use fathomable_core::workspace::{Cancellation, CommitId, ComparisonEndpoint, Filter, Workspace};
use fathomable_testing::TempDir;

type Result = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn wide_discovery_stops_at_examined_entry_budget() -> Result {
    let dir = TempDir::new("discovery-entry-budget")?;
    for i in 0..40 {
        fs::write(dir.0.join(format!("{i}.txt")), "fixture")?;
    }
    let mut workspace = Workspace::discover(&dir.0)?;
    workspace.set_limits(LimitsConfig {
        discovery_entries: 7,
        ..LimitsConfig::default()
    });
    let found = workspace.discover_files(Filter::All);
    assert_eq!(found.paths().len(), 7);
    assert!(
        found
            .incomplete()
            .is_some_and(|error| error.message().contains("examined-entry"))
    );
    let error = workspace
        .compare(
            ComparisonEndpoint::EmptyTree,
            ComparisonEndpoint::WorkingTree,
        )
        .err()
        .ok_or("incomplete comparison must fail")?;
    assert!(error.message().contains("examined-entry"));
    Ok(())
}

#[test]
fn retained_paths_bound_a_wide_directory_frontier() -> Result {
    let dir = TempDir::new("discovery-frontier-budget")?;
    for i in 0..40 {
        fs::create_dir(dir.0.join(format!("{i}")))?;
    }
    let mut workspace = Workspace::discover(&dir.0)?;
    workspace.set_limits(LimitsConfig {
        retained_paths: 5,
        ..LimitsConfig::default()
    });
    let found = workspace.discover_files(Filter::All);
    assert!(
        found
            .incomplete()
            .is_some_and(|error| error.message().contains("retained-path"))
    );
    Ok(())
}

#[test]
fn cancelled_discovery_and_comparison_never_report_complete() -> Result {
    let dir = TempDir::new("discovery-cancelled")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let cancellation = Cancellation::default();
    workspace.set_cancellation(cancellation.clone());
    cancellation.cancel();
    assert!(workspace.discover_files(Filter::All).incomplete().is_some());
    let error = workspace
        .compare(
            ComparisonEndpoint::EmptyTree,
            ComparisonEndpoint::WorkingTree,
        )
        .err()
        .ok_or("cancelled comparison must fail")?;
    assert!(error.message().contains("cancelled"));
    Ok(())
}

#[test]
fn deep_walk_is_iterative_and_does_not_follow_directory_links() -> Result {
    let dir = TempDir::new("discovery-deep")?;
    let mut path = dir.0.clone();
    for _ in 0..60 {
        path.push("d");
        fs::create_dir(&path)?;
    }
    fs::write(path.join("leaf"), "fixture")?;
    std::os::unix::fs::symlink(&dir.0, dir.0.join("loop"))?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let found = workspace.discover_files(Filter::All);
    assert!(found.incomplete().is_none());
    assert_eq!(found.paths().len(), 2);
    assert!(
        found
            .paths()
            .iter()
            .any(|path| Path::new(path) == Path::new("loop"))
    );
    Ok(())
}

#[test]
fn comparison_path_budget_is_not_a_clean_empty_result() -> Result {
    let dir = TempDir::new("comparison-path-budget")?;
    for i in 0..4 {
        fs::write(dir.0.join(format!("{i}.txt")), "fixture")?;
    }
    let mut workspace = Workspace::discover(&dir.0)?;
    workspace.set_limits(LimitsConfig {
        comparison_paths: 3,
        ..LimitsConfig::default()
    });
    let error = workspace
        .compare(
            ComparisonEndpoint::EmptyTree,
            ComparisonEndpoint::WorkingTree,
        )
        .err()
        .ok_or("comparison should exceed its path budget")?;
    assert!(error.message().contains("limits.comparison-paths"));
    Ok(())
}

#[test]
fn comparison_total_content_budget_marks_unavailable_content() -> Result {
    let dir = TempDir::new("comparison-byte-budget")?;
    fs::write(dir.0.join("a"), b"1234")?;
    fs::write(dir.0.join("b"), b"5678")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    workspace.set_limits(LimitsConfig {
        comparison_bytes: 7,
        ..LimitsConfig::default()
    });
    let comparison = workspace.compare(
        ComparisonEndpoint::EmptyTree,
        ComparisonEndpoint::WorkingTree,
    )?;
    assert_eq!(comparison.len(), 2);
    assert!(
        comparison
            .changes()
            .iter()
            .any(|change| change.kind() == PathChangeKind::Missing)
    );
    Ok(())
}

#[test]
fn exact_content_budget_preserves_complete_comparison() -> Result {
    let dir = TempDir::new("comparison-exact-budget")?;
    fs::write(dir.0.join("a"), b"1234")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    workspace.set_limits(LimitsConfig {
        comparison_bytes: 4,
        ..LimitsConfig::default()
    });
    let comparison = workspace.compare(
        ComparisonEndpoint::EmptyTree,
        ComparisonEndpoint::WorkingTree,
    )?;
    assert_eq!(comparison.len(), 1);
    assert_eq!(comparison.changes()[0].kind(), PathChangeKind::Added);
    Ok(())
}

#[test]
fn immutable_comparison_does_not_read_identical_blobs() -> Result {
    let dir = TempDir::new("comparison-identical-objects")?;
    fathomable_testing::git::init(&dir.0)?;
    let unchanged = "x".repeat(1_024);
    fathomable_testing::git::commit_and_stage(
        &dir.0,
        &[("changed", "old\n"), ("unchanged", &unchanged)],
    )?;
    let base = Workspace::discover(&dir.0)?
        .head_commit()
        .ok_or("base commit missing")?;
    fathomable_testing::git::commit_and_stage(
        &dir.0,
        &[("changed", "new\n"), ("unchanged", &unchanged)],
    )?;
    let target = Workspace::discover(&dir.0)?
        .head_commit()
        .ok_or("target commit missing")?;

    let mut workspace = Workspace::discover(&dir.0)?;
    workspace.set_limits(LimitsConfig {
        comparison_bytes: 8,
        ..LimitsConfig::default()
    });
    let comparison = workspace.compare(
        ComparisonEndpoint::Commit(CommitId::parse(&base)?),
        ComparisonEndpoint::Commit(CommitId::parse(&target)?),
    )?;

    assert_eq!(
        comparison.target_paths(),
        [Path::new("changed"), Path::new("unchanged")]
    );
    assert_eq!(comparison.changes().len(), 1);
    assert_eq!(comparison.changes()[0].path(), Path::new("changed"));
    assert_eq!(
        comparison.changes()[0].kind(),
        PathChangeKind::ContentChanged
    );
    Ok(())
}

#[test]
fn recursive_expansion_keeps_a_partial_tree_within_its_path_budget() -> Result {
    let dir = TempDir::new("tree-path-budget")?;
    for i in 0..4 {
        fs::create_dir(dir.0.join(format!("dir{i}")))?;
        for j in 0..12 {
            fs::write(dir.0.join(format!("dir{i}/{j}")), b"fixture")?;
        }
    }
    let mut workspace = Workspace::discover(&dir.0)?;
    workspace.set_limits(LimitsConfig {
        retained_paths: 7,
        ..LimitsConfig::default()
    });
    let mut tree = fathomable_core::tree::Tree::new(&mut workspace)?;
    assert!(tree.toggle_all(&mut workspace).is_err());
    assert!(tree.rows().len() <= 7);
    assert!(workspace.listing_incomplete().is_some());
    Ok(())
}

#[test]
fn virtual_ancestors_cannot_bypass_the_retained_tree_budget() -> Result {
    let dir = TempDir::new("tree-virtual-budget")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    workspace.set_limits(LimitsConfig {
        retained_paths: 5,
        ..LimitsConfig::default()
    });
    let mut tree = fathomable_core::tree::Tree::new(&mut workspace)?;
    let paths = (0..20).map(|i| format!("dir{i}/sub/file").into()).collect();
    tree.set_snapshot_paths(&fathomable_core::status::Status::default(), paths);
    let _ = tree.toggle_all(&mut workspace);
    assert!(tree.rows().len() <= 5);
    assert!(tree.discovery_limited());
    Ok(())
}

#[test]
fn git_status_cannot_report_clean_after_incomplete_discovery() -> Result {
    let dir = TempDir::new("status-discovery-budget")?;
    fathomable_testing::git::init(&dir.0)?;
    fathomable_testing::git::commit_and_stage(&dir.0, &[("tracked", "base\n")])?;
    for i in 0..12 {
        fs::write(dir.0.join(format!("untracked{i}")), b"fixture")?;
    }
    let mut workspace = Workspace::discover(&dir.0)?;
    workspace.set_limits(LimitsConfig {
        discovery_entries: 4,
        ..LimitsConfig::default()
    });
    let error = workspace
        .status()
        .err()
        .ok_or("incomplete status must fail")?;
    assert!(error.message().contains("examined-entry"));
    Ok(())
}

#[test]
fn git_status_hashing_obeys_the_content_budget() -> Result {
    let dir = TempDir::new("status-content-budget")?;
    fathomable_testing::git::init(&dir.0)?;
    fathomable_testing::git::commit_and_stage(&dir.0, &[("tracked", "base\n")])?;
    fs::write(dir.0.join("tracked"), b"a much larger changed file\n")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    workspace.set_limits(LimitsConfig {
        comparison_bytes: 4,
        ..LimitsConfig::default()
    });
    let error = workspace
        .status()
        .err()
        .ok_or("oversized status content must fail")?;
    assert!(error.message().contains("limits.comparison-bytes"));
    Ok(())
}

#[test]
fn git_status_line_counts_charge_aggregate_content_reads() -> Result {
    let dir = TempDir::new("status-aggregate-content-budget")?;
    fathomable_testing::git::init(&dir.0)?;
    fs::write(dir.0.join("a"), b"1234")?;
    fs::write(dir.0.join("b"), b"5678")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    workspace.set_limits(LimitsConfig {
        comparison_bytes: 7,
        ..LimitsConfig::default()
    });
    let error = workspace
        .status()
        .err()
        .ok_or("aggregate status content must be charged")?;
    assert!(error.message().contains("limits.comparison-bytes"));
    Ok(())
}

#[test]
fn incomplete_physical_listing_survives_a_noop_until_a_complete_refresh() -> Result {
    let dir = TempDir::new("retained-physical-listing-warning")?;
    for i in 0..6 {
        fs::write(dir.0.join(format!("{i}")), "fixture")?;
    }
    let mut workspace = Workspace::discover(&dir.0)?;
    workspace.set_limits(LimitsConfig {
        retained_paths: 3,
        ..LimitsConfig::default()
    });
    let mut tree = fathomable_core::tree::Tree::new(&mut workspace)?;
    assert!(tree.listing_incomplete().is_some());
    tree.toggle_all(&mut workspace)?;
    assert!(tree.discovery_limited());
    assert!(tree.listing_incomplete().is_some());
    workspace.set_limits(LimitsConfig::default());
    tree.refresh(&mut workspace)?;
    assert!(tree.listing_incomplete().is_none());
    assert!(!tree.discovery_limited());
    assert_eq!(tree.rows().len(), 6);
    Ok(())
}
