use std::{collections::HashSet, ffi::OsString, path::PathBuf, str::FromStr};

use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand};

use crate::{report::collect_reports, report::print_scan_report, tui::TuiOptions};

const DEFAULT_ARTIFACT_DIR_NAMES: [&str; 31] = [
    "target",
    "node_modules",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".astro",
    ".vercel",
    ".turbo",
    ".cache",
    ".parcel-cache",
    ".vite",
    ".angular",
    ".gradle",
    ".terraform",
    ".serverless",
    ".dart_tool",
    ".venv",
    "venv",
    ".tox",
    ".direnv",
    "bin",
    "obj",
    "coverage",
    ".pytest_cache",
    "__pycache__",
    ".mypy_cache",
    ".ruff_cache",
    "tmp",
];

#[derive(Parser, Debug)]
#[command(name = "clean-code")]
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
    Scan,

    Tui(TuiArgs),
}

#[derive(Args, Debug, Clone)]
struct TuiArgs {
    #[arg(long, default_value_t = 30)]
    stale_days: u64,

    #[arg(long, default_value = "1MiB")]
    min_size: ByteSize,

    #[arg(long)]
    clean_all: bool,

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
        artifact_dir_names.extend(DEFAULT_ARTIFACT_DIR_NAMES.map(OsString::from));
    }
    artifact_dir_names.extend(cli.common.artifacts.into_iter().map(OsString::from));

    if artifact_dir_names.is_empty() {
        anyhow::bail!("no artifact directory names configured");
    }

    let command = cli.command.unwrap_or_else(|| {
        Command::Tui(TuiArgs {
            stale_days: 30,
            min_size: ByteSize::from_str("1MiB").unwrap_or(ByteSize(1024 * 1024)),
            clean_all: false,
            dry_run: false,
        })
    });

    match command {
        Command::Scan => {
            let run_scan = || -> Result<()> {
                let reports = collect_reports(&scan_root, &artifact_dir_names);
                print_scan_report(&scan_root, &reports);
                Ok(())
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
                stale_days: args.stale_days,
                min_size_bytes: args.min_size.as_u64(),
                clean_all: args.clean_all,
                dry_run: args.dry_run,
            },
        ),
    }
}
