//! Terminal UI (port of `packages/tui`).
//!
//! CONTRACT: oc-cli calls `oc_tui::run(agent, session, store)`.

pub mod input;
pub mod markdown;
pub mod perm;
pub mod state;
pub mod theme;
pub mod view;

use std::io::{stdout, Stdout};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyboardEnhancementFlags, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use oc_core::{Agent, AgentEvent, Session, SessionStore};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::input::{handle_key, Action};
use crate::perm::{AskMsg, ChannelAsker};
use crate::state::{AppState, Block, SessionPopup, ThemePopup};
use crate::theme::{Mode, Theme, ThemeCatalog};

type Term = Terminal<CrosstermBackend<Stdout>>;

enum Msg {
    Agent(u64, AgentEvent),
    Finished(u64, Option<String>),
    Info(String),
}

const HELP: &str = "Commands: /new  /sessions  /theme [name]  /share  /unshare  /help  /exit\n\
Keys: Enter send | Shift+Enter or Alt+Enter newline | PgUp/PgDn or mouse wheel scroll | \
Esc interrupt | Ctrl+O expand tool output | Ctrl+N new session | Ctrl+L sessions | Ctrl+C quit";

struct App {
    agent: Arc<Agent>,
    session: Arc<Mutex<Session>>,
    store: Arc<SessionStore>,
    state: AppState,
    theme: Theme,
    catalog: ThemeCatalog,
    mode: Mode,
    config: oc_core::Config,
    tx: UnboundedSender<Msg>,
    turn: u64,
    task: Option<JoinHandle<()>>,
}

impl App {
    fn new(
        mut agent: Agent,
        session: Session,
        store: SessionStore,
        tx: UnboundedSender<Msg>,
        perm_tx: UnboundedSender<AskMsg>,
    ) -> App {
        let config = oc_core::Config::load(&session.cwd).unwrap_or_default();
        let catalog = ThemeCatalog::load(&session.cwd);
        let mode = Mode::detect();
        let wanted = theme::load_saved_theme().or_else(|| config.theme.clone()).unwrap_or_else(|| theme::DEFAULT_THEME.into());
        let theme = catalog.get_or_default(&wanted, mode);
        agent.hooks.permission =
            Some(Arc::new(oc_core::permission::RulePermission::new(&config, Arc::new(ChannelAsker::new(perm_tx)))));
        let mut state = AppState::new(agent.model.clone(), session.cwd.display().to_string(), session.title.clone());
        state.load_session(&session);
        App {
            agent: Arc::new(agent),
            session: Arc::new(Mutex::new(session)),
            store: Arc::new(store),
            state,
            theme,
            catalog,
            mode,
            config,
            tx,
            turn: 0,
            task: None,
        }
    }

    fn submit(&mut self) {
        let text = self.state.editor.text();
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(cmd) = text.strip_prefix('/') {
            let (name, arg) = cmd.split_once(char::is_whitespace).map(|(n, a)| (n, a.trim())).unwrap_or((cmd, ""));
            if arg.is_empty() || matches!(name, "theme") {
                self.state.editor.clear();
                self.command(name, arg);
                return;
            }
        }
        if self.state.busy {
            self.state.blocks.push(Block::Info("Still working; press Esc to interrupt first.".into()));
            return;
        }
        self.state.editor.clear();
        self.state.blocks.push(Block::User(text.clone()));
        self.state.busy = true;
        self.state.scroll = 0;
        self.turn += 1;
        let turn = self.turn;
        let (agent, session, store, tx) = (self.agent.clone(), self.session.clone(), self.store.clone(), self.tx.clone());
        self.task = Some(tokio::spawn(async move {
            let (etx, mut erx) = unbounded_channel::<AgentEvent>();
            let fwd_tx = tx.clone();
            let fwd = tokio::spawn(async move {
                while let Some(ev) = erx.recv().await {
                    let _ = fwd_tx.send(Msg::Agent(turn, ev));
                }
            });
            let mut s = session.lock().await;
            if s.messages.is_empty() && s.title == "New session" {
                s.title = state::truncate(text.lines().next().unwrap_or(""), 60);
            }
            let res = agent.prompt(&mut s, text, etx).await;
            if let Err(e) = store.save(&s) {
                let _ = tx.send(Msg::Agent(turn, AgentEvent::Error { message: format!("failed to save session: {e}") }));
            }
            drop(s);
            let _ = fwd.await;
            let _ = tx.send(Msg::Finished(turn, res.err().map(|e| e.to_string())));
        }));
    }

    fn set_theme(&mut self, name: &str) -> bool {
        match self.catalog.get(name, self.mode) {
            Ok(t) => {
                self.theme = t;
                true
            }
            Err(e) => {
                self.state.blocks.push(Block::Error(format!("theme {name}: {e:#}")));
                false
            }
        }
    }

