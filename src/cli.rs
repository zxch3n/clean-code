use std::{
    collections::HashSet,
    ffi::OsString,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    str::FromStr,
    time::SystemTime,
};

use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand};

use crate::{
    clean::{execute_delete_with_progress, plan_delete_targets},
    format::format_bytes,
    report::{RepoReport, collect_reports, print_scan_report},
    tui::TuiOptions,
};

const DEFAULT_ARTIFACT_DIR_NAMES: &[&str] = &[
    // General build outputs.
    "target",
    "dist",
    "build",
    "out",
    "obj",
    "Debug",
    "Release",
    // Frontend / JS / Node.
    "node_modules",
    "bower_components",
    "elm-stuff",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".astro",
    "storybook-static",
    "_site",
    ".turbo",
    ".parcel-cache",
    ".vite",
    ".angular",
    // Python / tooling.
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
    ".nox",
    ".venv",
    "venv",
    ".ipynb_checkpoints",
    "htmlcov",
    ".pyre",
    ".pytype",
    // JVM / Haskell / build tools.
    ".gradle",
    "dist-newstyle",
    ".stack-work",
    // .NET / Visual Studio.
    ".vs",
    // CMake (CLion).
    "CMakeFiles",
    "cmake-build-debug",
    "cmake-build-release",
    "cmake-build-relwithdebinfo",
    "cmake-build-minsizerel",
    // Apple platforms.
    "Pods",
    "Carthage",
    ".swiftpm",
    ".build",
    "DerivedData",
    // Mobile / Flutter.
    ".dart_tool",
    // Misc.
    "coverage",
];

#[derive(Parser, Debug)]
#[command(name = "clean-my-code")]
#[command(about = "Scan and clean gitignored build artifacts per Git repo.")]
pub struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Args, Debug, Clone)]
struct CommonArgs {
    #[arg(long, global = true, default_value = ".", value_name = "PATH")]
    root: PathBuf,

    #[arg(long, global = true, value_name = "N")]
    threads: Option<usize>,

    #[arg(long = "artifact", global = true, value_name = "NAME")]
    artifacts: Vec<String>,

    #[arg(long, global = true)]
    no_default_artifacts: bool,
}

#[derive(Subcommand, Debug, Clone)]
enum Command {
    /// Print a scan report without deleting anything.
    Scan,

    /// Scan, preview, and delete artifacts from matching repos without a TUI.
    Clean(CleanArgs),

    Tui(TuiArgs),
}

#[derive(Args, Debug, Clone)]
struct CleanArgs {
    /// Minimum total artifact size per repo.
    #[arg(long, default_value = "1MiB")]
    min_size: ByteSize,

    /// Minimum age in days of the newest artifact modification per repo (0 disables age filtering).
    #[arg(long, default_value_t = 180)]
    older_than: u64,

    /// Preview matching paths without deleting anything or asking for confirmation.
    #[arg(long)]
    dry_run: bool,

    /// Delete without prompting (required for non-interactive use).
    #[arg(short = 'y', long)]
    yes: bool,
}

#[derive(Args, Debug, Clone)]
struct TuiArgs {
    #[arg(long, default_value = "1MiB")]
    min_size: ByteSize,

    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Clone, Copy)]
struct ByteSize(u64);

impl ByteSize {
    fn as_u64(self) -> u64 {
        self.0
    }
}

impl FromStr for ByteSize {
    type Err = anyhow::Error;

    fn from_str(input: &str) -> Result<Self> {
        let input = input.trim();
        if input.is_empty() {
            return Err(anyhow!("size cannot be empty"));
        }

        let input_lower = input.to_ascii_lowercase();
        let unit_start = input_lower
            .find(|c: char| c.is_ascii_alphabetic())
            .unwrap_or(input_lower.len());
        let (value_raw, unit_raw) = input_lower.split_at(unit_start);

        let value_raw = value_raw.trim().replace('_', "");
        let value: f64 = value_raw
            .parse()
            .with_context(|| format!("invalid size number: {value_raw:?}"))?;

        if !value.is_finite() || value < 0.0 {
            return Err(anyhow!("size must be a finite non-negative number"));
        }

        let multiplier = match unit_raw.trim() {
            "" | "b" => 1u64,
            "k" | "kb" => 1_000u64,
            "m" | "mb" => 1_000_000u64,
            "g" | "gb" => 1_000_000_000u64,
            "t" | "tb" => 1_000_000_000_000u64,
            "p" | "pb" => 1_000_000_000_000_000u64,
            "kib" => 1024u64,
            "mib" => 1024u64.pow(2),
            "gib" => 1024u64.pow(3),
            "tib" => 1024u64.pow(4),
            "pib" => 1024u64.pow(5),
            unit => return Err(anyhow!("unsupported size unit: {unit:?}")),
        };

        let bytes = value * (multiplier as f64);
        if bytes > (u64::MAX as f64) {
            return Err(anyhow!("size is too large"));
        }

        Ok(ByteSize(bytes as u64))
    }
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    run_with_cli(cli)
}

