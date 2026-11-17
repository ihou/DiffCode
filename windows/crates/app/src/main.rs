//! DiffCode for Windows. On the Mac this builds a headless harness that renders frames to PNG files instead,
//! so the interface can be checked without a Windows machine.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod gallery;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
mod win;

#[cfg(target_os = "macos")]
fn main() {
    use diffcode_ui::geom::{Point, Size};
    use diffcode_ui::input::Event;
    use diffcode_ui::theme::Theme;
    use diffcode_ui::ui::{Ui, UiState};

    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let dark = args.iter().any(|a| a == "--dark");
    let size = Size::new(1100.0, 700.0);
    let backend = mac::MacBackend::new();
    let theme = Theme::new(dark, 13.0, 13.0);
    if let Some(out) = flag("--gallery") {
        let mut st = UiState::default();
        let mut g = gallery::Gallery::default();
        let hover = flag("--mouse").and_then(|m| {
            let (x, y) = m.split_once(',')?;
            Some(Point::new(x.parse().ok()?, y.parse().ok()?))
        });
        // Two frames: the first lays out what the mouse can hit, the second is drawn with hover state.
        for frame in 0..2 {
            let event = match (frame, hover) {
                (1, Some(pos)) => Some(Event::MouseMove(pos)),
                _ => None,
            };
            if let Some(e) = &event {
                st.observe(e);
            }
            let mut painter = mac::MacPainter::new(&backend, size, 2.0);
            let mut ui = Ui::begin(&mut st, &mut painter, &backend, &theme, event, size);
            gallery::draw(&mut ui, &mut g);
            ui.finish();
            if frame == 1 && !painter.write_png(std::path::Path::new(&out)) {
                eprintln!("could not write {out}");
                std::process::exit(1);
            }
        }
        return;
    }
    if let Some(out) = flag("--diff") {
        render_diff(&args, &out, dark);
        return;
    }
    if let Some(out) = flag("--app") {
        render_app(&args, &out, dark);
        return;
    }
    eprintln!("usage: DiffCode --gallery out.png [--dark] [--mouse x,y]");
    eprintln!("       DiffCode --app out.png --project <repo>... [--show quick|files|actions|branches|find|keymap|settings|graph|dialog|log|goto|terminal|terminals] [--type text] [--key name] [--mouse x,y] [--click x,y] [--rclick x,y] [--drag x,y:x,y] [--files-over x,y] [--size WxH] [--zh] [--dark]");
    eprintln!("       DiffCode --diff out.png --root <repo> --path <file> [--commit <hash>] [--unified] [--edit] [--ticks] [--revert] [--dark]");
}

