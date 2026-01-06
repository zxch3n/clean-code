use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow};

#[derive(Debug, Clone)]
pub struct GitHead {
    pub hash: String,
    pub unix_seconds: i64,
    pub iso8601: String,
}

pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if has_dot_git(dir) {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

pub fn is_git_ignored(repo_root: &Path, path: &Path) -> Result<bool> {
    let rel = path.strip_prefix(repo_root).with_context(|| {
        format!("path is not under repo root: repo={repo_root:?}, path={path:?}")
    })?;

    let status = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["check-ignore", "--quiet", "--"])
        .arg(rel)
        .status()
        .with_context(|| format!("failed to run git check-ignore in {repo_root:?}"))?;

    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        Some(code) => Err(anyhow!("git check-ignore failed with exit code {code}")),
        None => Err(anyhow!("git check-ignore terminated by signal")),
    }
}

pub fn git_head(repo_root: &Path) -> Result<Option<GitHead>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["log", "-1", "--format=%H%n%ct%n%cI"])
        .output()
        .with_context(|| format!("failed to run git log in {repo_root:?}"))?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8(output.stdout).context("git log output is not valid UTF-8")?;
    let mut lines = stdout.lines();

    let hash = lines.next().unwrap_or_default().trim().to_string();
    let unix_seconds: i64 = lines
        .next()
        .unwrap_or_default()
        .trim()
        .parse()
        .context("failed to parse git unix timestamp")?;
    let iso8601 = lines.next().unwrap_or_default().trim().to_string();

    if hash.is_empty() || iso8601.is_empty() {
        return Ok(None);
    }

    Ok(Some(GitHead {
        hash,
        unix_seconds,
        iso8601,
    }))
}

fn has_dot_git(dir: &Path) -> bool {
    std::fs::metadata(dir.join(".git")).is_ok()
}
