# clean-my-code

A small, super fast tool to scan and remove **gitignored build artifacts** (e.g. `target/`, `node_modules/`, `dist/`) across a workspace, **grouped by Git repo root**.

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
- `bin`
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
- `public`
- `.vercel`
- `.turbo`
- `.cache`
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
- `env`
- `ENV`
- `.direnv`
- `.ipynb_checkpoints`
- `htmlcov`
- `.pyre`
- `.pytype`
- `.gradle`
- `dist-newstyle`
- `.stack-work`
- `.vs`
- `packages`
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
- `.terraform`
- `.serverless`
- `coverage`
- `tmp`
- `temp`

## Notes

- Size is computed as the sum of file sizes (not disk blocks like `du`).
- Requires `git` on `PATH` and follows Git ignore rules (`.gitignore`, `.git/info/exclude`, global excludes).
- The built-in list intentionally does not include some stateful directories (e.g. `.pulumi`, `.vagrant`). Add them explicitly via `--artifact` if you really want to clean them.
- The TUI is built with `ratatui` + `crossterm`. If keybindings/rendering are odd, check your terminal settings and input method conflicts.
