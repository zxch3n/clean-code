# clean-code

一个用于“按 Git 仓库聚合”统计并清理 **被 `.gitignore` 忽略的构建产物（artifacts）** 的小工具。

核心行为（scan + clean）：

- 从 `--root`（默认当前目录）开始递归扫描所有子目录
- 发现疑似 artifacts 的目录名后（例如 `target/`、`node_modules/`、`dist/`），使用 `git check-ignore` 判定其是否真的被忽略
- 对被忽略的目录计算体积与最近修改时间（递归累加文件大小；默认不跟随 symlink）
- 按 “artifact 所属的最近 Git 仓库根目录” 分组
- 默认进入 TUI：默认预选 “最近修改时间距今 >= 180d 且体积 >= 1MiB” 的 stale artifacts，回车后删除选中的 artifacts
- 也支持 `scan` 子命令：按每个仓库的最新 commit 时间从老到新排序输出统计结果

## Usage

默认（TUI）：

```bash
cargo run --release
```

指定扫描根目录：

```bash
cargo run --release -- --root /path/to/workspace
```

设置线程数（Rayon）：

```bash
cargo run --release -- --threads 8
```

TUI 参数：

```bash
cargo run --release -- tui --stale-days 30 --min-size 1MiB
```

TUI 键位：

- Up/Down：移动
- Space：切换选中
- a：全选
- n：全不选
- Enter：确认并删除（会有二次确认）
- q / Esc：退出

预览删除（不实际删除）：

```bash
cargo run --release -- tui --dry-run
```

删除所有被忽略的 artifacts（不做 stale 过滤）：

```bash
cargo run --release -- tui --clean-all
```

仅输出统计（scan）：

```bash
cargo run --release -- scan
```

追加自定义 artifacts 目录名：

```bash
cargo run --release -- --artifact .gradle --artifact .venv
```

仅使用自定义列表（禁用默认清单）：

```bash
cargo run --release -- --no-default-artifacts --artifact target --artifact node_modules
```

## Default artifact dir names

默认会识别以下目录名（最终是否计入仍以 `git check-ignore` 为准）：

- `target`
- `node_modules`
- `dist`
- `build`
- `out`
- `.next`
- `.nuxt`
- `.svelte-kit`
- `.astro`
- `.vercel`
- `.turbo`
- `.cache`
- `.parcel-cache`
- `.vite`
- `.angular`
- `.gradle`
- `.terraform`
- `.serverless`
- `.dart_tool`
- `.venv`
- `venv`
- `.tox`
- `.direnv`
- `bin`
- `obj`
- `coverage`
- `.pytest_cache`
- `__pycache__`
- `.mypy_cache`
- `.ruff_cache`
- `tmp`

## Notes

- 体积统计为 “文件大小之和”，不等同于磁盘块占用（`du`）。
- 目录判定依赖本机 `git` 可执行文件，并以 Git 的 ignore 规则为准（含 `.gitignore` / `.git/info/exclude` / global excludes）。
- TUI 基于 `ratatui` + `crossterm`，理论上可跨平台运行；如遇键位/渲染异常，优先检查终端类型与输入法/快捷键冲突。
