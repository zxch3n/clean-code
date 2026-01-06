use std::{
    cmp::Ordering as CmpOrdering,
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Cell, Clear, HighlightSpacing, Paragraph, Row, Table, TableState, Wrap,
    },
};
use rayon::prelude::*;

use crate::{
    clean::{
        DeleteProgress, DeleteSummary, DeleteTarget, execute_delete_with_progress,
        plan_delete_targets,
    },
    format::{display_rel_path, format_bytes},
    git::{GitHead, git_head},
    report::{ArtifactRecord, RepoReport, process_candidate},
    scan::scan_artifact_dirs,
};

#[derive(Debug, Clone)]
pub struct TuiOptions {
    pub min_size_bytes: u64,
    pub dry_run: bool,
}

pub fn run(
    scan_root: &Path,
    artifact_dir_names: HashSet<OsString>,
    threads: Option<usize>,
    options: TuiOptions,
) -> Result<()> {
    let now = SystemTime::now();

    let (tx, rx) = mpsc::channel::<AppEvent>();
    let scan_cancel = Arc::new(AtomicBool::new(false));
    let clean_cancel = Arc::new(AtomicBool::new(false));
    spawn_scan_worker(
        scan_root.to_path_buf(),
        artifact_dir_names,
        threads,
        Arc::clone(&scan_cancel),
        tx.clone(),
    );

    let mut app = App::new(now);
    let mut terminal = TerminalGuard::enter().context("failed to initialize terminal")?;

    loop {
        while let Ok(event) = rx.try_recv() {
            app.apply_event(scan_root, &options, event);
        }

        terminal.draw(|frame| render(frame, scan_root, &options, &mut app))?;

        if event::poll(Duration::from_millis(50)).context("failed to poll terminal events")? {
            let event = event::read().context("failed to read terminal event")?;
            if let Event::Key(key) = event {
                if handle_key(
                    scan_root,
                    &options,
                    &scan_cancel,
                    &clean_cancel,
                    &tx,
                    &mut app,
                    key,
                )? {
                    break;
                }
            }
        }
    }

    scan_cancel.store(true, Ordering::Relaxed);
    clean_cancel.store(true, Ordering::Relaxed);
    Ok(())
}

fn spawn_scan_worker(
    scan_root: PathBuf,
    artifact_dir_names: HashSet<OsString>,
    threads: Option<usize>,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<AppEvent>,
) {
    thread::spawn(move || {
        let run = || scan_worker(scan_root, artifact_dir_names, cancel, tx);

        let result = match threads {
            Some(threads) => rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .context("failed to build rayon thread pool")
                .and_then(|pool| pool.install(run)),
            None => run(),
        };

        if let Err(err) = result {
            eprintln!("scan worker error: {err:#}");
        }
    });
}

fn scan_worker(
    scan_root: PathBuf,
    artifact_dir_names: HashSet<OsString>,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<AppEvent>,
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }

    let candidates = scan_artifact_dirs(&scan_root, &artifact_dir_names);
    let total = candidates.len();
    let _ = tx.send(AppEvent::Scan(ScanEvent::CandidatesTotal { total }));
    if total == 0 {
        let _ = tx.send(AppEvent::Scan(ScanEvent::Finished));
        return Ok(());
    }

    let processed = AtomicUsize::new(0);
    let head_started: Arc<std::sync::Mutex<HashSet<PathBuf>>> =
        Arc::new(std::sync::Mutex::new(HashSet::new()));

    candidates.par_iter().for_each(|path| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }

        if let Some(record) = process_candidate(path) {
            let repo_root = record.repo_root.clone();
            let should_spawn_head = {
                let mut started = match head_started.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                started.insert(repo_root.clone())
            };

            if should_spawn_head {
                let head = git_head(&repo_root).unwrap_or(None);
                let _ = tx.send(AppEvent::Scan(ScanEvent::RepoHead { repo_root, head }));
            }

            let _ = tx.send(AppEvent::Scan(ScanEvent::Artifact { record }));
        }

        let processed_count = processed.fetch_add(1, Ordering::Relaxed) + 1;
        if processed_count == total || processed_count % 64 == 0 {
            let _ = tx.send(AppEvent::Scan(ScanEvent::CandidateProcessed {
                processed: processed_count,
            }));
        }
    });

    let _ = tx.send(AppEvent::Scan(ScanEvent::CandidateProcessed {
        processed: total,
    }));
    let _ = tx.send(AppEvent::Scan(ScanEvent::Finished));
    Ok(())
}

