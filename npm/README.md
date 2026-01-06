# clean-code (npm)

This npm package is a thin wrapper around the Rust `clean-code` CLI.

On install, it downloads a prebuilt binary for your OS/CPU and exposes the `clean-code` command.

## Quick Start

```bash
npx clean-code
# or
npm i -g clean-code
clean-code
```

## Requirements

- Node.js >= 14
- `git` available on `PATH` (the CLI uses `git check-ignore` / `git log`)

## Usage

```bash
# Default (TUI)
clean-code

# Choose scan root
clean-code --root /path/to/workspace

# Scan-only (no TUI)
clean-code scan

# TUI with options
clean-code tui --min-size 1MiB
clean-code tui --dry-run

# Control parallelism
clean-code --threads 8

# Add artifact dir names (repeatable)
clean-code --artifact .gradle --artifact .venv

# Only use your custom list (disable built-ins)
clean-code --no-default-artifacts --artifact target --artifact node_modules
```

Run `clean-code --help` for the full CLI reference.

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

If your platform isn’t covered, build from source (Rust required).

## Install Details & Troubleshooting

- This package downloads a prebuilt binary during `npm install` into `vendor/` and runs it via a small JS shim at `bin/clean-code.js`.
- To use a custom mirror, set `CLEAN_CODE_DOWNLOAD_BASE` to a base URL that mirrors the GitHub Releases layout.
