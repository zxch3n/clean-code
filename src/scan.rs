use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use anyhow::{Context, Result};

#[derive(Debug, Default, Clone, Copy)]
pub struct DirStats {
    pub size_bytes: u64,
    pub newest_mtime: Option<SystemTime>,
}

pub fn scan_artifact_dirs(root: &Path, artifact_dir_names: &HashSet<OsString>) -> Vec<PathBuf> {
    let results: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
    let root_is_git = has_dot_git(root);

    rayon::scope(|scope| {
        scan_dir(
            scope,
            root.to_path_buf(),
            artifact_dir_names,
            Arc::clone(&results),
            root_is_git,
        );
    });

    let mut results = match Arc::try_unwrap(results) {
        Ok(mutex) => mutex.into_inner().unwrap_or_default(),
        Err(arc) => match arc.lock() {
            Ok(guard) => (*guard).clone(),
            Err(poisoned) => (*poisoned.into_inner()).clone(),
        },
    };
    results.sort();
    results.dedup();
    results
}

pub fn dir_stats(root: &Path) -> Result<DirStats> {
    let meta = std::fs::symlink_metadata(root)
        .with_context(|| format!("failed to read metadata: {root:?}"))?;

    if meta.file_type().is_symlink() {
        return Ok(DirStats::default());
    }

    if meta.is_file() {
        return Ok(DirStats {
            size_bytes: meta.len(),
            newest_mtime: meta.modified().ok(),
        });
    }

    if !meta.is_dir() {
        return Ok(DirStats::default());
    }

    let global: Arc<Mutex<DirStats>> = Arc::new(Mutex::new(DirStats {
        size_bytes: 0,
        newest_mtime: meta.modified().ok(),
    }));

    rayon::scope(|scope| walk_dir_stats(scope, root.to_path_buf(), Arc::clone(&global)));

    let stats = match global.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    };

    Ok(stats)
}

fn walk_dir_stats<'scope>(
    scope: &rayon::Scope<'scope>,
    dir: PathBuf,
    global: Arc<Mutex<DirStats>>,
) {
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    let mut local = DirStats {
        size_bytes: 0,
        newest_mtime: None,
    };

    if let Ok(meta) = std::fs::symlink_metadata(&dir)
        && !meta.file_type().is_symlink()
    {
        local.merge_mtime(meta.modified().ok());
    }

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };

        if file_type.is_symlink() {
            continue;
        }

        let path = entry.path();
        if file_type.is_dir() {
            let global = Arc::clone(&global);
            scope.spawn(move |scope| walk_dir_stats(scope, path, global));
            continue;
        }

        if file_type.is_file() {
            let meta = match entry.metadata() {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            local.size_bytes = local.size_bytes.saturating_add(meta.len());
            local.merge_mtime(meta.modified().ok());
        }
    }

    let mut global_guard = match global.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    global_guard.merge(local);
}

fn scan_dir<'scope>(
    scope: &rayon::Scope<'scope>,
    dir: PathBuf,
    artifact_dir_names: &'scope HashSet<OsString>,
    results: Arc<Mutex<Vec<PathBuf>>>,
    in_git_repo: bool,
) {
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };

        if !file_type.is_dir() {
            continue;
        }

        let file_name = entry.file_name();
        if file_name == ".git" {
            continue;
        }

        let path = entry.path();
        if artifact_dir_names.contains(&file_name) {
            let mut results = match results.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            results.push(path);
            continue;
        }

        if in_git_repo {
            let results = Arc::clone(&results);
            scope.spawn(move |scope| scan_dir(scope, path, artifact_dir_names, results, true));
            continue;
        }

        if has_dot_git(&path) {
            let results = Arc::clone(&results);
            scope.spawn(move |scope| scan_dir(scope, path, artifact_dir_names, results, true));
            continue;
        }

        // Generic multi-level layout support:
        // if a directory is not a repo itself, probe 1-2 levels below for nested repos.
        let nested_git_roots = find_nested_git_roots(&path, 2);
        if nested_git_roots.is_empty() {
            let results = Arc::clone(&results);
            scope.spawn(move |scope| scan_dir(scope, path, artifact_dir_names, results, false));
            continue;
        }

        for repo_root in nested_git_roots {
            let results = Arc::clone(&results);
            scope.spawn(move |scope| scan_dir(scope, repo_root, artifact_dir_names, results, true));
        }
    }
}

impl DirStats {
    fn merge(&mut self, other: DirStats) {
        self.size_bytes = self.size_bytes.saturating_add(other.size_bytes);
        self.merge_mtime(other.newest_mtime);
    }

    fn merge_mtime(&mut self, other: Option<SystemTime>) {
        let Some(other) = other else {
            return;
        };

        self.newest_mtime = match self.newest_mtime {
            Some(existing) if existing >= other => Some(existing),
            _ => Some(other),
        };
    }
}

fn has_dot_git(path: &Path) -> bool {
    std::fs::metadata(path.join(".git")).is_ok()
}

fn find_nested_git_roots(start: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut stack = vec![(start.to_path_buf(), 0usize)];
    let mut roots = Vec::new();

    while let Some((dir, depth)) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if !file_type.is_dir() {
                continue;
            }

            let file_name = entry.file_name();
            if file_name == ".git" {
                continue;
            }

            let path = entry.path();
            if has_dot_git(&path) {
                roots.push(path);
                continue;
            }

            if depth < max_depth {
                stack.push((path, depth + 1));
            }
        }
    }

    roots.sort();
    roots.dedup();
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashSet,
        ffi::OsString,
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn scan_uses_nested_git_probe_for_multi_level_layout() {
        let root = make_temp_dir("clean-my-code-scan");
        let repo = root.join("repo");

        fs::create_dir_all(repo.join("bare.git/objects/target")).unwrap();
        fs::create_dir_all(repo.join("bare.git/refs")).unwrap();
        fs::write(repo.join("bare.git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let worktree_target = repo.join("worktrees/w1/target");
        fs::create_dir_all(&worktree_target).unwrap();
        fs::write(
            repo.join("worktrees/w1/.git"),
            "gitdir: ../../bare.git/worktrees/w1\n",
        )
        .unwrap();

        let mut artifact_dir_names = HashSet::new();
        artifact_dir_names.insert(OsString::from("target"));

        let found = scan_artifact_dirs(&root, &artifact_dir_names);
        assert_eq!(found, vec![worktree_target]);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_falls_back_to_deeper_walk_when_probe_misses() {
        let root = make_temp_dir("clean-my-code-scan");
        let repo_root = root.join("a/b/c/d/repo");
        let target = repo_root.join("target");

        fs::create_dir_all(&target).unwrap();
        fs::write(repo_root.join(".git"), "gitdir: /tmp/fake\n").unwrap();

        let mut artifact_dir_names = HashSet::new();
        artifact_dir_names.insert(OsString::from("target"));

        let found = scan_artifact_dirs(&root, &artifact_dir_names);
        assert_eq!(found, vec![target]);

        let _ = fs::remove_dir_all(root);
    }

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
