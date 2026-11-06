# clean-my-code (npm)

This npm package is a thin wrapper around the Rust `clean-my-code` CLI.

On install, it downloads a prebuilt binary for your OS/CPU and exposes the `clean-my-code` command.

## Quick Start

```bash
npx clean-my-code
# or
npm i -g clean-my-code
clean-my-code
```

## Requirements

- Node.js >= 14
- `git` available on `PATH` (the CLI uses `git check-ignore` / `git log`)

## Usage

```bash
# Default (TUI)
clean-my-code

# Choose scan root
clean-my-code --root /path/to/workspace

# Scan-only (no TUI)
clean-my-code scan

# TUI with options
clean-my-code tui --min-size 1MiB
clean-my-code tui --dry-run

# Control parallelism
clean-my-code --threads 8

# Add artifact dir names (repeatable)
clean-my-code --artifact .gradle --artifact .venv

# Only use your custom list (disable built-ins)
clean-my-code --no-default-artifacts --artifact target --artifact node_modules
```

Run `clean-my-code --help` for the full CLI reference.

The built-in artifact list is intentionally conservative. It excludes stateful or user-managed directories that may contain secrets, deployment metadata, uploads, or local state, such as `.terraform`, `.direnv`, `.vercel`, `.serverless`, `public`, `packages`, `bin`, and `tmp`. Add them explicitly via `--artifact` only if you are sure they are safe to remove.

### CLI cleanup (no TUI)

```bash
# Preview the directories that would be deleted
clean-my-code --root /path/to/workspace clean --dry-run

# Scan, show the plan, then ask for confirmation
clean-my-code --root /path/to/workspace clean

# Delete without prompting (for scripts)
clean-my-code --root /path/to/workspace clean --yes

# Customize age (days) and minimum total size per repo
clean-my-code --root /path/to/workspace clean --older-than 30 --min-size 100MiB --dry-run

# Disable age and size filtering explicitly
clean-my-code --root /path/to/workspace clean --older-than 0 --min-size 0 --yes
```

Filters apply to each repo as a whole: by default, its combined artifact size must
be at least `1MiB`, and its newest artifact modification must be at least 180 days
old (including directories and nested files, not the Git commit date). All scanned
artifacts in a matching repo are included. Unknown or future modification times
are excluded when age filtering is enabled. `--older-than 0` disables age filtering.

`--dry-run` never deletes, even with `--yes`. Without `--yes`, deletion requires
confirmation in a terminal; scripts must pass `--yes`. Only configured artifact
directories ignored by Git are eligible, and Git ignore rules are checked again
before deletion. Cleanup errors produce a nonzero exit code; earlier successful
deletions are not rolled back.

## TUI keybindings

- Up/Down: move cursor
- Space: toggle selection
- a: select all
- n: select none
- Tab: toggle sort (age/size)
- Enter: confirm and delete (with a second confirmation)
- q / Esc: quit

## Supported Platforms

Prebuilt binaries are provided for:

- macOS: `x64`, `arm64`
- Linux (gnu): `x64`, `arm64`
- Windows (MSVC): `x64`

For Linux `x64` prebuilt releases (`x86_64-unknown-linux-gnu`), CI enforces
that required GLIBC symbols do not exceed `GLIBC_2.36`.

If your platform isn’t covered, build from source (Rust required).

## Install Details & Troubleshooting

- This package downloads a prebuilt binary during `npm install` into `vendor/` and runs it via a small JS shim at `bin/clean-my-code.js`.
- To use a custom mirror, set `CLEAN_MY_CODE_DOWNLOAD_BASE` (or legacy `CLEAN_CODE_DOWNLOAD_BASE`) to a base URL that mirrors the GitHub Releases layout.
- Git worktree and other multi-level layouts are supported; scanning probes 1-2 levels below non-repo directories to find nested git repos.
