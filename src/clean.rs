use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use anyhow::anyhow;

use crate::{git::is_git_ignored, report::RepoReport};

#[derive(Debug, Clone)]
pub struct DeleteTarget {
    pub repo_root: PathBuf,
    pub path: PathBuf,
    pub planned_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct DeleteProgress {
    pub processed: usize,
    pub total: usize,
    pub deleted_paths: usize,
    pub deleted_bytes: u64,
    pub skipped_paths: usize,
    pub error_count: usize,
}

#[derive(Debug, Default)]
pub struct DeleteSummary {
    pub planned_paths: usize,
    pub planned_bytes: u64,
    pub deleted_paths: usize,
    pub deleted_bytes: u64,
    pub skipped_paths: usize,
    pub errors: Vec<(PathBuf, anyhow::Error)>,
}

pub fn plan_delete_targets<'a, I>(reports: I) -> Vec<DeleteTarget>
where
    I: IntoIterator<Item = (&'a RepoReport, bool)>,
{
    let mut targets = Vec::new();
    for (report, is_selected) in reports {
        if !is_selected {
            continue;
        }

        for artifact in &report.artifacts {
            targets.push(DeleteTarget {
                repo_root: report.repo_root.clone(),
                path: artifact.path.clone(),
                planned_bytes: artifact.stats.size_bytes,
            });
        }
    }
    targets.sort_by(|a, b| a.path.cmp(&b.path));
    targets.dedup_by(|a, b| a.path == b.path);
    targets
}

pub fn execute_delete_with_progress<C, F>(
    targets: &[DeleteTarget],
    dry_run: bool,
    should_cancel: C,
    mut on_progress: F,
) -> DeleteSummary
where
    C: Fn() -> bool,
    F: FnMut(DeleteProgress),
{
    let planned_bytes = targets.iter().map(|t| t.planned_bytes).sum::<u64>();
    let mut summary = DeleteSummary {
        planned_paths: targets.len(),
        planned_bytes,
        ..DeleteSummary::default()
    };

    for (index, target) in targets.iter().enumerate() {
        let processed = index + 1;
        let total = summary.planned_paths;

        if should_cancel() {
            break;
        }

        if is_blocked_path(&target.path) {
            summary.skipped_paths += 1;
            summary.errors.push((
                target.path.clone(),
                anyhow!("refusing to delete blocked path"),
            ));
            on_progress(DeleteProgress {
                processed,
                total,
                deleted_paths: summary.deleted_paths,
                deleted_bytes: summary.deleted_bytes,
                skipped_paths: summary.skipped_paths,
                error_count: summary.errors.len(),
            });
            continue;
        }

        match is_git_ignored(&target.repo_root, &target.path) {
            Ok(true) => {}
            Ok(false) => {
                summary.skipped_paths += 1;
                on_progress(DeleteProgress {
                    processed,
                    total,
                    deleted_paths: summary.deleted_paths,
                    deleted_bytes: summary.deleted_bytes,
                    skipped_paths: summary.skipped_paths,
                    error_count: summary.errors.len(),
                });
                continue;
            }
            Err(err) => {
                summary.skipped_paths += 1;
                summary.errors.push((target.path.clone(), err));
                on_progress(DeleteProgress {
                    processed,
                    total,
                    deleted_paths: summary.deleted_paths,
                    deleted_bytes: summary.deleted_bytes,
                    skipped_paths: summary.skipped_paths,
                    error_count: summary.errors.len(),
                });
                continue;
            }
        }

        if dry_run {
            on_progress(DeleteProgress {
                processed,
                total,
                deleted_paths: summary.deleted_paths,
                deleted_bytes: summary.deleted_bytes,
                skipped_paths: summary.skipped_paths,
                error_count: summary.errors.len(),
            });
            continue;
        }

        match fs::remove_dir_all(&target.path) {
            Ok(()) => {
                summary.deleted_paths += 1;
                summary.deleted_bytes = summary.deleted_bytes.saturating_add(target.planned_bytes);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                summary.skipped_paths += 1;
            }
            Err(err) => {
                summary.errors.push((target.path.clone(), err.into()));
            }
        }

        on_progress(DeleteProgress {
            processed,
            total,
            deleted_paths: summary.deleted_paths,
            deleted_bytes: summary.deleted_bytes,
            skipped_paths: summary.skipped_paths,
            error_count: summary.errors.len(),
        });
    }

    summary
}

fn is_blocked_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == OsStr::new(".git"))
}
