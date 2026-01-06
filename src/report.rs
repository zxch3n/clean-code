use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use rayon::prelude::*;

use crate::{
    format::{display_rel_path, format_bytes},
    git::{GitHead, git_head, is_git_ignored},
    scan::{DirStats, dir_stats, scan_artifact_dirs},
};

#[derive(Debug, Clone)]
pub struct ArtifactRecord {
    pub repo_root: PathBuf,
    pub path: PathBuf,
    pub stats: DirStats,
}

impl ArtifactRecord {
    pub fn is_stale(&self, now: SystemTime, stale_for: Duration) -> bool {
        let Some(newest) = self.stats.newest_mtime else {
            return true;
        };

        match now.duration_since(newest) {
            Ok(age) => age >= stale_for,
            Err(_) => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RepoReport {
    pub repo_root: PathBuf,
    pub head: Option<GitHead>,
    pub artifacts: Vec<ArtifactRecord>,
    pub total_size_bytes: u64,
    pub newest_mtime: Option<SystemTime>,
}

impl RepoReport {
    pub fn stale_size_bytes(&self, now: SystemTime, stale_for: Duration) -> u64 {
        self.artifacts
            .iter()
            .filter(|a| a.is_stale(now, stale_for))
            .map(|a| a.stats.size_bytes)
            .sum()
    }
}

pub fn collect_reports(
    scan_root: &Path,
    artifact_dir_names: &HashSet<OsString>,
) -> Vec<RepoReport> {
    let candidates = scan_artifact_dirs(scan_root, artifact_dir_names);
    let records = candidates
        .par_iter()
        .filter_map(|path| process_candidate(path))
        .collect::<Vec<_>>();

    let mut by_repo: HashMap<PathBuf, Vec<ArtifactRecord>> = HashMap::new();
    for record in records {
        by_repo
            .entry(record.repo_root.clone())
            .or_default()
            .push(record);
    }

    let mut reports: Vec<RepoReport> = by_repo
        .into_iter()
        .map(|(repo_root, mut artifacts)| {
            artifacts.sort_by(|a, b| {
                b.stats
                    .size_bytes
                    .cmp(&a.stats.size_bytes)
                    .then_with(|| a.path.cmp(&b.path))
            });
            let total_size_bytes = artifacts.iter().map(|a| a.stats.size_bytes).sum::<u64>();
            let newest_mtime = artifacts.iter().filter_map(|a| a.stats.newest_mtime).max();

            let head = match git_head(&repo_root) {
                Ok(head) => head,
                Err(err) => {
                    eprintln!("warn: git head lookup failed: repo={repo_root:?} err={err:#}");
                    None
                }
            };

            RepoReport {
                repo_root,
                head,
                artifacts,
                total_size_bytes,
                newest_mtime,
            }
        })
        .collect();

    reports.sort_by(|a, b| {
        let a_ts = a.head.as_ref().map(|h| h.unix_seconds).unwrap_or(i64::MAX);
        let b_ts = b.head.as_ref().map(|h| h.unix_seconds).unwrap_or(i64::MAX);

        a_ts.cmp(&b_ts).then_with(|| a.repo_root.cmp(&b.repo_root))
    });

    reports
}

pub fn print_scan_report(scan_root: &Path, reports: &[RepoReport]) {
    let total_bytes = reports.iter().map(|r| r.total_size_bytes).sum::<u64>();

    println!("Scan root: {}", scan_root.display());
    println!(
        "Repos with gitignored artifacts: {}  Total: {}",
        reports.len(),
        format_bytes(total_bytes)
    );
    println!();

    for report in reports {
        let repo_display = display_rel_path(scan_root, &report.repo_root);
        let head_display = report
            .head
            .as_ref()
            .map(|head| {
                let short_hash = head.hash.get(0..8).unwrap_or(&head.hash);
                format!("{} {}", head.iso8601, short_hash)
            })
            .unwrap_or_else(|| "no commits".to_string());

        println!(
            "{repo_display}  {head_display}  total {}",
            format_bytes(report.total_size_bytes)
        );
        for artifact in &report.artifacts {
            let rel = display_rel_path(&report.repo_root, &artifact.path);
            println!("  {}  {}", format_bytes(artifact.stats.size_bytes), rel);
        }
        println!();
    }
}

pub fn process_candidate(path: &Path) -> Option<ArtifactRecord> {
    let repo_root = crate::git::find_git_root(path)?;
    let is_ignored = match is_git_ignored(&repo_root, path) {
        Ok(is_ignored) => is_ignored,
        Err(err) => {
            eprintln!(
                "warn: git check-ignore failed: repo={repo_root:?} path={path:?} err={err:#}"
            );
            return None;
        }
    };
    if !is_ignored {
        return None;
    }

    let stats = match dir_stats(path) {
        Ok(stats) => stats,
        Err(err) => {
            eprintln!("warn: stats calculation failed: path={path:?} err={err:#}");
            return None;
        }
    };

    Some(ArtifactRecord {
        repo_root,
        path: path.to_path_buf(),
        stats,
    })
}