#[derive(Debug)]
enum AppEvent {
    Scan(ScanEvent),
    Clean(CleanEvent),
}

#[derive(Debug)]
enum ScanEvent {
    CandidatesTotal {
        total: usize,
    },
    CandidateProcessed {
        processed: usize,
    },
    RepoHead {
        repo_root: PathBuf,
        head: Option<GitHead>,
    },
    Artifact {
        record: ArtifactRecord,
    },
    Finished,
}

#[derive(Debug)]
enum CleanEvent {
    Progress {
        progress: DeleteProgress,
        current: DeleteTarget,
    },
    Finished {
        summary: DeleteSummary,
        canceled: bool,
    },
}

#[derive(Debug)]
struct App {
    now: SystemTime,

    sort_mode: SortMode,
    items: Vec<RepoItem>,
    table_state: TableState,
    pending_heads: HashMap<PathBuf, Option<GitHead>>,

    screen: Screen,
    result_lines: Vec<String>,

    scan_started_at: Instant,
    scan_elapsed_final: Option<Duration>,
    scan_total: Option<usize>,
    scan_processed: usize,
    scan_done: bool,
    artifacts_found: usize,

    new_repo_default_selected: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Age,
    Size,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Age(Option<SystemTime>),
    Size {
        bytes: u64,
        time: Option<SystemTime>,
    },
}

impl App {
    fn new(now: SystemTime) -> Self {
        let mut table_state = TableState::default();
        table_state.select(None);

        Self {
            now,
            sort_mode: SortMode::Age,
            items: Vec::new(),
            table_state,
            pending_heads: HashMap::new(),
            screen: Screen::Main,
            result_lines: Vec::new(),
            scan_started_at: Instant::now(),
            scan_elapsed_final: None,
            scan_total: None,
            scan_processed: 0,
            scan_done: false,
            artifacts_found: 0,
            new_repo_default_selected: None,
        }
    }

    fn toggle_sort_mode(&mut self, options: &TuiOptions) {
        self.sort_mode = match self.sort_mode {
            SortMode::Age => SortMode::Size,
            SortMode::Size => SortMode::Age,
        };

        self.sort_keep_cursor(options);
    }

    fn apply_event(&mut self, scan_root: &Path, options: &TuiOptions, event: AppEvent) {
        match event {
            AppEvent::Scan(event) => self.apply_scan_event(scan_root, options, event),
            AppEvent::Clean(event) => self.apply_clean_event(scan_root, options, event),
        }
    }

    fn apply_scan_event(&mut self, scan_root: &Path, options: &TuiOptions, event: ScanEvent) {
        match event {
            ScanEvent::CandidatesTotal { total } => {
                self.scan_total = Some(total);
                self.scan_processed = 0;
                self.scan_elapsed_final = None;
            }
            ScanEvent::CandidateProcessed { processed } => {
                self.scan_processed = processed;
            }
            ScanEvent::RepoHead { repo_root, head } => {
                if let Some(item) = self
                    .items
                    .iter_mut()
                    .find(|i| i.report.repo_root == repo_root)
                {
                    item.head_loaded = true;
                    item.report.head = head;
                } else {
                    self.pending_heads.insert(repo_root, head);
                }
            }
            ScanEvent::Artifact { record } => {
                self.artifacts_found += 1;
                self.upsert_artifact(scan_root, options, record);
            }
            ScanEvent::Finished => {
                self.scan_done = true;
                self.scan_elapsed_final = Some(self.scan_started_at.elapsed());
                if let Some(total) = self.scan_total {
                    self.scan_processed = total;
                }
            }
        }
    }

    fn apply_clean_event(&mut self, scan_root: &Path, options: &TuiOptions, event: CleanEvent) {
        match event {
            CleanEvent::Progress { progress, current } => {
                let Screen::Cleaning(cleaning) = &mut self.screen else {
                    return;
                };

                cleaning.processed = progress.processed;
                cleaning.total = progress.total;
                cleaning.deleted_paths = progress.deleted_paths;
                cleaning.deleted_bytes = progress.deleted_bytes;
                cleaning.skipped_paths = progress.skipped_paths;
                cleaning.error_count = progress.error_count;
                cleaning.current = Some(format!(
                    "{}  {}",
                    display_rel_path(scan_root, &current.repo_root),
                    display_rel_path(&current.repo_root, &current.path)
                ));
            }
            CleanEvent::Finished { summary, canceled } => {
                self.screen = Screen::Result;
                self.result_lines =
                    format_delete_summary(scan_root, &summary, options.dry_run, canceled);
            }
        }
    }