/// Renders one file's diff in the code canvas, the way the viewer shows it.
#[cfg(target_os = "macos")]
fn render_diff(args: &[String], out: &str, dark: bool) {
    use diffcode_ui::canvas::{CodeCanvas, CommitTicks, InitialPosition, Mode};
    use diffcode_ui::core::diff::{self, DiffRequest, Source};
    use diffcode_ui::core::editor_session::EditorSession;
    use diffcode_ui::geom::{Rect, Size};
    use diffcode_ui::input::Event;
    use diffcode_ui::theme::Theme;
    use diffcode_ui::ui::{Id, Ui, UiState};

    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let has = |name: &str| args.iter().any(|a| a == name);
    let root = std::path::PathBuf::from(flag("--root").expect("--root"));
    let path = flag("--path").expect("--path");
    let source = match flag("--commit") {
        Some(hash) => Source::Commit { parent: Some(format!("{hash}^")), hash },
        None => Source::Worktree { base: "HEAD".into() },
    };
    let request = DiffRequest { root: root.clone(), path: path.clone(), old_path: None, kind: None, source, ignore_whitespace: false };
    let doc = diff::load(&request);
    let editor = if has("--edit") { EditorSession::new(&doc, root, path, false) } else { None };
    let mut canvas = CodeCanvas::new();
    let mode = if has("--unified") { Mode::Unified } else { Mode::SideBySide };
    canvas.set_document(Some(doc), mode, false);
    canvas.editor = editor;
    canvas.shows_revert_buttons = has("--revert");
    if has("--ticks") {
        let mut ticks = CommitTicks { included: true, ..Default::default() };
        // The second change's first new line stays out, to show a partly ticked change.
        if let Some(h) = canvas.document().and_then(|d| d.hunks().get(1).cloned()) {
            ticks.added.insert(h.right_start);
        }
        canvas.set_commit_ticks(Some(ticks));
    }
    canvas.position_when_laid_out(InitialPosition::Change { last: false });
    let size = Size::new(1100.0, 700.0);
    let backend = mac::MacBackend::new();
    let theme = Theme::new(dark, 13.0, 13.0);
    let mut st = UiState::default();
    let id = Id::new("canvas");
    st.focus = Some(id);
    let typed = flag("--type");
    for frame in 0..3 {
        let event = match (frame, &typed) {
            (1, Some(t)) => Some(Event::Text(t.clone())),
            _ => None,
        };
        let mut painter = mac::MacPainter::new(&backend, size, 2.0);
        let mut ui = Ui::begin(&mut st, &mut painter, &backend, &theme, event, size);
        struct NoClipboard;
        impl diffcode_ui::text_field::Clipboard for NoClipboard {
            fn get(&mut self) -> Option<String> {
                None
            }
            fn set(&mut self, _: &str) {}
        }
        canvas.handle_input(&mut ui, &mut NoClipboard);
        let actions = canvas.show(&mut ui, id, Rect::new(0.0, 0.0, size.w, size.h), true);
        if !actions.is_empty() {
            eprintln!("actions: {actions:?}");
        }
        ui.finish();
        if frame == 2 && !painter.write_png(std::path::Path::new(out)) {
            eprintln!("could not write {out}");
            std::process::exit(1);
        }
    }
}