    fn open_theme_picker(&mut self) {
        let names = self.catalog.names();
        let selected = names.iter().position(|n| *n == self.theme.name).unwrap_or(0);
        self.state.theme_popup = Some(ThemePopup { names, selected, original: self.theme.name.clone() });
    }

    fn theme_popup_key(&mut self, key: KeyEvent) {
        let Some(p) = self.state.theme_popup.as_mut() else { return };
        let before = p.selected;
        match key.code {
            KeyCode::Up => p.selected = p.selected.saturating_sub(1),
            KeyCode::Down => p.selected = (p.selected + 1).min(p.names.len().saturating_sub(1)),
            KeyCode::Esc => {
                let orig = p.original.clone();
                self.state.theme_popup = None;
                self.theme = self.catalog.get_or_default(&orig, self.mode);
                return;
            }
            KeyCode::Enter => {
                let name = p.names.get(p.selected).cloned();
                self.state.theme_popup = None;
                if let Some(n) = name {
                    if self.set_theme(&n) {
                        theme::save_theme(&n);
                    }
                }
                return;
            }
            _ => {}
        }
        if p.selected != before {
            let name = p.names[p.selected].clone();
            if let Ok(t) = self.catalog.get(&name, self.mode) {
                self.theme = t;
            }
        }
    }

    fn share(&mut self, unshare: bool) {
        let (session, store, config, tx) = (self.session.clone(), self.store.clone(), self.config.clone(), self.tx.clone());
        self.state.blocks.push(Block::Info(if unshare { "Unsharing session…" } else { "Sharing session…" }.into()));
        tokio::spawn(async move {
            let snapshot = session.lock().await.clone();
            let _ = store.save(&snapshot);
            let msg = if unshare {
                match oc_core::share::unshare_session(&config, &snapshot.id).await {
                    Ok(()) => "Session unshared.".to_string(),
                    Err(e) => format!("Unshare failed: {e:#}"),
                }
            } else {
                match oc_core::share::share_session(&config, &snapshot).await {
                    Ok(url) => format!("Shared: {url}"),
                    Err(e) => format!("Share failed: {e:#}"),
                }
            };
            let _ = tx.send(Msg::Info(msg));
        });
    }

    fn command(&mut self, cmd: &str, arg: &str) {
        match cmd {
            "theme" if arg.is_empty() => self.open_theme_picker(),
            "theme" => {
                if self.catalog.names().iter().any(|n| n == arg) {
                    if self.set_theme(arg) {
                        theme::save_theme(arg);
                        self.state.blocks.push(Block::Info(format!("Theme: {arg}")));
                    }
                } else {
                    self.state.blocks.push(Block::Error(format!("Unknown theme {arg}. Use /theme to pick one.")));
                }
            }
            "share" => self.share(false),
            "unshare" => self.share(true),
            "new" => self.new_session(),
            "sessions" => self.open_sessions(),
            "help" => self.state.blocks.push(Block::Info(HELP.into())),
            "exit" | "quit" => self.state.should_quit = true,
            other => self.state.blocks.push(Block::Info(format!("Unknown command /{other}. Try /help."))),
        }
    }

    fn interrupt(&mut self) {
        if !self.state.busy {
            return;
        }
        if let Some(t) = self.task.take() {
            t.abort();
        }
        self.turn += 1; // ignore stale events
        self.state.perm.clear();
        self.state.busy = false;
        self.state.blocks.push(Block::Info("Interrupted.".into()));
        let (session, store) = (self.session.clone(), self.store.clone());
        tokio::spawn(async move {
            let s = session.lock().await;
            let _ = store.save(&s);
        });
    }

    fn switch_to(&mut self, s: Session) {
        if self.state.busy {
            self.interrupt();
        }
        self.state.load_session(&s);
        self.state.cwd = s.cwd.display().to_string();
        self.session = Arc::new(Mutex::new(s));
    }

    fn new_session(&mut self) {
        let cwd = self.session.try_lock().map(|s| s.cwd.clone()).unwrap_or_else(|_| ".".into());
        self.switch_to(Session::new(cwd));
    }

    fn open_sessions(&mut self) {
        match self.store.list() {
            Ok(mut items) => {
                items.reverse();
                self.state.popup = Some(SessionPopup { items, selected: 0 });
            }
            Err(e) => self.state.blocks.push(Block::Error(format!("cannot list sessions: {e}"))),
        }
    }