    fn upsert_artifact(&mut self, scan_root: &Path, options: &TuiOptions, record: ArtifactRecord) {
        let repo_root = record.repo_root.clone();
        let sort_mode = self.sort_mode;
        let now = self.now;
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|i| i.report.repo_root == repo_root)
        {
            if item.report.artifacts.iter().any(|a| a.path == record.path) {
                return;
            }

            let old_sort_key = Self::sort_key_for_report(sort_mode, &item.report);

            item.report.total_size_bytes = item
                .report
                .total_size_bytes
                .saturating_add(record.stats.size_bytes);
            item.report.newest_mtime = item.report.newest_mtime.max(record.stats.newest_mtime);
            item.report.artifacts.push(record);

            item.report.artifacts.sort_by(|a, b| {
                b.stats
                    .size_bytes
                    .cmp(&a.stats.size_bytes)
                    .then_with(|| a.path.cmp(&b.path))
            });

            if item.selection_mode == SelectionMode::Auto {
                item.selected = should_auto_select(&item.report, options, now);
            }

            let new_sort_key = Self::sort_key_for_report(sort_mode, &item.report);

            if old_sort_key != new_sort_key {
                self.sort_keep_cursor(options);
            } else {
                self.ensure_selection_valid(options);
            }
            return;
        }

        let (head, head_loaded) = match self.pending_heads.remove(&repo_root) {
            Some(head) => (head, true),
            None => (None, false),
        };

        let record_size_bytes = record.stats.size_bytes;
        let record_newest_mtime = record.stats.newest_mtime;
        let report = RepoReport {
            repo_root: repo_root.clone(),
            head,
            artifacts: vec![record],
            total_size_bytes: record_size_bytes,
            newest_mtime: record_newest_mtime,
        };

        let (selected, selection_mode) = match self.new_repo_default_selected {
            Some(selected) => (selected, SelectionMode::Manual),
            None => (
                should_auto_select(&report, options, now),
                SelectionMode::Auto,
            ),
        };

        self.items.push(RepoItem {
            report,
            head_loaded,
            selected,
            selection_mode,
            repo_display: display_rel_path(scan_root, &repo_root),
        });

