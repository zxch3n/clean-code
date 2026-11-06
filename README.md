# clean-my-code

A small, super fast tool to scan and remove **gitignored build artifacts** (e.g. `target/`, `node_modules/`, `dist/`) across a workspace, **grouped by Git repo root**.

<img width="1294" height="1166" alt="CleanShot 2026-01-06 at 16 16 54@2x" src="https://github.com/user-attachments/assets/2428085f-8627-41f7-9f96-59abadafd567" />

Instead of blindly deleting by directory name, it verifies candidates with `git check-ignore` first.

## What it does

- Recursively scans from `--root` (default: current directory).
- When a directory name matches the built-in (or user-provided) artifact list, it:
  - finds the nearest Git repo root (`.git`),
  - checks if the directory is actually ignored by that repo (`git check-ignore`),
  - computes total size and newest mtime (recursive; skips symlinks),
  - groups results by repo root.
- Default mode is an interactive TUI:
  - auto-selects repos whose artifacts are **>= 180 days old** and **>= `--min-size`** (default `1MiB`),
  - deletes selected repos’ artifacts after a confirmation step.
- `scan` mode prints a report sorted by repo head commit time (oldest first).
- `clean` mode scans and deletes matching repos’ artifacts without a TUI, with a preview and confirmation.

## Install

Option 1: via npm (prebuilt binary)

```bash
npx clean-my-code
# or
npm i -g clean-my-code
```

Option 2: build from source (Rust required)

```bash
# From the project root
cargo install --path .

# Or run directly
cargo run --release -- --help
```

## Usage

Default (TUI):

```bash
clean-my-code
```

Choose scan root:

```bash
clean-my-code --root /path/to/workspace
```

Control parallelism (Rayon):

```bash
clean-my-code --threads 8
```

TUI options:

```bash
clean-my-code tui --min-size 1MiB
clean-my-code tui --dry-run
```

Scan-only report (no TUI):

```bash
clean-my-code scan
```

Add artifact dir names (repeatable):

```bash
clean-my-code --artifact .gradle --artifact .venv
```

Only use your custom list (disable built-ins):

```bash
clean-my-code --no-default-artifacts --artifact target --artifact node_modules
```

Run `clean-my-code --help` for the full CLI reference.

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

## Default artifact dir names

These directory names are treated as candidates (they are only counted/deleted if `git check-ignore` says they are ignored):

- `target`
- `dist`
- `build`
- `out`
- `obj`
- `Debug`
- `Release`
- `node_modules`
- `bower_components`
- `elm-stuff`
- `.next`
- `.nuxt`
- `.svelte-kit`
- `.astro`
- `storybook-static`
- `_site`
- `.turbo`
- `.parcel-cache`
- `.vite`
- `.angular`
- `__pycache__`
- `.pytest_cache`
- `.mypy_cache`
- `.ruff_cache`
- `.tox`
- `.nox`
- `.venv`
- `venv`
- `.ipynb_checkpoints`
- `htmlcov`
- `.pyre`
- `.pytype`
- `.gradle`
- `dist-newstyle`
- `.stack-work`
- `.vs`
- `CMakeFiles`
- `cmake-build-debug`
- `cmake-build-release`
- `cmake-build-relwithdebinfo`
- `cmake-build-minsizerel`
- `Pods`
- `Carthage`
- `.swiftpm`
- `.build`
- `DerivedData`
- `.dart_tool`
- `coverage`

## Notes

- Size is computed as the sum of file sizes (not disk blocks like `du`).
- Requires `git` on `PATH` and follows Git ignore rules (`.gitignore`, `.git/info/exclude`, global excludes).
- Git worktree and other multi-level layouts are supported; when a directory is not a repo, scan probes 1-2 levels below for nested git repos.
- Linux prebuilt releases (`x86_64-unknown-linux-gnu`) are CI-checked to require at most `GLIBC_2.36`.
- The built-in list is intentionally conservative. It excludes stateful or user-managed directories that may contain secrets, deployment metadata, uploads, or local state (e.g. `.terraform`, `.direnv`, `.vercel`, `.serverless`, `public`, `packages`, `bin`, `tmp`, `.pulumi`, `.vagrant`). Add them explicitly via `--artifact` only if you are sure they are safe to remove.
- The TUI is built with `ratatui` + `crossterm`. If keybindings/rendering are odd, check your terminal settings and input method conflicts.