    fn popup_key(&mut self, key: KeyEvent) {
        let Some(p) = self.state.popup.as_mut() else { return };
        match key.code {
            KeyCode::Esc => self.state.popup = None,
            KeyCode::Up => p.selected = p.selected.saturating_sub(1),
            KeyCode::Down => p.selected = (p.selected + 1).min(p.items.len().saturating_sub(1)),
            KeyCode::Enter => {
                let chosen = p.items.get(p.selected).cloned();
                self.state.popup = None;
                if let Some(c) = chosen {
                    let loaded = self.store.load(&c.id).unwrap_or(c);
                    self.switch_to(loaded);
                }
            }
            _ => {}
        }
    }

    fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) => {
                if self.state.perm.handle_key(key) {
                    return;
                }
                if self.state.theme_popup.is_some() {
                    if key.kind != event::KeyEventKind::Release {
                        self.theme_popup_key(key);
                    }
                    return;
                }
                if self.state.popup.is_some() {
                    if key.kind != event::KeyEventKind::Release {
                        self.popup_key(key);
                    }
                    return;
                }
                match handle_key(&mut self.state.editor, key) {
                    Action::Submit => self.submit(),
                    Action::Quit => self.state.should_quit = true,
                    Action::Interrupt => self.interrupt(),
                    Action::NewSession => self.new_session(),
                    Action::SessionList => self.open_sessions(),
                    Action::ScrollUp(n) => self.state.scroll = self.state.scroll.saturating_add(n),
                    Action::ScrollDown(n) => self.state.scroll = self.state.scroll.saturating_sub(n),
                    Action::ToggleTools => self.state.expand_tools = !self.state.expand_tools,
                    Action::Newline | Action::Edit | Action::None => {}
                }
            }
            Event::Mouse(m) => match m.kind {
                MouseEventKind::ScrollUp => self.state.scroll = self.state.scroll.saturating_add(3),
                MouseEventKind::ScrollDown => self.state.scroll = self.state.scroll.saturating_sub(3),
                _ => {}
            },
            Event::Paste(s) => self.state.editor.insert_str(&s),
            _ => {}
        }
    }

    fn on_msg(&mut self, m: Msg) {
        match m {
            Msg::Agent(t, ev) if t == self.turn => {
                self.state.title = self.session.try_lock().map(|s| s.title.clone()).unwrap_or_else(|_| self.state.title.clone());
                self.state.apply(ev);
            }
            Msg::Info(s) => self.state.blocks.push(Block::Info(s)),
            Msg::Finished(t, err) if t == self.turn => {
                self.state.perm.clear();
                if let Some(e) = err {
                    self.state.blocks.push(Block::Error(e));
                }
                self.state.busy = false;
                if let Ok(s) = self.session.try_lock() {
                    self.state.title = s.title.clone();
                }
            }
            _ => {}
        }
    }
}

fn setup_terminal() -> anyhow::Result<Term> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture, crossterm::event::EnableBracketedPaste)?;
    // Lets terminals report Shift+Enter distinctly; ignore if unsupported.
    let _ = execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

fn restore_terminal() {
    let mut out = stdout();
    let _ = execute!(out, PopKeyboardEnhancementFlags);
    let _ = execute!(out, crossterm::event::DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

struct TermGuard;
impl Drop for TermGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn spawn_input_thread() -> UnboundedReceiver<Event> {
    let (tx, rx) = unbounded_channel();
    std::thread::spawn(move || loop {
        if tx.is_closed() {
            break;
        }
        match event::poll(Duration::from_millis(100)) {
            Ok(true) => match event::read() {
                Ok(ev) => {
                    if tx.send(ev).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            },
            Ok(false) => {}
            Err(_) => break,
        }
    });
    rx
}

pub async fn run(agent: Agent, session: Session, store: SessionStore) -> anyhow::Result<()> {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev_hook(info);
    }));
    let mut terminal = setup_terminal()?;
    let _guard = TermGuard;

    let (tx, mut rx) = unbounded_channel::<Msg>();
    let (perm_tx, mut perm_rx) = unbounded_channel::<AskMsg>();
    let mut app = App::new(agent, session, store, tx, perm_tx);
    let mut input = spawn_input_thread();
    let mut ticker = tokio::time::interval(Duration::from_millis(100));

    while !app.state.should_quit {
        terminal.draw(|f| view::draw(f, &mut app.state, &app.theme))?;
        tokio::select! {
            Some(ev) = input.recv() => {
                app.on_event(ev);
                // drain queued input so fast typing/scrolling stays responsive
                while let Ok(ev) = input.try_recv() {
                    app.on_event(ev);
                }
            }
            Some(m) = rx.recv() => app.on_msg(m),
            Some(m) = perm_rx.recv() => app.state.perm.push(m),
            _ = ticker.tick() => {
                app.state.tick = app.state.tick.wrapping_add(1);
                app.state.perm.prune_closed();
            }
        }
    }
    if let Some(t) = app.task.take() {
        t.abort();
    }
    Ok(())
}