        self.sort_keep_cursor(options);
        self.ensure_selection_valid(options);
    }

    fn sort_key_for_report(sort_mode: SortMode, report: &RepoReport) -> SortKey {
        match sort_mode {
            SortMode::Age => SortKey::Age(report.newest_mtime),
            SortMode::Size => SortKey::Size {
                bytes: report.total_size_bytes,
                time: report.newest_mtime,
            },
        }
    }

    fn sort_keep_cursor(&mut self, options: &TuiOptions) {
        let current_repo_root = self.selected_repo_root(options);

        match self.sort_mode {
            SortMode::Age => {
                self.items.sort_by(|a, b| {
                    let a_time = a.report.newest_mtime;
                    let b_time = b.report.newest_mtime;

                    cmp_time_key(a_time, b_time)
                        .then_with(|| a.report.repo_root.cmp(&b.report.repo_root))
                });
            }
            SortMode::Size => {
                self.items.sort_by(|a, b| {
                    let a_bytes = a.report.total_size_bytes;
                    let b_bytes = b.report.total_size_bytes;
                    let a_time = a.report.newest_mtime;
                    let b_time = b.report.newest_mtime;

                    b_bytes
                        .cmp(&a_bytes)
                        .then_with(|| cmp_time_key(a_time, b_time))
                        .then_with(|| a.report.repo_root.cmp(&b.report.repo_root))
                });
            }
        }

        self.restore_selection(options, current_repo_root);
    }

    fn ensure_selection_valid(&mut self, options: &TuiOptions) {
        let visible_len = self.visible_len(options);
        if visible_len == 0 {
            self.table_state.select(None);
            return;
        }

        let selected = self.table_state.selected();
        if selected.is_some_and(|idx| idx < visible_len) {
            return;
        }

        self.table_state.select(Some(0));
    }

    fn restore_selection(&mut self, options: &TuiOptions, repo_root: Option<PathBuf>) {
        let visible_len = self.visible_len(options);
        if visible_len == 0 {
            self.table_state.select(None);
            return;
        }

        if let Some(repo_root) = repo_root {
            let mut row = 0usize;
            for item in &self.items {
                if !is_visible(&item.report, options) {
                    continue;
                }

                if item.report.repo_root == repo_root {
                    self.table_state.select(Some(row));
                    return;
                }
                row += 1;
            }
        }

        self.table_state.select(Some(0));
    }

    fn selected_repo_root(&self, options: &TuiOptions) -> Option<PathBuf> {
        let selected_row = self.table_state.selected()?;
        let mut row = 0usize;
        for item in &self.items {
            if !is_visible(&item.report, options) {
                continue;
            }

            if row == selected_row {
                return Some(item.report.repo_root.clone());
            }
            row += 1;
        }
        None
    }

    fn visible_len(&self, options: &TuiOptions) -> usize {
        self.items
            .iter()
            .filter(|item| is_visible(&item.report, options))
            .count()
    }

    fn move_cursor_up(&mut self, options: &TuiOptions) {
        let visible_len = self.visible_len(options);
        if visible_len == 0 {
            self.table_state.select(None);
            return;
        }

        let current = self
            .table_state
            .selected()
            .unwrap_or(0)
            .min(visible_len - 1);
        self.table_state.select(Some(current.saturating_sub(1)));
    }

    fn move_cursor_down(&mut self, options: &TuiOptions) {
        let visible_len = self.visible_len(options);
        if visible_len == 0 {
            self.table_state.select(None);
            return;
        }

        let current = self
            .table_state
            .selected()
            .unwrap_or(0)
            .min(visible_len - 1);
        self.table_state
            .select(Some((current + 1).min(visible_len - 1)));
    }

    fn move_cursor_by(&mut self, options: &TuiOptions, delta: isize) {
        let visible_len = self.visible_len(options);
        if visible_len == 0 {
            self.table_state.select(None);
            return;
        }

        let current = self.table_state.selected().unwrap_or(0) as isize;
        let max = (visible_len - 1) as isize;
        let next = (current + delta).clamp(0, max) as usize;
        self.table_state.select(Some(next));
    }

    fn toggle_current(&mut self, options: &TuiOptions) {
        let Some(selected_row) = self.table_state.selected() else {
            return;
        };

        let mut row = 0usize;
        for item in &mut self.items {
            if !is_visible(&item.report, options) {
                continue;
            }
            if row == selected_row {
                item.selected = !item.selected;
                item.selection_mode = SelectionMode::Manual;
                return;
            }
            row += 1;
        }
    }

    fn select_all(&mut self, value: bool) {
        self.new_repo_default_selected = Some(value);
        for item in &mut self.items {
            item.selected = value;
            item.selection_mode = SelectionMode::Manual;
        }
    }
}

#[derive(Debug)]
struct RepoItem {
    report: RepoReport,
    head_loaded: bool,
    selected: bool,
    selection_mode: SelectionMode,
    repo_display: String,
}

impl RepoItem {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionMode {
    Auto,
    Manual,
}

#[derive(Debug)]
enum Screen {
    Main,
    Confirm(ConfirmData),
    Cleaning(CleaningData),
    Result,
}

#[derive(Debug, Clone, Copy)]
enum ScreenKind {
    Main,
    Confirm,
    Cleaning,
    Result,
}

#[derive(Debug)]
struct ConfirmData {
    targets: Vec<DeleteTarget>,
    selected_repos: usize,
    planned_dirs: usize,
    planned_bytes: u64,
}

#[derive(Debug)]
struct CleaningData {
    total: usize,
    planned_bytes: u64,
    processed: usize,
    deleted_paths: usize,
    deleted_bytes: u64,
    skipped_paths: usize,
    error_count: usize,
    current: Option<String>,
    started_at: Instant,
    cancel_requested: bool,
}

fn handle_key(
    scan_root: &Path,
    options: &TuiOptions,
    scan_cancel: &Arc<AtomicBool>,
    clean_cancel: &Arc<AtomicBool>,
    tx: &mpsc::Sender<AppEvent>,
    app: &mut App,
    key: KeyEvent,
) -> Result<bool> {
    let screen_kind = match &app.screen {
        Screen::Main => ScreenKind::Main,
        Screen::Confirm(_) => ScreenKind::Confirm,
        Screen::Cleaning(_) => ScreenKind::Cleaning,
        Screen::Result => ScreenKind::Result,
    };

    if matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }
    ) {
        if matches!(screen_kind, ScreenKind::Cleaning) {
            clean_cancel.store(true, Ordering::Relaxed);
            if let Screen::Cleaning(cleaning) = &mut app.screen {
                cleaning.cancel_requested = true;
            }
            return Ok(false);
        }
        return Ok(true);
    }

    match screen_kind {
        ScreenKind::Main => handle_key_main(scan_root, options, app, key),
        ScreenKind::Confirm => {
            handle_key_confirm(scan_root, options, scan_cancel, clean_cancel, tx, app, key)
        }
        ScreenKind::Cleaning => handle_key_cleaning(clean_cancel, app, key),
        ScreenKind::Result => Ok(true),
    }
}