fn run_with_cli(cli: Cli) -> Result<()> {
    let scan_root = std::fs::canonicalize(&cli.common.root)
        .with_context(|| format!("invalid root: {:?}", cli.common.root))?;

    let mut artifact_dir_names: HashSet<OsString> = HashSet::new();
    if !cli.common.no_default_artifacts {
        artifact_dir_names.extend(
            DEFAULT_ARTIFACT_DIR_NAMES
                .iter()
                .copied()
                .map(OsString::from),
        );
    }
    artifact_dir_names.extend(cli.common.artifacts.into_iter().map(OsString::from));

    if artifact_dir_names.is_empty() {
        anyhow::bail!("no artifact directory names configured");
    }

    let command = cli.command.unwrap_or_else(|| {
        Command::Tui(TuiArgs {
            min_size: ByteSize::from_str("1MiB").unwrap_or(ByteSize(1024 * 1024)),
            dry_run: false,
        })
    });

    match command {
        Command::Scan | Command::Clean(_) => {
            let run_scan = || -> Result<()> {
                let reports = collect_reports(&scan_root, &artifact_dir_names);
                match &command {
                    Command::Clean(args) => run_clean(&scan_root, reports, args),
                    _ => {
                        print_scan_report(&scan_root, &reports);
                        Ok(())
                    }
                }
            };

            match cli.common.threads {
                Some(threads) => {
                    let pool = rayon::ThreadPoolBuilder::new()
                        .num_threads(threads)
                        .build()
                        .context("failed to build rayon thread pool")?;
                    pool.install(run_scan)
                }
                None => run_scan(),
            }
        }
        Command::Tui(args) => crate::tui::run(
            &scan_root,
            artifact_dir_names,
            cli.common.threads,
            TuiOptions {
                min_size_bytes: args.min_size.as_u64(),
                dry_run: args.dry_run,
            },
        ),
    }
}

fn matches_clean_filters(report: &RepoReport, args: &CleanArgs, now: SystemTime) -> bool {
    if report.artifacts.is_empty() || report.total_size_bytes < args.min_size.as_u64() {
        return false;
    }
    if args.older_than == 0 {
        return true;
    }
    report
        .newest_mtime
        .and_then(|mtime| now.duration_since(mtime).ok())
        .is_some_and(|age| age.as_secs() / 86_400 >= args.older_than)
}

fn run_clean(scan_root: &Path, mut reports: Vec<RepoReport>, args: &CleanArgs) -> Result<()> {
    let now = SystemTime::now();
    reports.retain(|report| matches_clean_filters(report, args, now));
    print_scan_report(scan_root, &reports);
    let targets = plan_delete_targets(reports.iter().map(|report| (report, true)));
    if targets.is_empty() {
        println!("Nothing to clean for the current filters.");
        return Ok(());
    }
    let planned_bytes = targets.iter().map(|target| target.planned_bytes).sum();
    println!(
        "Plan: delete {} artifact dirs from {} repos ({}){}",
        targets.len(),
        reports.len(),
        format_bytes(planned_bytes),
        if args.dry_run { " [dry-run]" } else { "" }
    );

    if !args.dry_run && !args.yes {
        if !io::stdin().is_terminal() {
            anyhow::bail!(
                "confirmation requires a terminal; use --yes to delete or --dry-run to preview"
            );
        }
        print!("Delete these directories? [y/N] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Canceled. Nothing deleted.");
            return Ok(());
        }
    }

    let summary = execute_delete_with_progress(&targets, args.dry_run, || false, |_| {});
    if args.dry_run {
        println!(
            "Dry run complete. Nothing deleted. Skipped: {}",
            summary.skipped_paths
        );
    } else {
        println!(
            "Deleted: {} dirs ({})  Skipped: {}  Errors: {}",
            summary.deleted_paths,
            format_bytes(summary.deleted_bytes),
            summary.skipped_paths,
            summary.errors.len()
        );
    }
    for (path, err) in &summary.errors {
        eprintln!("error: {}: {err:#}", path.display());
    }
    if !summary.errors.is_empty() {
        anyhow::bail!("cleanup encountered {} error(s)", summary.errors.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{report::ArtifactRecord, scan::DirStats};
    use std::time::Duration;

    #[test]
    fn cleanup_age_filter_handles_boundary_missing_and_future_times() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(200 * 86_400);
        let args = CleanArgs {
            min_size: ByteSize(1),
            older_than: 180,
            dry_run: true,
            yes: false,
        };
        let mut report = RepoReport {
            repo_root: PathBuf::from("repo"),
            head: None,
            artifacts: vec![ArtifactRecord {
                repo_root: PathBuf::from("repo"),
                path: PathBuf::from("repo/target"),
                stats: DirStats::default(),
            }],
            total_size_bytes: 1,
            newest_mtime: None,
        };
        for (mtime, expected) in [
            (None, false),
            (Some(now + Duration::from_secs(1)), false),
            (Some(now - Duration::from_secs(180 * 86_400 - 1)), false),
            (Some(now - Duration::from_secs(180 * 86_400)), true),
        ] {
            report.newest_mtime = mtime;
            assert_eq!(matches_clean_filters(&report, &args, now), expected);
        }
        report.newest_mtime = None;
        assert!(matches_clean_filters(
            &report,
            &CleanArgs {
                older_than: 0,
                ..args
            },
            now
        ));
    }

    #[test]
    fn default_artifacts_exclude_stateful_or_user_managed_dirs() {
        for name in [
            ".terraform",
            ".direnv",
            ".vercel",
            ".serverless",
            ".cache",
            "public",
            "packages",
            "bin",
            "env",
            "ENV",
            "tmp",
            "temp",
        ] {
            assert!(
                !DEFAULT_ARTIFACT_DIR_NAMES.contains(&name),
                "default artifacts must not include {name} because it may contain local state, secrets, or user-managed data"
            );
        }
    }
}