/// Runs the whole program for a few seconds against real repositories and renders the last frame.
#[cfg(target_os = "macos")]
#[allow(clippy::type_complexity)]
fn render_app(args: &[String], out: &str, dark: bool) {
    use diffcode_ui::app::search::QuickOpenMode;
    use diffcode_ui::app::settings::{Appearance, Settings};
    use diffcode_ui::app::{App, Popup};
    use diffcode_ui::geom::{Point, Size};
    use diffcode_ui::input::{Event, Key, Modifiers, MouseButton};
    use diffcode_ui::platform::{Platform, Program, Waker};
    use diffcode_ui::text_field::Clipboard;
    use diffcode_ui::ui::{Ui, UiState};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    struct Clip(Option<String>);
    impl Clipboard for Clip {
        fn get(&mut self) -> Option<String> {
            self.0.clone()
        }
        fn set(&mut self, text: &str) {
            self.0 = Some(text.to_string());
        }
    }
    /// The clipboard, and the reply a fake GitHub gives to the update check (`--update-json <file>`).
    struct Harness(Clip, Option<String>);
    impl Platform for Harness {
        fn clipboard(&mut self) -> &mut dyn Clipboard {
            &mut self.0
        }
        fn pick_folder(&mut self, _: &str) -> Option<PathBuf> {
            None
        }
        fn open_url(&mut self, url: &str) {
            eprintln!("open {url}");
        }
        fn reveal(&mut self, path: &Path) {
            eprintln!("reveal {}", path.display());
        }
        fn waker(&self) -> Waker {
            std::sync::Arc::new(|| {})
        }
        fn set_title(&mut self, _: &str) {}
        fn quit(&mut self) {}
        fn memory_mb(&self) -> f64 {
            42.0
        }
        // Drawn as on Windows, where the interface has the title bar.
        fn caption_buttons(&self) -> bool {
            true
        }
        fn http_get(&self) -> Option<diffcode_ui::platform::HttpGet> {
            let json = self.1.clone()?;
            Some(std::sync::Arc::new(move |_: &str| Ok(json.clone())))
        }
        fn app_icon(&self, pixels: u32) -> Option<std::sync::Arc<Vec<u8>>> {
            let ico = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/res/DiffCode.ico")).ok()?;
            diffcode_ui::paint::png_in_ico(&ico, pixels).map(|png| std::sync::Arc::new(png.to_vec()))
        }
        fn monospace_families(&self) -> (String, Vec<String>) {
            let (default, mut list) = mac::monospace_families();
            // A long list, to see the menu scroll: HARNESS_EXTRA_FONTS=40.
            let extra = std::env::var("HARNESS_EXTRA_FONTS").ok().and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
            list.extend((1..=extra).map(|i| format!("Test Mono {i:02}")));
            (default, list)
        }
    }

    let values = |name: &str| -> Vec<String> { args.windows(2).filter(|w| w[0] == name).map(|w| w[1].clone()).collect() };
    let flag = |name: &str| values(name).into_iter().next();
    let point = |text: String| -> Option<Point> {
        let (x, y) = text.split_once(',')?;
        Some(Point::new(x.parse().ok()?, y.parse().ok()?))
    };
    let size = flag("--size")
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some(Size::new(w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or(Size::new(1280.0, 820.0));
    diffcode_ui::core::lang::set_chinese(args.iter().any(|a| a == "--zh"));
    let mut settings = Settings::default();
    settings.projects = values("--project").into_iter().map(PathBuf::from).collect();
    settings.appearance = if dark { Appearance::Dark } else { Appearance::Light };
    settings.code_font = flag("--code-font");
    settings.terminal_font = flag("--terminal-font");
    let mut app = App::new(settings);
    let backend = mac::MacBackend::new();
    let mut st = UiState::default();
    let mut platform = Harness(Clip(None), flag("--update-json").and_then(|p| std::fs::read_to_string(p).ok()));

    let mut run = |app: &mut App, st: &mut UiState, event: Option<Event>, write: bool| {
        if let Some(e) = &event {
            st.observe(e);
        }
        let theme = app.theme(dark);
        let mut painter = mac::MacPainter::new(&backend, size, 2.0);
        let mut ui = Ui::begin(st, &mut painter, &backend, &theme, event, size);
        app.frame(&mut ui, &mut platform);
        ui.finish();
        if write && !painter.write_png(Path::new(out)) {
            eprintln!("could not write {out}");
            std::process::exit(1);
        }
    };
    let settle = |app: &mut App, st: &mut UiState, run: &mut dyn FnMut(&mut App, &mut UiState, Option<Event>, bool), ms: u64| {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            run(app, st, None, false);
            std::thread::sleep(Duration::from_millis(25));
        }
    };
    settle(&mut app, &mut st, &mut run, 2500);
    match flag("--show").as_deref() {
        Some("quick") => app.show_quick_open(QuickOpenMode::All),
        Some("files") => app.show_quick_open(QuickOpenMode::Files),
        Some("actions") => app.show_quick_open(QuickOpenMode::Actions),
        Some("branches") => app.show_quick_open(QuickOpenMode::Branches),
        Some("find") => app.show_find_in_path(),
        Some("keymap") => app.present(Popup::Keymap),
        Some("settings") => app.present(Popup::Settings),
        Some("update") => app.show_update_dialog(),
        Some("graph") => app.toggle_git_graph(),
        Some("goto") => app.show_go_to_line(),
        Some("log") => app.toggle_tab(diffcode_ui::app::project::ProjectTab::Log),
        Some("terminal") => app.toggle_terminal(),
        Some("terminal-empty") => app.terminals.visible = true,
        Some("terminals") => {
            app.toggle_terminal();
            app.open_terminal_in_current_project();
        }
        Some("dialog") => {
            let changes: Vec<_> = app.current_project().map(|p| p.status.changes.clone()).unwrap_or_default();
            let request = diffcode_ui::app::dialog::DialogRequest::new(
                diffcode_ui::app::dialog::DialogStyle::Destructive,
                diffcode_ui::icons::Icon::ArrowUndo,
                "Roll back 3 files?",
                "Roll Back",
            )
            .message("Their changes are thrown away. This cannot be undone.")
            .with_files(&changes, 4);
            app.ask(request);
        }
        _ => {}
    }
    settle(&mut app, &mut st, &mut run, 600);
    // Typing and keys in the order given.
    let steps: Vec<(String, String)> =
        args.windows(2).filter(|w| w[0] == "--type" || w[0] == "--key").map(|w| (w[0].clone(), w[1].clone())).collect();
    for (kind, value) in steps {
        if kind == "--type" {
            run(&mut app, &mut st, Some(Event::Text(value)), false);
            settle(&mut app, &mut st, &mut run, 1200);
            continue;
        }
        let key = match value.as_str() {
            "down" => Key::Down,
            "up" => Key::Up,
            "enter" => Key::Enter,
            "tab" => Key::Tab,
            "escape" => Key::Escape,
            "ctrl-a" | "ctrl-c" | "ctrl-k" | "ctrl-l" => Key::Char(value.chars().last().unwrap()),
            _ => continue,
        };
        let mods = if value.starts_with("ctrl-") { Modifiers::CTRL } else { Modifiers::NONE };
        run(&mut app, &mut st, Some(Event::Key { key, mods, repeat: false }), false);
        settle(&mut app, &mut st, &mut run, 400);
    }
    for pos in values("--click").into_iter().filter_map(point) {
        run(&mut app, &mut st, Some(Event::MouseMove(pos)), false);
        run(&mut app, &mut st, Some(Event::MouseDown { pos, button: MouseButton::Left, clicks: 1, mods: Modifiers::NONE }), false);
        run(&mut app, &mut st, Some(Event::MouseUp { pos, button: MouseButton::Left, mods: Modifiers::NONE }), false);
        settle(&mut app, &mut st, &mut run, 800);
    }
    for pos in values("--rclick").into_iter().filter_map(point) {
        run(&mut app, &mut st, Some(Event::MouseMove(pos)), false);
        run(&mut app, &mut st, Some(Event::MouseDown { pos, button: MouseButton::Right, clicks: 1, mods: Modifiers::NONE }), false);
        run(&mut app, &mut st, Some(Event::MouseUp { pos, button: MouseButton::Right, mods: Modifiers::NONE }), false);
        settle(&mut app, &mut st, &mut run, 300);
    }
    for drag in values("--drag") {
        let Some((a, b)) = drag.split_once(':') else { continue };
        let (Some(a), Some(b)) = (point(a.to_string()), point(b.to_string())) else { continue };
        run(&mut app, &mut st, Some(Event::MouseMove(a)), false);
        run(&mut app, &mut st, Some(Event::MouseDown { pos: a, button: MouseButton::Left, clicks: 1, mods: Modifiers::NONE }), false);
        for i in 1..=8 {
            let f = i as f32 / 8.0;
            run(&mut app, &mut st, Some(Event::MouseMove(Point::new(a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f))), false);
        }
        run(&mut app, &mut st, Some(Event::MouseUp { pos: b, button: MouseButton::Left, mods: Modifiers::NONE }), false);
        settle(&mut app, &mut st, &mut run, 300);
    }
    if let Some(pos) = flag("--files-over").and_then(point) {
        run(&mut app, &mut st, Some(Event::FilesHover(Some(pos))), false);
    }
    if let Some(pos) = flag("--mouse").and_then(point) {
        run(&mut app, &mut st, Some(Event::MouseMove(pos)), false);
        // Long enough for a tooltip.
        settle(&mut app, &mut st, &mut run, 900);
    }
    run(&mut app, &mut st, None, true);
}

#[cfg(windows)]
fn main() {
    use diffcode_ui::app::settings::Settings;
    use diffcode_ui::app::App;

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--gallery") {
        win::run(Box::new(gallery::GalleryProgram::default()));
        return;
    }
    // One DiffCode at a time: a second launch brings the first one forward.
    let relaunch = args.iter().any(|a| a == "--relaunch");
    let Some(_instance) = win::single_instance(relaunch) else { return };
    let dir = std::env::var_os("APPDATA").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(".")).join("DiffCode");
    let _ = std::fs::create_dir_all(&dir);
    let settings = Settings::load(&dir.join("settings.ini"));
    // The language is fixed for the run, before any text is made.
    let system_chinese = win::system_chinese();
    diffcode_ui::core::lang::set_chinese(settings.language.is_chinese(system_chinese));
    let mut app = App::new(settings);
    app.system_chinese = system_chinese;
    win::run(Box::new(app));
}

#[cfg(not(any(windows, target_os = "macos")))]
fn main() {}