fn handle_key_main(
    _scan_root: &Path,
    options: &TuiOptions,
    app: &mut App,
    key: KeyEvent,
) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Up => app.move_cursor_up(options),
        KeyCode::Down => app.move_cursor_down(options),
        KeyCode::PageUp => app.move_cursor_by(options, -10),
        KeyCode::PageDown => app.move_cursor_by(options, 10),
        KeyCode::Char(' ') => app.toggle_current(options),
        KeyCode::Char('a') => app.select_all(true),
        KeyCode::Char('n') => app.select_all(false),
        KeyCode::Tab => app.toggle_sort_mode(options),
        KeyCode::Enter => {
            let targets = plan_delete_targets(
                app.items
                    .iter()
                    .filter(|item| is_visible(&item.report, options))
                    .map(|item| (&item.report, item.selected)),
            );

            if targets.is_empty() {
                app.screen = Screen::Result;
                app.result_lines = vec!["Nothing to delete for current selection.".to_string()];
                return Ok(false);
            }

            let planned_dirs = targets.len();
            let planned_bytes = targets.iter().map(|t| t.planned_bytes).sum::<u64>();
            let selected_repos = app
                .items
                .iter()
                .filter(|item| item.selected && is_visible(&item.report, options))
                .count();

            app.screen = Screen::Confirm(ConfirmData {
                targets,
                selected_repos,
                planned_dirs,
                planned_bytes,
            });
        }
        _ => {}
    }

    Ok(false)
}

fn handle_key_confirm(
    scan_root: &Path,
    options: &TuiOptions,
    scan_cancel: &Arc<AtomicBool>,
    clean_cancel: &Arc<AtomicBool>,
    tx: &mpsc::Sender<AppEvent>,
    app: &mut App,
    key: KeyEvent,
) -> Result<bool> {
    let targets = match &app.screen {
        Screen::Confirm(confirm) => confirm.targets.clone(),
        _ => return Ok(false),
    };

    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            scan_cancel.store(true, Ordering::Relaxed);
            clean_cancel.store(false, Ordering::Relaxed);
            spawn_clean_worker(
                targets.clone(),
                options.dry_run,
                Arc::clone(clean_cancel),
                tx.clone(),
            );

            let planned_bytes = targets.iter().map(|t| t.planned_bytes).sum::<u64>();
            let current = targets.first().map(|target| {
                format!(
                    "{}  {}",
                    display_rel_path(scan_root, &target.repo_root),
                    display_rel_path(&target.repo_root, &target.path)
                )
            });
            app.screen = Screen::Cleaning(CleaningData {
                total: targets.len(),
                planned_bytes,
                processed: 0,
                deleted_paths: 0,
                deleted_bytes: 0,
                skipped_paths: 0,
                error_count: 0,
                current,
                started_at: Instant::now(),
                cancel_requested: false,
            });
            Ok(false)
        }
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') | KeyCode::Char('N') => {
            app.screen = Screen::Main;
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn handle_key_cleaning(
    clean_cancel: &Arc<AtomicBool>,
    app: &mut App,
    key: KeyEvent,
) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            clean_cancel.store(true, Ordering::Relaxed);
            if let Screen::Cleaning(cleaning) = &mut app.screen {
                cleaning.cancel_requested = true;
            }
        }
        _ => {}
    }

    Ok(false)
}

fn render(frame: &mut Frame, scan_root: &Path, options: &TuiOptions, app: &mut App) {
    match &app.screen {
        Screen::Main => render_main(frame, scan_root, options, app),
        Screen::Confirm(confirm) => render_confirm(frame, scan_root, options, confirm),
        Screen::Cleaning(cleaning) => render_cleaning(frame, scan_root, options, cleaning),
        Screen::Result => render_result(frame, scan_root, app),
    }
}

fn render_main(frame: &mut Frame, scan_root: &Path, options: &TuiOptions, app: &mut App) {
    let area = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    let (planned_dirs, reclaim_bytes, selected_repos) = summarize_selection(&app.items, options);
    let visible_repos = app
        .items
        .iter()
        .filter(|item| is_visible(&item.report, options))
        .count();

    let dry_run_label = if options.dry_run { " DRY RUN" } else { "" };
    let sort_label = match app.sort_mode {
        SortMode::Age => "age",
        SortMode::Size => "size",
    };

    let header = Paragraph::new(Text::from(vec![
        Line::from(format!(
            "clean-code  show>={}  auto-select>=180d{}  sort={sort_label}",
            format_bytes(options.min_size_bytes),
            dry_run_label
        )),
        Line::from(format!("root: {}", scan_root.display())),
        Line::from(format!(
            "shown: {} repos  selected: {} repos  planned: {} dirs  reclaim: {}",
            visible_repos,
            selected_repos,
            planned_dirs,
            format_bytes(reclaim_bytes)
        )),
        Line::from(""),
    ]));
    frame.render_widget(header, layout[0]);

    let visible_items: Vec<Row<'static>> = app
        .items
        .iter()
        .filter(|item| is_visible(&item.report, options))
        .map(|item| render_repo_row(item, app.now))
        .collect();

    if visible_items.is_empty() {
        let threshold = format_bytes(options.min_size_bytes);
        let message = if app.scan_done {
            format!("No gitignored artifacts >= {threshold} found.")
        } else {
            "Scanning...".to_string()
        };
        frame.render_widget(Paragraph::new(message), layout[1]);
        app.table_state.select(None);
    } else {
        app.ensure_selection_valid(options);

        let (size_label, age_label) = match app.sort_mode {
            SortMode::Age => ("Size", "Age*"),
            SortMode::Size => ("Size*", "Age"),
        };

        let header = Row::new(vec![
            Cell::from("Sel"),
            Cell::from(Text::from(size_label).alignment(Alignment::Right)),
            Cell::from(Text::from(age_label).alignment(Alignment::Right)),
            Cell::from("Repo"),
        ])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

        let widths = [
            Constraint::Length(3),
            Constraint::Length(11),
            Constraint::Length(6),
            Constraint::Min(10),
        ];

        let table = Table::new(visible_items, widths)
            .header(header)
            .column_spacing(1)
            .highlight_spacing(HighlightSpacing::Never)
            .row_highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            );
        frame.render_stateful_widget(table, layout[1], &mut app.table_state);
    }

    let footer = Paragraph::new(Text::from(vec![
        help_line(),
        Line::from(progress_line(app)),
    ]))
    .wrap(Wrap { trim: true });
    frame.render_widget(footer, layout[2]);
}

fn render_repo_row(item: &RepoItem, now: SystemTime) -> Row<'static> {
    let checkbox = if item.selected { "[x]" } else { "[ ]" };
    let bytes = item.report.total_size_bytes;
    let size = format_bytes(bytes);
    let age_days = repo_age_days(&item.report, now)
        .map(|d| format!("{d}d"))
        .unwrap_or_else(|| "-".to_string());

    Row::new(vec![
        Cell::from(checkbox.to_string()),
        Cell::from(Text::from(size).alignment(Alignment::Right)).style(size_style(bytes)),
        Cell::from(Text::from(age_days).alignment(Alignment::Right)),
        Cell::from(item.repo_display.clone()),
    ])
}

fn size_style(bytes: u64) -> Style {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    const BRIGHT_BYTES: u64 = 100 * MIB;
    const LOUD_BYTES: u64 = GIB;
    const EXTRA_BOLD_BYTES: u64 = 10 * GIB;

    if bytes >= EXTRA_BOLD_BYTES {
        Style::default()
            .fg(Color::LightRed)
            .add_modifier(Modifier::BOLD)
    } else if bytes >= LOUD_BYTES {
        Style::default().fg(Color::LightRed)
    } else if bytes >= BRIGHT_BYTES {
        Style::default().fg(Color::LightYellow)
    } else {
        Style::default()
    }
}

fn render_confirm(
    frame: &mut Frame,
    scan_root: &Path,
    options: &TuiOptions,
    confirm: &ConfirmData,
) {
    let area = frame.area();
    let message = confirm_message(scan_root, options, confirm);
    let popup = centered_rect(80, 40, area);

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(message)
            .block(Block::default().borders(Borders::ALL).title("Confirm"))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn render_cleaning(
    frame: &mut Frame,
    scan_root: &Path,
    options: &TuiOptions,
    cleaning: &CleaningData,
) {
    let area = frame.area();
    let popup = centered_rect(90, 40, area);

    let elapsed = cleaning.started_at.elapsed();
    let elapsed = if elapsed.as_secs() == 0 {
        format!("{}ms", elapsed.as_millis())
    } else {
        format!("{:.1}s", elapsed.as_secs_f64())
    };

    let dry_run_label = if options.dry_run { " (dry run)" } else { "" };
    let cancel_label = if cleaning.cancel_requested {
        " cancel requested"
    } else {
        ""
    };

    let current = cleaning
        .current
        .as_deref()
        .unwrap_or("starting...")
        .to_string();

    let text = Text::from(vec![
        Line::from(format!("root: {}", scan_root.display())),
        Line::from(format!(
            "plan: {} dirs, reclaim {}{}",
            cleaning.total,
            format_bytes(cleaning.planned_bytes),
            dry_run_label
        )),
        Line::from(format!(
            "progress: {}/{}  deleted: {} ({})  skipped: {}  errors: {}  elapsed: {}{}",
            cleaning.processed,
            cleaning.total,
            cleaning.deleted_paths,
            format_bytes(cleaning.deleted_bytes),
            cleaning.skipped_paths,
            cleaning.error_count,
            elapsed,
            cancel_label
        )),
        Line::from(""),
        Line::from(format!("current: {current}")),
        Line::from(""),
        Line::from("Press Ctrl+C to cancel."),
    ]);

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("Cleaning"))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn render_result(frame: &mut Frame, scan_root: &Path, app: &App) {
    let area = frame.area();
    let popup = centered_rect(80, 60, area);
    frame.render_widget(Clear, popup);

    let text = app
        .result_lines
        .iter()
        .map(|line| Line::from(line.as_str()))
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(Text::from(text))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!("Result ({})", scan_root.display())),
            )
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn confirm_message(scan_root: &Path, options: &TuiOptions, confirm: &ConfirmData) -> Text<'static> {
    let dry_run_label = if options.dry_run { " (dry run)" } else { "" };
    let lines = vec![
        Line::from(format!("root: {}", scan_root.display())),
        Line::from(format!(
            "plan: delete {} artifact dirs from {} repos, reclaim {}{}",
            confirm.planned_dirs,
            confirm.selected_repos,
            format_bytes(confirm.planned_bytes),
            dry_run_label
        )),
        Line::from(""),
        Line::from("Press 'y' to confirm, 'n' to cancel."),
    ];

    Text::from(lines)
}

fn format_delete_summary(
    scan_root: &Path,
    summary: &DeleteSummary,
    dry_run: bool,
    canceled: bool,
) -> Vec<String> {
    let dry_run_label = if dry_run { " (dry run)" } else { "" };

    let mut lines = Vec::new();
    lines.push(format!("root: {}", scan_root.display()));
    if canceled {
        lines.push("status: canceled".to_string());
    }
    lines.push(format!(
        "planned: {} dirs, reclaim {}{}",
        summary.planned_paths,
        format_bytes(summary.planned_bytes),
        dry_run_label
    ));
    lines.push(format!(
        "deleted: {} dirs, reclaimed {}",
        summary.deleted_paths,
        format_bytes(summary.deleted_bytes)
    ));
    lines.push(format!("skipped: {} dirs", summary.skipped_paths));

    if !summary.errors.is_empty() {
        lines.push(String::new());
        lines.push(format!("errors ({}):", summary.errors.len()));
        for (path, err) in &summary.errors {
            lines.push(format!("- {}: {err}", display_rel_path(scan_root, path)));
        }
    }

    lines.push(String::new());
    lines.push("Press any key to exit.".to_string());
    lines
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1]);

    horizontal[1]
}

fn repo_age_days(report: &RepoReport, now: SystemTime) -> Option<u64> {
    let newest = report.newest_mtime?;
    now.duration_since(newest)
        .ok()
        .map(|d| d.as_secs() / (24 * 60 * 60))
}

fn cmp_time_key(a: Option<SystemTime>, b: Option<SystemTime>) -> CmpOrdering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => CmpOrdering::Less,
        (None, Some(_)) => CmpOrdering::Greater,
        (None, None) => CmpOrdering::Equal,
    }
}

fn is_visible(report: &RepoReport, options: &TuiOptions) -> bool {
    report.total_size_bytes >= options.min_size_bytes && !report.artifacts.is_empty()
}

fn should_auto_select(report: &RepoReport, options: &TuiOptions, now: SystemTime) -> bool {
    const AUTO_SELECT_DAYS: u64 = 180;

    if report.total_size_bytes < options.min_size_bytes || report.artifacts.is_empty() {
        return false;
    }

    let Some(age_days) = repo_age_days(report, now) else {
        return false;
    };

    age_days >= AUTO_SELECT_DAYS
}

fn summarize_selection(items: &[RepoItem], options: &TuiOptions) -> (usize, u64, usize) {
    let mut planned_dirs = 0usize;
    let mut reclaim_bytes = 0u64;
    let mut selected_repos = 0usize;

    for item in items {
        if !is_visible(&item.report, options) {
            continue;
        }

        if !item.selected {
            continue;
        }
        selected_repos += 1;
        planned_dirs += item.report.artifacts.len();
        reclaim_bytes = reclaim_bytes.saturating_add(item.report.total_size_bytes);
    }

    (planned_dirs, reclaim_bytes, selected_repos)
}

fn progress_line(app: &App) -> String {
    let elapsed = app
        .scan_elapsed_final
        .unwrap_or_else(|| app.scan_started_at.elapsed());
    let elapsed_ms = elapsed.as_millis();
    let elapsed = if elapsed_ms < 1000 {
        format!("{elapsed_ms}ms")
    } else {
        format!("{:.1}s", elapsed.as_secs_f64())
    };

    let done = if app.scan_done { " done" } else { "" };

    match app.scan_total {
        Some(total) => format!(
            "scan: {}/{} candidates  repos: {}  artifacts: {}  elapsed: {}{}",
            app.scan_processed,
            total,
            app.items.len(),
            app.artifacts_found,
            elapsed,
            done
        ),
        None => format!(
            "scan: discovering candidates  repos: {}  artifacts: {}  elapsed: {}{}",
            app.items.len(),
            app.artifacts_found,
            elapsed,
            done
        ),
    }
}

fn help_line() -> Line<'static> {
    let key_style = Style::default().fg(Color::LightBlue);
    Line::from(vec![
        Span::styled("↑/↓", key_style),
        Span::raw(" move  "),
        Span::styled("Space", key_style),
        Span::raw(" toggle  "),
        Span::styled("a", key_style),
        Span::raw(" all  "),
        Span::styled("n", key_style),
        Span::raw(" none  "),
        Span::styled("Tab", key_style),
        Span::raw(" sort  "),
        Span::styled("⏎", key_style),
        Span::raw(" clean  "),
        Span::styled("q", key_style),
        Span::raw(" quit"),
    ])
}

fn spawn_clean_worker(
    targets: Vec<DeleteTarget>,
    dry_run: bool,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<AppEvent>,
) {
    thread::spawn(move || {
        let mut last_processed = 0usize;
        let total = targets.len();

        let summary = execute_delete_with_progress(
            &targets,
            dry_run,
            || cancel.load(Ordering::Relaxed),
            |progress| {
                last_processed = progress.processed;
                let idx = progress.processed.saturating_sub(1);
                let current = targets.get(idx).cloned().unwrap_or_else(|| DeleteTarget {
                    repo_root: PathBuf::new(),
                    path: PathBuf::new(),
                    planned_bytes: 0,
                });

                let _ = tx.send(AppEvent::Clean(CleanEvent::Progress { progress, current }));
            },
        );

        let canceled = cancel.load(Ordering::Relaxed) && last_processed < total;
        let _ = tx.send(AppEvent::Clean(CleanEvent::Finished { summary, canceled }));
    });
}

struct TerminalGuard {
    terminal: ratatui::Terminal<CrosstermBackend<std::io::Stdout>>,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("enable_raw_mode failed")?;

        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide).context("enter alternate screen failed")?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = ratatui::Terminal::new(backend).context("failed to create terminal")?;

        Ok(Self { terminal })
    }

    fn draw<F>(&mut self, f: F) -> Result<()>
    where
        F: FnOnce(&mut Frame),
    {
        self.terminal.draw(f).context("terminal draw failed")?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = execute!(stdout, Show, LeaveAlternateScreen);
    }
}
