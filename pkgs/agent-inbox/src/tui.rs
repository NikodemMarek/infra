//! The terminal UI: `App` is the state and key handling (pure, returns an `Effect` for the loop
//! to carry out), `render` draws it, `run` is the loop around the router client.
//!
//! Layout: a header line, the list of items next to (below, on narrow terminals) a detail pane
//! for the selected item, and a footer with key hints or the last status message. `f` cycles
//! through All, the configured filter tabs (`Tab`, which filter the items by the kind of their
//! origin) and the Sessions tab: the same layout over the live Claude sessions (list and detail
//! of the selected session; Enter focuses the detail like an item's, `s` lists the session's
//! items), all of them whatever their origin. `N` (everywhere) asks for a task
//! in a popup (Ctrl+J adds a line) and starts a new agent for it (`jw agent -d`). Only the events
//! and sessions of agents with an origin (a merge request, an issue or a task, see `Origin`) are
//! shown, with it as their context; `o` opens its link (`xdg-open`).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use serde_json::{json, Map, Value};
use unicode_width::UnicodeWidthChar;

use crate::client::{self, clean, Item, Origin, Question};
use crate::reply::{self, ReplyError};
use crate::sessions::{self, Session};

const POLL: Duration = Duration::from_millis(250);
const REFRESH: Duration = Duration::from_secs(1);
/// Seconds a status message stays in the footer.
const STATUS_TTL: f64 = 4.0;
/// Seconds after the last press of a key that opened an item (or the next question) during
/// which that same key is ignored: a held key repeats after a delay of up to ~0.7 s, then
/// every ~30 ms.
const KEY_GUARD: f64 = 1.0;

/// Terminals at least this wide show list and detail side by side, narrower ones stacked.
const SIDE_BY_SIDE: u16 = 100;
const LIST_MIN_WIDTH: u16 = 30;
/// Width of the kind badge column in the list.
const BADGE_WIDTH: usize = 5;
/// Widest context (merge request and title, or place) column in the lists.
const CONTEXT_MAX_WIDTH: usize = 32;
/// The status of a session that waits for the user.
const WAITING: &str = "waiting";
/// Longest multi-line value (a command, ...) shown in the detail pane.
const MAX_LINES: usize = 40;
/// Lines of the last message of a waiting session shown in the session detail.
const WAIT_BODY_LINES: usize = 5;
/// Longest line of an origin description or prompt shown in the detail pane.
const ORIGIN_LINE_CHARS: usize = 500;

/// Decisions for the hook behind a claude.* item (one JSON object line).
const ALLOW: &str = r#"{"behavior":"allow"}"#;
const DENY: &str = r#"{"behavior":"deny"}"#;

#[derive(Debug, PartialEq)]
enum Effect {
    None,
    Quit,
    Dismiss(String),
    /// Every item without a reply target (those are waiting for an answer).
    DismissAll,
    /// Answer item `.0` with the line `.1`.
    Send(String, String),
    /// Switch to (or open) the zellij tab of shpool session `.0`.
    Attach(String),
    /// SIGTERM to the Claude process `pid` (started at `start`), then dismiss the items of its
    /// session.
    Kill {
        pid: u32,
        start: u64,
    },
    /// Start a new agent for the task `.0` with `jw agent -d` (empty: pick a repo; needs the
    /// terminal: done by `event_loop`).
    NewAgent(String),
    /// Open the http(s) link `.0` with `xdg-open`.
    Open(String),
}

#[derive(Default)]
enum Mode {
    /// Focus in the list; the detail pane shows the selected item.
    #[default]
    List,
    /// Focus in the detail pane (scrolling).
    Detail { id: String, scroll: u16 },
    /// Focus in the detail pane of the session of process `pid` (scrolling).
    SessionDetail { pid: u32, scroll: u16 },
    /// Masked input; the buffer is moved out into `Effect::Send` on Enter.
    Password { id: String, buf: String },
    /// Choosing the answers of a claude.question item.
    Answer { id: String, st: Answering },
    /// One key (`y`) confirms dismissing every item without a reply target, any other cancels.
    ConfirmDismissAll,
    /// The task for a new agent being typed (footer).
    Task { buf: String },
}

/// Progress through the questions of one item.
#[derive(Default)]
struct Answering {
    /// The question being answered.
    idx: usize,
    /// Highlighted row: an option, or (last) "Other…".
    cursor: usize,
    /// Multi-select: the toggled options.
    picked: Vec<usize>,
    /// Free text being typed for "Other…".
    input: Option<String>,
    /// (question, answer) pairs given so far.
    done: Vec<(String, String)>,
}

struct Status {
    text: String,
    error: bool,
    /// Unix seconds, see `App::now`.
    at: f64,
}

/// A filter tab: shows the items whose origin kind is in `kinds`; without `kinds`, those whose
/// kind no tab lists.
#[derive(Debug, PartialEq)]
pub struct Tab {
    pub name: String,
    pub kinds: Option<Vec<String>>,
}

#[derive(Default)]
struct App {
    /// Newest first.
    items: Vec<Item>,
    /// Index into `visible()`.
    selected: usize,
    tabs: Vec<Tab>,
    /// The active tab: 0 = all, the `tabs` index + 1, and after those the Sessions tab.
    tab: usize,
    /// Show only the items of this Claude session (the tab stays the one it was entered from).
    session: Option<String>,
    /// Newest first.
    sessions: Vec<Session>,
    /// Index into `visible_sessions()`.
    session_sel: usize,
    /// The origin files seen so far (items and sessions) and what `Origin::load` made of them.
    origins: HashMap<String, Option<Origin>>,
    /// When (`now`) and by which key an item (or the next question) was opened, see `KEY_GUARD`.
    guard: Option<(f64, KeyCode)>,
    mode: Mode,
    /// Show the raw event instead of the summary in the detail pane.
    raw: bool,
    /// Result of the last action; hidden after `STATUS_TTL`.
    status: Option<Status>,
    /// Set while the router cannot be reached.
    offline: Option<String>,
    /// Unix seconds, for ages.
    now: f64,
    /// Terminal size, for scrolling the detail pane (set by the loop).
    area: Rect,
}

/// What can be done with an item; None = read-only.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Password,
    Permission,
    Question,
}

fn action(item: &Item) -> Option<Action> {
    item.reply()?;
    match item.kind() {
        "sudo.password_request" => Some(Action::Password),
        "claude.permission_request" => Some(Action::Permission),
        "claude.question" if !item.questions().is_empty() => Some(Action::Question),
        _ => None,
    }
}

fn action_hints(item: &Item) -> Vec<(&'static str, &'static str)> {
    match action(item) {
        Some(Action::Password) => vec![("p", "password")],
        Some(Action::Permission) => vec![("y", "allow"), ("n", "deny")],
        Some(Action::Question) => vec![("enter", "answer")],
        None => vec![],
    }
}

/// The answer to a multi-select question: the picked labels (in option order) and the free
/// text, if any, joined by ", ".
fn join_answer(q: &Question, picked: &[usize], other: Option<&str>) -> String {
    let mut picked = picked.to_vec();
    picked.sort_unstable();
    picked.dedup();
    let mut parts: Vec<&str> = picked
        .iter()
        .filter_map(|&i| q.labels.get(i))
        .map(String::as_str)
        .collect();
    parts.extend(other);
    parts.join(", ")
}

/// Toggles option `i` in the picked ones.
fn toggle(picked: &mut Vec<usize>, i: usize) {
    match picked.iter().position(|&p| p == i) {
        Some(pos) => {
            picked.remove(pos);
        }
        None => picked.push(i),
    }
}

impl App {
    /// `items` as the router returns them (oldest first). The selection follows its item.
    fn set_items(&mut self, mut items: Vec<Item>) {
        items.reverse();
        let followed = self.selected_item().map(|i| i.id.clone());
        self.items = items;
        // A session without items is over (but not while one of its items is open: the next
        // refresh after the list is back ends it).
        if self.visible().is_empty() && matches!(self.mode, Mode::List) {
            self.session = None;
        }
        self.select_id(followed);
        // The mode is kept even if the open item is gone: switching to the list would turn the
        // rest of a typed password or answer into list commands. The detail pane and send()
        // report the loss.
    }

    fn find(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// The items of the session view, or all of them in the active tab.
    fn visible(&self) -> Vec<&Item> {
        self.items_in(self.tab)
    }

    /// The items of `visible()` as if `tab` were the active one: those of agents with an origin
    /// and sudo requests (they come from no agent, so every tab shows them). The session view
    /// ignores the tab (it cannot be changed there).
    fn items_in(&self, tab: usize) -> Vec<&Item> {
        let session = self.session.as_deref();
        self.shown()
            .filter(|i| match session {
                Some(s) => i.session_id() == s,
                None => self.item_in_tab(tab, i),
            })
            .collect()
    }

    /// The items that have an origin or are sudo requests, in any session.
    fn shown(&self) -> impl Iterator<Item = &Item> {
        self.items
            .iter()
            .filter(|i| !i.origin_file().is_empty() || i.kind().starts_with("sudo."))
    }

    fn item_in_tab(&self, tab: usize, item: &Item) -> bool {
        let kind = self.origin_kind(item.origin_file());
        item.kind().starts_with("sudo.") || self.in_tab(tab, kind)
    }

    /// All sessions: the origin tabs filter items only.
    fn visible_sessions(&self) -> Vec<&Session> {
        self.sessions.iter().collect()
    }

    /// The Sessions tab: after All and the configured tabs.
    fn sessions_tab(&self) -> usize {
        self.tabs.len() + 1
    }

    /// Whether the sessions list is shown: the Sessions tab, not an item list of one session.
    fn sessions_shown(&self) -> bool {
        self.tab == self.sessions_tab() && self.session.is_none()
    }

    /// The tab `tab` (0 = all); None for all and the Sessions tab.
    fn tab_def(&self, tab: usize) -> Option<&Tab> {
        self.tabs.get(tab.checked_sub(1)?)
    }

    fn tab_name(&self, tab: usize) -> &str {
        if tab == self.sessions_tab() {
            return "Sessions";
        }
        self.tab_def(tab).map_or("All", |t| t.name.as_str())
    }

    /// Whether something of origin kind `kind` shows in `tab`.
    fn in_tab(&self, tab: usize, kind: &str) -> bool {
        let listed = |t: &Tab| t.kinds.iter().flatten().any(|k| k == kind);
        match self.tab_def(tab) {
            None => true,
            Some(t) if t.kinds.is_some() => listed(t),
            Some(_) => !self.tabs.iter().any(listed),
        }
    }

    /// `All N · Review N · … · Sessions N` with the active tab in bold, the row below the header.
    fn tabs_title(&self) -> Line<'static> {
        let mut spans = vec![Span::raw(" ")];
        for tab in 0..=self.sessions_tab() {
            if tab > 0 {
                spans.push(Span::styled(" · ", dim()));
            }
            let count = if tab == self.sessions_tab() {
                self.sessions.len()
            } else {
                // not items_in: the bar counts every tab, also inside a session
                self.shown().filter(|i| self.item_in_tab(tab, i)).count()
            };
            let text = format!("{} {count}", self.tab_name(tab));
            spans.push(if tab == self.tab {
                Span::styled(text, bold())
            } else {
                Span::raw(text)
            });
        }
        spans.push(Span::raw(" "));
        Line::from(spans)
    }

    /// Switches to the next tab (after Sessions: all); the selection follows its item or pid.
    fn next_tab(&mut self) {
        let item = self.selected_item().map(|i| i.id.clone());
        let pid = self.selected_session().map(|s| s.pid);
        self.tab = self.following_tab();
        self.select_id(item);
        self.select_pid(pid);
    }

    fn following_tab(&self) -> usize {
        (self.tab + 1) % (self.sessions_tab() + 1)
    }

    /// The origin that was read from `file`.
    fn origin(&self, file: &str) -> Option<&Origin> {
        self.origins.get(file)?.as_ref()
    }

    /// The kind of the origin read from `file`, "" if there is none.
    fn origin_kind(&self, file: &str) -> &str {
        self.origin(file).map_or("", |o| o.kind.as_str())
    }

    fn origin_label(&self, file: &str) -> Option<String> {
        self.origin(file).and_then(client::origin_label)
    }

    /// What the item is about: its origin, else where it comes from.
    fn item_context(&self, item: &Item) -> String {
        let label = self.origin_label(item.origin_file());
        label.unwrap_or_else(|| item.place())
    }

    /// What the session is about: its origin, else where it runs.
    fn session_context(&self, session: &Session) -> String {
        let label = self.origin_label(&session.origin_file);
        label.unwrap_or_else(|| session.place())
    }

    /// The link of the origin read from `file`, if it has an http(s) one (the file is data of a
    /// GitLab event: a `file:` or `javascript:` URL must not reach xdg-open).
    fn link(&self, file: &str) -> Option<&str> {
        let url = self.origin(file)?.url.as_str();
        (url.starts_with("http://") || url.starts_with("https://")).then_some(url)
    }

    /// The effect that opens the link of the origin `file`.
    fn open_link(&mut self, file: &str) -> Effect {
        let Some(url) = self.link(file).map(str::to_string) else {
            self.notify("no link for this item", true);
            return Effect::None;
        };
        Effect::Open(url)
    }

    fn selected_item(&self) -> Option<&Item> {
        self.visible().get(self.selected).copied()
    }

    /// Selects the item `id` if it is visible, else keeps the position (within the list).
    fn select_id(&mut self, id: Option<String>) {
        let visible = self.visible();
        self.selected = id
            .and_then(|id| visible.iter().position(|i| i.id == id))
            .unwrap_or(self.selected)
            .min(visible.len().saturating_sub(1));
    }

    /// Enters (or, with None, leaves) the view of one session; the selected item stays selected.
    fn set_session(&mut self, session: Option<String>) {
        let followed = self.selected_item().map(|i| i.id.clone());
        self.session = session;
        self.select_id(followed);
    }

    /// The live sessions as `sessions::load` returns them. The selection follows its pid.
    fn set_sessions(&mut self, sessions: Vec<Session>) {
        let followed = self.selected_session().map(|s| s.pid);
        self.sessions = sessions;
        self.select_pid(followed);
    }

    /// Selects the session of process `pid` if it is visible, else keeps the position (within
    /// the list).
    fn select_pid(&mut self, pid: Option<u32>) {
        let visible = self.visible_sessions();
        self.session_sel = pid
            .and_then(|pid| visible.iter().position(|s| s.pid == pid))
            .unwrap_or(self.session_sel)
            .min(visible.len().saturating_sub(1));
    }

    /// The session of process `pid`.
    fn session_of(&self, pid: u32) -> Option<&Session> {
        self.sessions.iter().find(|s| s.pid == pid)
    }

    fn selected_session(&self) -> Option<&Session> {
        self.visible_sessions().get(self.session_sel).copied()
    }

    /// The pending items of `session` (none while the router is unreachable: they are stale).
    fn session_items(&self, session: &Session) -> Vec<&Item> {
        if self.offline.is_some() || session.session_id.is_empty() {
            return Vec::new();
        }
        self.shown()
            .filter(|i| i.session_id() == session.session_id)
            .collect()
    }

    fn without_reply(&self) -> Vec<String> {
        self.visible()
            .into_iter()
            .filter(|i| i.reply().is_none())
            .map(|i| i.id.clone())
            .collect()
    }

    fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some(Status {
            text: text.into(),
            error,
            at: self.now,
        });
    }

    /// The status message, until it has been shown for `STATUS_TTL`.
    fn status(&self) -> Option<&Status> {
        self.status
            .as_ref()
            .filter(|s| self.now - s.at < STATUS_TTL)
    }

    fn on_key(&mut self, key: KeyEvent) -> Effect {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Effect::Quit;
        }
        // A line break in the task (before the filter below: Ctrl+J is a Ctrl character).
        if is_newline(key) {
            if let Mode::Task { buf } = &mut self.mode {
                buf.push('\n');
                return Effect::None;
            }
        }
        // Ctrl/Alt + a character is neither a command nor text: Ctrl+Y must not allow, nor
        // Alt+D dismiss. The mode stays as it is.
        if matches!(key.code, KeyCode::Char(_))
            && key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return Effect::None;
        }
        let code = key.code;
        if self.held(code) {
            return Effect::None;
        }
        let from_list = matches!(self.mode, Mode::List | Mode::ConfirmDismissAll);
        let effect = match std::mem::take(&mut self.mode) {
            Mode::List if self.sessions_shown() => self.sessions_key(code),
            Mode::List => self.list_key(code),
            Mode::Detail { id, scroll } => self.detail_key(id, scroll, code),
            Mode::SessionDetail { pid, scroll } => self.session_detail_key(pid, scroll, code),
            Mode::Password { id, buf } => self.password_key(id, buf, code),
            Mode::Answer { id, st } => self.answer_key(id, st, code),
            Mode::ConfirmDismissAll if code == KeyCode::Char('y') => Effect::DismissAll,
            Mode::ConfirmDismissAll => Effect::None,
            Mode::Task { buf } => self.task_key(buf, code),
        };
        // An item decided in the detail pane of a session: on to the next item of the session.
        // (From the list the focus stays there, a held key must not decide the next item.)
        if !from_list && self.session.is_some() && matches!(self.mode, Mode::List) {
            if let Effect::Send(id, _) | Effect::Dismiss(id) = &effect {
                self.advance(id, code);
            }
        }
        effect
    }

    /// A bracketed paste: only the text inputs (task, password, "Other…") take it, the task
    /// with its lines and the others as one line. Elsewhere it would be commands.
    fn on_paste(&mut self, text: &str) {
        if let Mode::Task { buf } = &mut self.mode {
            buf.push_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
            return;
        }
        // a password has no line breaks to keep (a manager's trailing newline must not become a space)
        if let Mode::Password { buf, .. } = &mut self.mode {
            buf.extend(text.chars().filter(|c| !matches!(c, '\n' | '\r')));
            return;
        }
        let text = text.replace(['\n', '\r'], " ");
        if let Mode::Answer {
            st: Answering {
                input: Some(buf), ..
            },
            ..
        } = &mut self.mode
        {
            buf.push_str(&text);
        }
    }

    /// Opens the item after `id` in the session view (the one before it for the last), and
    /// guards the key `code` that decided `id` for `KEY_GUARD`. The session ends with its last
    /// item.
    fn advance(&mut self, id: &str, code: KeyCode) {
        let visible = self.visible();
        let pos = visible.iter().position(|i| i.id == id).unwrap_or(0);
        let others: Vec<&Item> = visible.into_iter().filter(|i| i.id != id).collect();
        let last = others.len().saturating_sub(1);
        let Some(next) = others.get(pos.min(last)).map(|i| i.id.clone()) else {
            return;
        };
        self.select_id(Some(next.clone()));
        self.open(next, code);
        self.guard = Some((self.now, code));
    }

    /// Focuses the item `id`: a question goes straight to choosing the answer (and guards the
    /// key `code` that opened it for `KEY_GUARD`, a held Enter in the list must not also pick),
    /// the rest to the detail pane.
    fn open(&mut self, id: String, code: KeyCode) {
        self.mode = if self.find(&id).and_then(action) == Some(Action::Question) {
            self.raw = false; // the choice must be visible
            self.guard = Some((self.now, code));
            Mode::Answer {
                id,
                st: Answering::default(),
            }
        } else {
            Mode::Detail { id, scroll: 0 }
        };
    }

    fn list_key(&mut self, code: KeyCode) -> Effect {
        let last = self.visible().len().saturating_sub(1);
        match code {
            KeyCode::Char('q') => return Effect::Quit,
            KeyCode::Char('j') | KeyCode::Down => self.selected = (self.selected + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('g') => self.selected = 0,
            KeyCode::Char('G') => self.selected = last,
            KeyCode::Char('N') => self.mode = Mode::Task { buf: String::new() },
            KeyCode::Char('f') if self.session.is_none() => self.next_tab(),
            KeyCode::Esc | KeyCode::Char('h' | 's') if self.session.is_some() => {
                self.set_session(None)
            }
            KeyCode::Char('s') => {
                let session = self.selected_item().map(|i| i.session_id().to_string());
                match session.filter(|s| !s.is_empty()) {
                    Some(session) => self.set_session(Some(session)),
                    None => self.notify("no Claude session on this item", false),
                }
            }
            KeyCode::Enter | KeyCode::Tab | KeyCode::Char('l') => {
                if let Some(id) = self.selected_item().map(|i| i.id.clone()) {
                    self.open(id, code);
                }
            }
            KeyCode::Char('D') => {
                if self.without_reply().is_empty() {
                    self.notify("no read-only items to dismiss", false);
                } else {
                    self.mode = Mode::ConfirmDismissAll;
                }
            }
            // Dismiss, attach, open and raw view only: decisions and input modes are for the
            // detail pane (a held key must not decide the items that move up under the selection).
            KeyCode::Char('d' | 'a' | 'o' | '?') => {
                if let Some(id) = self.selected_item().map(|i| i.id.clone()) {
                    if let Some(effect) = self.item_key(id, code) {
                        return effect;
                    }
                }
            }
            _ => {}
        }
        Effect::None
    }

    fn sessions_key(&mut self, code: KeyCode) -> Effect {
        let last = self.visible_sessions().len().saturating_sub(1);
        match code {
            KeyCode::Char('q') => return Effect::Quit,
            KeyCode::Char('f') => self.next_tab(),
            KeyCode::Char('j') | KeyCode::Down => {
                self.session_sel = (self.session_sel + 1).min(last)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.session_sel = self.session_sel.saturating_sub(1)
            }
            KeyCode::Char('g') => self.session_sel = 0,
            KeyCode::Char('G') => self.session_sel = last,
            KeyCode::Enter | KeyCode::Tab | KeyCode::Char('l') => {
                if let Some(pid) = self.selected_session().map(|s| s.pid) {
                    self.mode = Mode::SessionDetail { pid, scroll: 0 };
                }
            }
            KeyCode::Char('s') => {
                let Some(session) = self.selected_session() else {
                    return Effect::None;
                };
                let id = session.session_id.clone();
                let attachable = !session.shpool.is_empty();
                if self.session_items(session).is_empty() {
                    let text = if attachable {
                        "no pending items — a to attach"
                    } else {
                        "no pending items"
                    };
                    self.notify(text, false);
                } else {
                    self.guard = Some((self.now, code)); // a held s would leave the items again
                    self.set_session(Some(id));
                    self.selected = 0;
                }
            }
            KeyCode::Char('a' | 'x' | 'o') => {
                let pid = self.selected_session().map(|s| s.pid);
                return self.session_key(pid, code);
            }
            KeyCode::Char('N') => self.mode = Mode::Task { buf: String::new() },
            _ => {}
        }
        Effect::None
    }

    /// Attach (`a`), kill (`x`) and open (`o`) for the session of process `pid`, from the list
    /// and from its detail pane; the mode is left alone.
    fn session_key(&mut self, pid: Option<u32>, code: KeyCode) -> Effect {
        let session = self.sessions.iter().find(|s| Some(s.pid) == pid);
        match code {
            KeyCode::Char('a') => {
                let name = session.map(|s| s.shpool.clone());
                match name.filter(|n| !n.is_empty()) {
                    Some(name) => return Effect::Attach(name),
                    None => self.notify("not in a shpool session", true),
                }
            }
            // Without a start time the pid cannot be told from a reused one.
            KeyCode::Char('x') => match session.map(|s| (s.pid, s.start)) {
                Some((pid, Some(start))) => return Effect::Kill { pid, start },
                Some((pid, None)) => self.notify(
                    format!("pid {pid} not killed: the registry has no start time"),
                    true,
                ),
                None => {}
            },
            KeyCode::Char('o') => {
                let file = session.map(|s| s.origin_file.clone());
                return self.open_link(&file.unwrap_or_default());
            }
            _ => {}
        }
        Effect::None
    }

    /// The session detail pane: scrolling, and the keys of `session_key`. A vanished session:
    /// only Esc/q leave, every other key is swallowed.
    fn session_detail_key(&mut self, pid: u32, scroll: u16, code: KeyCode) -> Effect {
        let Some(session) = self.session_of(pid) else {
            if !matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
                self.mode = Mode::SessionDetail { pid, scroll };
            }
            return Effect::None;
        };
        let max = self.max_scroll_of(&session_lines(self, session));
        let scroll = scroll_key(scroll, max, code).unwrap_or(scroll);
        self.mode = Mode::SessionDetail { pid, scroll };
        match code {
            KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Backspace | KeyCode::Tab => {
                self.mode = Mode::List
            }
            KeyCode::Char('a' | 'x' | 'o') => return self.session_key(Some(pid), code),
            _ => {}
        }
        Effect::None
    }

    /// The ids of the pending items of the session of process `pid`.
    fn session_item_ids(&self, pid: u32) -> Vec<String> {
        let Some(session) = self.session_of(pid) else {
            return Vec::new();
        };
        self.session_items(session)
            .into_iter()
            .map(|i| i.id.clone())
            .collect()
    }

    /// Whether `code` is swallowed because it is the key that just opened an item (see
    /// `KEY_GUARD`): a held key must not decide it. Any other key lifts the guard.
    fn held(&mut self, code: KeyCode) -> bool {
        match self.guard {
            Some((at, key)) if key == code && self.now - at < KEY_GUARD => {
                self.guard = Some((self.now, key)); // a held key keeps the guard up
                true
            }
            _ => {
                self.guard = None;
                false
            }
        }
    }

    /// Keys that act on the item `id` (from the detail pane and the answer choice, `d`, `a`, `o`
    /// and `?` also from the list); None = not one of them. Decisions need a plain key (no Enter)
    /// and an item that can be answered.
    fn item_key(&mut self, id: String, code: KeyCode) -> Option<Effect> {
        let act = self.find(&id).and_then(action);
        match code {
            KeyCode::Char('d') => {
                self.mode = Mode::List;
                Some(Effect::Dismiss(id))
            }
            KeyCode::Char('a') => {
                let name = self.find(&id).map_or("", Item::shpool).to_string();
                if name.is_empty() {
                    self.notify("no shpool session for this item", true);
                    return Some(Effect::None);
                }
                Some(Effect::Attach(name))
            }
            KeyCode::Char('o') => {
                let file = self.find(&id).map_or("", Item::origin_file).to_string();
                Some(self.open_link(&file))
            }
            KeyCode::Char('?') => {
                self.raw = !self.raw;
                if let Mode::Detail { scroll, .. } = &mut self.mode {
                    *scroll = 0;
                }
                Some(Effect::None)
            }
            KeyCode::Char('p') | KeyCode::Enter if act == Some(Action::Password) => {
                self.mode = Mode::Password {
                    id,
                    buf: String::new(),
                };
                Some(Effect::None)
            }
            KeyCode::Char('y') if act == Some(Action::Permission) => {
                self.mode = Mode::List;
                Some(Effect::Send(id, ALLOW.into()))
            }
            KeyCode::Char('n') if matches!(act, Some(Action::Permission | Action::Question)) => {
                self.mode = Mode::List;
                Some(Effect::Send(id, DENY.into()))
            }
            _ => None,
        }
    }

    fn detail_key(&mut self, id: String, scroll: u16, code: KeyCode) -> Effect {
        // A vanished item: only Esc/q leave (the mode was reset to the list by `on_key`), every
        // other key is swallowed and must not reach a list command.
        let Some(item) = self.find(&id) else {
            if !matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
                self.mode = Mode::Detail { id, scroll };
            }
            return Effect::None;
        };
        let max = self.max_scroll(item);
        let scrolled = scroll_key(scroll, max, code);
        self.mode = Mode::Detail {
            id: id.clone(),
            scroll: scrolled.unwrap_or(scroll),
        };
        if scrolled.is_some() {
            return Effect::None;
        }
        match code {
            KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Backspace | KeyCode::Tab => {
                self.mode = Mode::List
            }
            code => {
                if let Some(effect) = self.item_key(id, code) {
                    return effect;
                }
            }
        }
        Effect::None
    }

    /// How far the summary (or raw event) of `item` can be scrolled.
    fn max_scroll(&self, item: &Item) -> u16 {
        self.max_scroll_of(&detail_view(self, item))
    }

    /// How far `lines` can be scrolled in the detail pane.
    fn max_scroll_of(&self, lines: &[Line<'_>]) -> u16 {
        let inner = Block::bordered().inner(panes(self.area).detail);
        let rows = wrapped_height(lines, inner.width);
        rows.saturating_sub(usize::from(inner.height))
            .min(usize::from(u16::MAX)) as u16
    }

    fn password_key(&mut self, id: String, mut buf: String, code: KeyCode) -> Effect {
        match code {
            KeyCode::Esc => self.mode = Mode::Detail { id, scroll: 0 },
            KeyCode::Enter if !buf.is_empty() => {
                self.mode = Mode::List;
                return Effect::Send(id, buf);
            }
            KeyCode::Backspace => {
                buf.pop();
                self.mode = Mode::Password { id, buf };
            }
            KeyCode::Char(c) => {
                buf.push(c);
                self.mode = Mode::Password { id, buf };
            }
            _ => self.mode = Mode::Password { id, buf },
        }
        Effect::None
    }

    /// Typing the task of a new agent; Enter starts it (an empty task: jw asks for the repo).
    fn task_key(&mut self, mut buf: String, code: KeyCode) -> Effect {
        match code {
            KeyCode::Esc => self.mode = Mode::List,
            KeyCode::Enter => {
                self.mode = Mode::List;
                return Effect::NewAgent(buf.trim().to_string());
            }
            KeyCode::Backspace => {
                buf.pop();
                self.mode = Mode::Task { buf };
            }
            KeyCode::Char(c) => {
                buf.push(c);
                self.mode = Mode::Task { buf };
            }
            _ => self.mode = Mode::Task { buf },
        }
        Effect::None
    }

    /// Choosing the answer to one question at a time (Enter/`l` or its number); the last one
    /// sends all answers. An item that vanished keeps the mode (like the password prompt):
    /// there is nothing to select, so every key but Esc does nothing.
    fn answer_key(&mut self, id: String, mut st: Answering, code: KeyCode) -> Effect {
        let questions = self.find(&id).map(Item::questions).unwrap_or_default();
        let Some(q) = questions.get(st.idx) else {
            self.mode = if code == KeyCode::Esc {
                Mode::List
            } else {
                Mode::Answer { id, st }
            };
            return Effect::None;
        };
        let other = q.labels.len();
        st.cursor = st.cursor.min(other);
        if let Some(mut buf) = st.input.take() {
            // typing the free text of "Other…"
            match code {
                KeyCode::Esc => {}
                KeyCode::Enter if !buf.trim().is_empty() => {
                    let text = buf.trim();
                    let answer = if q.multi {
                        join_answer(q, &st.picked, Some(text))
                    } else {
                        text.to_string()
                    };
                    let total = questions.len();
                    return self.answered(id, st, total, q.question.clone(), answer, code);
                }
                KeyCode::Backspace => {
                    buf.pop();
                    st.input = Some(buf);
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    st.input = Some(buf);
                }
                _ => st.input = Some(buf),
            }
        } else {
            match code {
                KeyCode::Esc | KeyCode::Char('q' | 'h') => {
                    self.mode = Mode::List;
                    return Effect::None;
                }
                // deny, dismiss and the raw view, as in the detail pane
                KeyCode::Char('n' | 'd' | '?') => match self.item_key(id.clone(), code) {
                    Some(effect) if effect != Effect::None => return effect,
                    _ => {}
                },
                // attach and open keep the choice open (item_key leaves the mode alone)
                KeyCode::Char('a' | 'o') => {
                    self.mode = Mode::Answer { id: id.clone(), st };
                    return self.item_key(id, code).unwrap_or(Effect::None);
                }
                // the raw view hides the choice: nothing is picked blind
                _ if self.raw => {}
                KeyCode::Char('k') | KeyCode::Up => st.cursor = st.cursor.saturating_sub(1),
                KeyCode::Char('j') | KeyCode::Down => st.cursor = (st.cursor + 1).min(other),
                KeyCode::Char('g') => st.cursor = 0,
                KeyCode::Char('G') => st.cursor = other,
                KeyCode::Char(c @ '1'..='9') => {
                    let n = c as usize - '1' as usize;
                    if n < other {
                        st.cursor = n;
                        if q.multi {
                            toggle(&mut st.picked, n);
                        } else {
                            let answer = q.labels[n].clone();
                            let total = questions.len();
                            return self.answered(id, st, total, q.question.clone(), answer, code);
                        }
                    }
                }
                KeyCode::Char(' ') if q.multi && st.cursor < other => {
                    toggle(&mut st.picked, st.cursor)
                }
                KeyCode::Char(' ' | 'l') | KeyCode::Enter if st.cursor == other => {
                    st.input = Some(String::new())
                }
                KeyCode::Enter | KeyCode::Char('l') if q.multi => {
                    if st.picked.is_empty() {
                        self.notify("select at least one option (space) or Other…", true);
                    } else {
                        let answer = join_answer(q, &st.picked, None);
                        let total = questions.len();
                        return self.answered(id, st, total, q.question.clone(), answer, code);
                    }
                }
                KeyCode::Enter | KeyCode::Char('l') => {
                    let answer = q.labels[st.cursor].clone();
                    let total = questions.len();
                    return self.answered(id, st, total, q.question.clone(), answer, code);
                }
                _ => {}
            }
        }
        self.mode = Mode::Answer { id, st };
        Effect::None
    }

    /// Records the answer to a question (given with the key `code`): on to the next one, or
    /// send all of them.
    fn answered(
        &mut self,
        id: String,
        mut st: Answering,
        total: usize,
        question: String,
        answer: String,
        code: KeyCode,
    ) -> Effect {
        st.done.push((question, answer));
        if st.idx + 1 < total {
            st.idx += 1;
            st.cursor = 0;
            st.picked.clear();
            self.guard = Some((self.now, code)); // a held key must not answer the next one too
            self.mode = Mode::Answer { id, st };
            return Effect::None;
        }
        self.mode = Mode::List;
        let answers: Map<String, Value> = st
            .done
            .into_iter()
            .map(|(question, answer)| (question, Value::String(answer)))
            .collect();
        Effect::Send(
            id,
            json!({"behavior": "allow", "answers": answers}).to_string(),
        )
    }

    /// The hint for `f`, which names the tab it switches to.
    fn tab_hint(&self) -> (&str, &str) {
        ("f", self.tab_name(self.following_tab()))
    }

    /// The hints of the keys of `session_key` that `session` allows.
    fn session_hints(&self, session: &Session) -> Vec<(&'static str, &'static str)> {
        let mut h = Vec::new();
        if !session.shpool.is_empty() {
            h.push(("a", "attach"));
        }
        if self.link(&session.origin_file).is_some() {
            h.push(("o", "open"));
        }
        if session.start.is_some() {
            h.push(("x", "kill"));
        }
        h
    }

    /// Key hints for the footer (key, label).
    fn hints(&self) -> Vec<(&str, &str)> {
        if self.sessions_shown() && matches!(self.mode, Mode::List) {
            let mut h = Vec::new();
            if let Some(session) = self.selected_session() {
                h.push(("enter", "open"));
                if !self.session_items(session).is_empty() {
                    h.push(("s", "items"));
                }
                h.extend(self.session_hints(session));
            }
            h.push(self.tab_hint());
            h.push(("N", "new"));
            h.push(("q", "quit"));
            return h;
        }
        match &self.mode {
            Mode::List | Mode::ConfirmDismissAll => {
                let mut h = Vec::new();
                if let Some(item) = self.selected_item() {
                    h.push(("enter", "open"));
                    h.push(("d", "dismiss"));
                    if !item.shpool().is_empty() {
                        h.push(("a", "attach"));
                    }
                    if self.link(item.origin_file()).is_some() {
                        h.push(("o", "open"));
                    }
                    if item.reply().is_none() {
                        h.push(("D", "dismiss read-only"));
                    }
                    h.push(("?", "raw"));
                    if self.session.is_none() && !item.session_id().is_empty() {
                        h.push(("s", "session"));
                    }
                }
                if self.session.is_some() {
                    h.push(("esc", self.tab_name(self.tab)));
                } else {
                    h.push(self.tab_hint());
                }
                h.push(("N", "new"));
                h.push(("q", "quit"));
                h
            }
            Mode::Detail { id, .. } => {
                // A vanished item swallows every key but Esc/q
                let Some(item) = self.find(id) else {
                    return vec![("esc", "back")];
                };
                let mut h = vec![("j/k", "scroll")];
                h.extend(action_hints(item));
                h.push(("d", "dismiss"));
                if !item.shpool().is_empty() {
                    h.push(("a", "attach"));
                }
                if self.link(item.origin_file()).is_some() {
                    h.push(("o", "open"));
                }
                h.push(("?", "raw"));
                h.push(("esc", "back"));
                h
            }
            Mode::SessionDetail { pid, .. } => {
                let Some(session) = self.session_of(*pid) else {
                    return vec![("esc", "back")];
                };
                let mut h = vec![("j/k", "scroll")];
                h.extend(self.session_hints(session));
                h.push(("esc", "back"));
                h
            }
            Mode::Password { .. } => vec![("enter", "send"), ("esc", "cancel")],
            Mode::Task { .. } => vec![("enter", "start"), ("ctrl-j", "newline"), ("esc", "cancel")],
            Mode::Answer { id, st } => {
                let multi = self
                    .find(id)
                    .map(Item::questions)
                    .and_then(|qs| qs.get(st.idx).map(|q| q.multi));
                let mut h = match (multi, &st.input) {
                    (None, _) => return vec![("esc", "back")],
                    (Some(_), Some(_)) => return vec![("enter", "accept"), ("esc", "back")],
                    (Some(true), None) => vec![
                        ("1-9/space", "toggle"),
                        ("j/k", "move"),
                        ("l/enter", "confirm"),
                        ("h", "back"),
                        ("n", "deny"),
                        ("d", "dismiss"),
                    ],
                    (Some(false), None) => vec![
                        ("1-9", "pick"),
                        ("j/k", "move"),
                        ("l/enter", "select"),
                        ("h", "back"),
                        ("n", "deny"),
                        ("d", "dismiss"),
                    ],
                };
                if self.find(id).is_some_and(|item| !item.shpool().is_empty()) {
                    h.push(("a", "attach"));
                }
                if self
                    .find(id)
                    .is_some_and(|item| self.link(item.origin_file()).is_some())
                {
                    h.push(("o", "open"));
                }
                h
            }
        }
    }
}

/// Ctrl+J or Alt+Enter: a line break in a text, where plain Enter confirms.
fn is_newline(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('j') => key.modifiers.contains(KeyModifiers::CONTROL),
        KeyCode::Enter => key.modifiers.contains(KeyModifiers::ALT),
        _ => false,
    }
}

fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max == 0 {
        String::new()
    } else {
        s.chars()
            .take(max - 1)
            .chain(std::iter::once('…'))
            .collect()
    }
}

/// The scroll offset after the scroll key `code` (None: not one), kept within 0..=`max`.
fn scroll_key(scroll: u16, max: u16, code: KeyCode) -> Option<u16> {
    let step = |delta: i32| (i32::from(scroll) + delta).clamp(0, i32::from(max)) as u16;
    match code {
        KeyCode::Char('j') | KeyCode::Down => Some(step(1)),
        KeyCode::Char('k') | KeyCode::Up => Some(step(-1)),
        KeyCode::PageDown => Some(step(10)),
        KeyCode::PageUp => Some(step(-10)),
        KeyCode::Char('g') => Some(0),
        KeyCode::Char('G') => Some(max),
        _ => None,
    }
}

/// How many rows `lines` take when word-wrapped to `width`, as ratatui draws them (display
/// width, not chars); used to limit scrolling.
fn wrapped_height(lines: &[Line<'_>], width: u16) -> usize {
    Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(width)
}

/// The scroll offset to draw `lines` at in `area`: `scroll` limited to the content, or
/// whatever shows the `cursor` line.
fn scroll_for(lines: &[Line<'_>], cursor: Option<usize>, scroll: u16, area: Rect) -> u16 {
    let height = usize::from(area.height);
    let total = wrapped_height(lines, area.width);
    let wanted = match cursor {
        Some(c) => {
            wrapped_height(&lines[..lines.len().min(c + 1)], area.width).saturating_sub(height)
        }
        None => usize::from(scroll),
    };
    wanted
        .min(total.saturating_sub(height))
        .min(usize::from(u16::MAX)) as u16
}

/// Text and style of the badge for a kind of event.
fn badge(kind: &str) -> (String, Style) {
    let strong = |color: Color| Style::new().fg(color).add_modifier(Modifier::BOLD);
    match kind {
        "sudo.password_request" => ("SUDO".into(), strong(Color::Red)),
        "claude.permission_request" => ("PERM".into(), strong(Color::Yellow)),
        "claude.question" => ("ASK".into(), strong(Color::Cyan)),
        "claude.waiting" => ("WAIT".into(), strong(Color::Green)),
        other => (
            truncate(&clean(other.rsplit('.').next().unwrap_or("")), BADGE_WIDTH),
            dim(),
        ),
    }
}

/// `value` as lines (commands and file contents keep their line breaks), each cleaned.
fn lines_of(value: &str, style: Style) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = value
        .lines()
        .take(MAX_LINES)
        .map(|l| Line::styled(clean(l), style))
        .collect();
    if value.lines().count() > MAX_LINES {
        lines.push(Line::styled("…", dim()));
    }
    lines
}

fn hint_spans(hints: &[(&str, &str)], sep: &str) -> Vec<Span<'static>> {
    let key = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    for (i, (k, label)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(sep.to_string(), dim()));
        }
        spans.push(Span::styled(k.to_string(), key));
        spans.push(Span::styled(format!(" {label}"), dim()));
    }
    spans
}

fn question_lines(q: &Question) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if !q.header.is_empty() {
        lines.push(Line::styled(format!("[{}]", clean(&q.header)), dim()));
    }
    let mut title = vec![Span::styled(clean(&q.question), bold())];
    if q.multi {
        title.push(Span::styled(" [multi]", Style::new().fg(Color::Cyan)));
    }
    lines.push(Line::from(title));
    lines
}

/// The detail of `item`: a summary per kind, or the raw event.
fn detail_view(app: &App, item: &Item) -> Vec<Line<'static>> {
    if app.raw {
        return serde_json::to_string_pretty(&item.event)
            .unwrap_or_default()
            .lines()
            .map(|l| Line::raw(clean(l)))
            .collect();
    }
    let (badge_text, badge_style) = badge(item.kind());
    let mut lines = vec![Line::from(vec![
        Span::styled(badge_text, badge_style),
        Span::raw("  "),
        Span::styled(clean(&item.project_name()), bold()),
        Span::styled(format!("  ·  {}", item.age(app.now)), dim()),
    ])];
    let jj_line = item.jj_line();
    let location = match &jj_line {
        Some(line) => ("jj", line.as_str()),
        None => ("cwd", item.cwd()),
    };
    for (label, value) in [
        location,
        ("agent", item.agent_type()),
        ("shpool", item.shpool()),
    ] {
        if !value.is_empty() {
            lines.push(Line::styled(format!("{label}: {}", clean(value)), dim()));
        }
    }
    lines.push(Line::default());
    match item.kind() {
        "sudo.password_request" => sudo_lines(item, &mut lines),
        "claude.permission_request" => permission_lines(item, &mut lines),
        "claude.question" if !item.questions().is_empty() => question_summary(item, &mut lines),
        _ => other_lines(item, &mut lines),
    }
    lines.push(Line::default());
    lines.push(match action(item) {
        Some(_) => Line::from(hint_spans(&action_hints(item), " · ")),
        None => {
            let why = if matches!(item.kind(), "claude.permission_request" | "claude.question")
                && item.reply().is_none()
            {
                "read-only — that session's hook isn't waiting for an answer (CLAUDE_INBOX_WAIT=0, an older session, or it timed out)"
            } else {
                "read-only"
            };
            Line::styled(
                why,
                Style::new().fg(Color::Yellow).add_modifier(Modifier::DIM),
            )
        }
    });
    let origin = origin_lines(app, item.origin_file());
    if !origin.is_empty() {
        lines.push(Line::default());
        lines.extend(origin);
    }
    lines
}

fn sudo_lines(item: &Item, lines: &mut Vec<Line<'static>>) {
    let command = if item.command().is_empty() {
        item.title()
    } else {
        item.command()
    };
    let highlight = Style::new()
        .fg(Color::Red)
        .add_modifier(Modifier::BOLD | Modifier::REVERSED);
    lines.extend(lines_of(command, highlight));
    if !item.prompt().is_empty() {
        lines.push(Line::default());
        lines.extend(lines_of(item.prompt(), dim()));
    }
}

fn permission_lines(item: &Item, lines: &mut Vec<Line<'static>>) {
    let tool = if item.tool_name().is_empty() {
        item.title()
    } else {
        item.tool_name()
    };
    lines.push(Line::styled(clean(tool), bold()));
    match item.tool_name() {
        "Bash" => {
            lines.extend(lines_of(
                item.input("command"),
                Style::new().fg(Color::Yellow),
            ));
            if !item.input("description").is_empty() {
                lines.push(Line::styled(clean(item.input("description")), dim()));
            }
        }
        "Edit" | "Write" | "Read" | "MultiEdit" => {
            lines.push(Line::raw(clean(item.input("file_path"))));
            if item.tool_name() == "Write" {
                let size = item.input("content").lines().count();
                lines.push(Line::styled(format!("{size} lines"), dim()));
            }
        }
        "WebFetch" => lines.push(Line::raw(clean(item.input("url")))),
        _ => {
            let input = item.event.get("tool_input").and_then(Value::as_object);
            for (key, value) in input.into_iter().flatten().take(8) {
                let text = match value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{}: ", clean(key)), dim()),
                    Span::raw(truncate(&clean(&text), 120)),
                ]));
            }
        }
    }
}

fn question_summary(item: &Item, lines: &mut Vec<Line<'static>>) {
    for (i, q) in item.questions().iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.extend(question_lines(q));
        for (label, description) in q.labels.iter().zip(&q.descriptions) {
            lines.push(Line::from(vec![
                Span::raw("  · "),
                Span::styled(clean(label), bold()),
            ]));
            if !description.is_empty() {
                lines.push(Line::styled(format!("    {}", clean(description)), dim()));
            }
        }
    }
}

/// What the agent behind the origin `file` works on: its name, link, and the description of the
/// merge request or issue (or the prompt of the task); empty without an origin.
fn origin_lines(app: &App, file: &str) -> Vec<Line<'static>> {
    let Some(origin) = app.origin(file) else {
        return Vec::new();
    };
    let text = if origin.description.is_empty() {
        &origin.prompt
    } else {
        &origin.description
    };
    let text = text
        .lines()
        .map(|l| truncate(l, ORIGIN_LINE_CHARS))
        .collect::<Vec<_>>()
        .join("\n");
    let mut lines = Vec::new();
    if let Some(label) = app
        .origin_label(file)
        .filter(|label| !starts_with_loosely(&text, label))
    {
        lines.push(Line::styled(label, bold()));
    }
    if !origin.url.is_empty() {
        lines.push(Line::styled(clean(&origin.url), dim()));
    }
    lines.extend(lines_of(&text, Style::new()));
    lines
}

/// Whether `text` starts with `head`, comparing only letters and digits, ignoring case: a title
/// cut from the text (and stripped of markdown) is not worth repeating above it.
fn starts_with_loosely(text: &str, head: &str) -> bool {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    norm(text).starts_with(&norm(head))
}

/// Items of an unknown kind (and questions without any): the title and what else there is.
fn other_lines(item: &Item, lines: &mut Vec<Line<'static>>) {
    let title = item.title();
    let values: Vec<&str> = [item.command(), item.prompt(), item.body()]
        .into_iter()
        .filter(|v| !v.is_empty() && *v != title)
        .collect();
    if !values
        .first()
        .is_some_and(|v| starts_with_loosely(v, title))
    {
        lines.push(Line::styled(clean(title), bold()));
    }
    for value in values {
        lines.extend(lines_of(value, Style::new()));
    }
}

/// The question being answered with the cursor on its options; and the line the cursor is on.
fn answer_view(item: &Item, st: &Answering) -> (Vec<Line<'static>>, Option<usize>) {
    let questions = item.questions();
    let Some(q) = questions.get(st.idx) else {
        return (vec![Line::styled("question is gone", dim())], None);
    };
    let mut lines = vec![Line::styled(
        format!("Question {}/{}", st.idx + 1, questions.len()),
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )];
    for (question, answer) in &st.done {
        lines.push(Line::styled(
            format!("✓ {} → {}", clean(question), clean(answer)),
            dim(),
        ));
    }
    lines.push(Line::default());
    lines.extend(question_lines(q));
    lines.push(Line::default());
    let choice = |on: bool, text: String| {
        if on {
            Line::styled(format!("> {text}"), bold().add_modifier(Modifier::REVERSED))
        } else {
            Line::styled(format!("  {text}"), bold())
        }
    };
    let mut cursor = 0;
    for (n, (label, description)) in q.labels.iter().zip(&q.descriptions).enumerate() {
        let mark = match (q.multi, st.picked.contains(&n)) {
            (false, _) => "",
            (true, true) => "[x] ",
            (true, false) => "[ ] ",
        };
        // the first nine options are picked by their number
        let number = if n < 9 {
            format!("{} ", n + 1)
        } else {
            "  ".into()
        };
        lines.push(choice(
            st.cursor == n,
            format!("{number}{mark}{}", clean(label)),
        ));
        if !description.is_empty() {
            lines.push(Line::styled(format!("    {}", clean(description)), dim()));
        }
        if st.cursor == n {
            cursor = lines.len() - 1;
        }
    }
    lines.push(choice(st.cursor == q.labels.len(), "  Other…".into()));
    if st.cursor == q.labels.len() {
        cursor = lines.len() - 1;
    }
    (lines, Some(cursor))
}

/// The end of `buf` that fits in `room` columns, so what is typed stays visible.
fn tail(buf: &str, room: usize) -> String {
    let mut width = 0;
    let mut start = buf.len();
    for (i, c) in buf.char_indices().rev() {
        width += c.width().unwrap_or(0);
        if width > room {
            break;
        }
        start = i;
    }
    buf[start..].to_string()
}

/// The inline input row at the bottom of the detail pane: the masked password or the free
/// text of "Other…".
fn input_line(mode: &Mode, width: u16) -> Option<Line<'static>> {
    let cursor = Span::styled(" ", Style::new().add_modifier(Modifier::REVERSED));
    match mode {
        Mode::Password { buf, .. } => Some(Line::from(vec![
            Span::styled("password: ", bold()),
            Span::raw("*".repeat(buf.chars().count())),
            cursor,
        ])),
        Mode::Answer { st, .. } => st.input.as_ref().map(|buf| {
            let room = usize::from(width).saturating_sub("Other: ".len() + 1);
            Line::from(vec![
                Span::styled("Other: ", bold()),
                Span::raw(tail(&clean(buf), room)),
                cursor,
            ])
        }),
        _ => None,
    }
}

struct Panes {
    header: Rect,
    tabs: Rect,
    list: Rect,
    detail: Rect,
    footer: Rect,
}

fn panes(area: Rect) -> Panes {
    let [header, tabs, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let [list, detail] = if area.width >= SIDE_BY_SIDE {
        Layout::horizontal([
            Constraint::Length((area.width * 2 / 5).max(LIST_MIN_WIDTH)),
            Constraint::Min(1),
        ])
        .areas(body)
    } else {
        Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(body)
    };
    Panes {
        header,
        tabs,
        list,
        detail,
        footer,
    }
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::new().fg(Color::Cyan)
    } else {
        dim()
    }
}

fn render(f: &mut Frame, app: &App) {
    let p = panes(f.area());
    render_header(f, app, p.header);
    f.render_widget(Paragraph::new(app.tabs_title()), p.tabs);
    if app.sessions_shown() {
        render_sessions(f, app, p.list);
        render_session_detail(f, app, p.detail);
    } else {
        render_list(f, app, p.list);
        render_detail(f, app, p.detail);
    }
    render_footer(f, app, p.footer);
    if let Mode::Task { buf } = &app.mode {
        render_task(f, &p, buf);
    }
}

/// The popup for the task of a new agent, over the body: the lines of `buf`, wrapped, the end
/// in view.
fn render_task(f: &mut Frame, p: &Panes, buf: &str) {
    let body = p.list.union(p.detail);
    let width = body.width.min(100).saturating_sub(2);
    // a no-break space: a plain one is dropped by the wrapping where a line exactly fills a row
    let cursor = Span::styled("\u{a0}", Style::new().add_modifier(Modifier::REVERSED));
    let mut rows: Vec<&str> = buf.split('\n').collect();
    let last = rows.pop().unwrap_or_default();
    let mut lines: Vec<Line> = rows.into_iter().map(|l| Line::raw(clean(l))).collect();
    let mut last = vec![Span::raw(clean(last)), cursor];
    if buf.is_empty() {
        last.push(Span::styled(
            "What should the agent do? (empty: pick a repo and attach)",
            dim(),
        ));
    }
    lines.push(Line::from(last));
    let total = wrapped_height(&lines, width.saturating_sub(2)) + 2;
    let height = total.max(5).min(usize::from(body.height)) as u16;
    let area = Rect::new(
        body.x + (body.width - width) / 2,
        body.y + (body.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered().title(" New agent ");
    let inner = block.inner(area);
    let scroll = scroll_for(&lines, Some(lines.len() - 1), 0, inner);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        inner,
    );
}

/// The only row of a list with nothing in it.
fn empty_row(app: &App) -> ListItem<'static> {
    let text = match (app.tab_def(app.tab), app.sessions_shown()) {
        (_, true) => "No Claude sessions".to_string(),
        (Some(tab), false) => format!("No pending items in {}", clean(&tab.name)),
        (None, false) => "No pending requests".to_string(),
    };
    ListItem::new(Line::styled(text, dim()))
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let pending = app.shown().count();
    let answerable = app.shown().filter(|i| action(i).is_some()).count();
    let waiting = match app.sessions.iter().filter(|s| s.status == WAITING).count() {
        0 => String::new(),
        n => format!(" ({n} waiting)"),
    };
    let left = Line::from(vec![
        Span::styled(" agent-inbox", bold()),
        Span::styled(
            format!(
                "  {pending} pending · {answerable} answerable · {} sessions{waiting}",
                app.sessions.len()
            ),
            dim(),
        ),
    ]);
    // The router status comes first (at most half of the line), the counts give way to it.
    let (text, style) = match &app.offline {
        Some(e) => (
            clean(&format!("router offline: {e}")),
            Style::new().fg(Color::Red),
        ),
        None => ("connected".to_string(), dim()),
    };
    let len = text.chars().count() + 2;
    let wanted = u16::try_from(len).unwrap_or(u16::MAX);
    let width = wanted.min(area.width / 2);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(width)]).areas(area);
    let room = usize::from(width).saturating_sub(2);
    let right = Span::styled(truncate(&text, room), style);
    f.render_widget(Paragraph::new(left), left_area);
    f.render_widget(
        Paragraph::new(Line::from(right)).alignment(Alignment::Right),
        right_area,
    );
}

fn render_list(f: &mut Frame, app: &App, area: Rect) {
    let focused = matches!(app.mode, Mode::List | Mode::ConfirmDismissAll);
    let visible = app.visible();
    let mut block = Block::bordered().border_style(border_style(focused));
    if let Some(session) = &app.session {
        let place = visible.first().map(|i| i.place()).unwrap_or_default();
        let short: String = session.chars().take(8).collect();
        block = block.title(Line::from(clean(&format!(" Session {place} · {short} "))));
    }
    let width = usize::from(block.inner(area).width);
    let context_width = visible
        .iter()
        .map(|i| clean(&app.item_context(i)).chars().count())
        .max()
        .unwrap_or(0)
        .min(CONTEXT_MAX_WIDTH)
        .min(width / 3);
    let mut rows: Vec<ListItem> = visible
        .iter()
        .map(|i| ListItem::new(row(app, i, width, context_width)))
        .collect();
    if rows.is_empty() {
        rows.push(empty_row(app));
    }
    let list = List::new(rows)
        .block(block)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state =
        ListState::default().with_selected((!visible.is_empty()).then_some(app.selected));
    f.render_stateful_widget(list, area, &mut state);
}

/// marker, badge, context, title, age right-aligned; exactly `width` chars if it fits.
fn row(app: &App, item: &Item, width: usize, context_width: usize) -> Line<'static> {
    let (badge_text, badge_style) = badge(item.kind());
    let marker = if action(item).is_some() {
        Span::styled("●", bold())
    } else {
        Span::styled("○", dim())
    };
    let context = truncate(&clean(&app.item_context(item)), context_width);
    let age = item.age(app.now);
    let head = 2 + BADGE_WIDTH + 1 + context_width + 1;
    let age_width = age.chars().count();
    let title = truncate(
        &clean(item.title()),
        width.saturating_sub(head + age_width + 1),
    );
    let pad = width.saturating_sub(head + title.chars().count() + age_width);
    Line::from(vec![
        marker,
        Span::raw(" "),
        Span::styled(format!("{:<w$}", badge_text, w = BADGE_WIDTH), badge_style),
        Span::raw(" "),
        Span::raw(format!("{:<w$}", context, w = context_width)),
        Span::raw(" "),
        Span::raw(title),
        Span::raw(" ".repeat(pad)),
        Span::styled(age, dim()),
    ])
}

/// Text and style of the badge for the status of a session.
fn status_badge(status: &str) -> (String, Style) {
    match status {
        WAITING => (
            "WAIT".into(),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        "idle" => ("IDLE".into(), Style::new().fg(Color::Green)),
        "busy" => ("BUSY".into(), dim()),
        other => (truncate(&clean(other), BADGE_WIDTH), dim()),
    }
}

fn render_sessions(f: &mut Frame, app: &App, area: Rect) {
    let focused = !matches!(app.mode, Mode::SessionDetail { .. });
    let block = Block::bordered().border_style(border_style(focused));
    let width = usize::from(block.inner(area).width);
    let sessions = app.visible_sessions();
    let context_width = sessions
        .iter()
        .map(|s| clean(&app.session_context(s)).chars().count())
        .max()
        .unwrap_or(0)
        .min(CONTEXT_MAX_WIDTH)
        .min(width / 3);
    let mut rows: Vec<ListItem> = sessions
        .iter()
        .map(|s| ListItem::new(session_row(app, s, width, context_width)))
        .collect();
    if rows.is_empty() {
        rows.push(empty_row(app));
    }
    let list = List::new(rows)
        .block(block)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state =
        ListState::default().with_selected((!sessions.is_empty()).then_some(app.session_sel));
    f.render_stateful_widget(list, area, &mut state);
}

/// pending items, badge, context, what it waits for, status age right-aligned; exactly `width`
/// chars if it fits.
fn session_row(app: &App, session: &Session, width: usize, context_width: usize) -> Line<'static> {
    let (badge_text, badge_style) = status_badge(&session.status);
    let pending = app.session_items(session).len();
    let count = if pending == 0 {
        "  ".to_string()
    } else {
        format!("{:>2}", pending.min(99))
    };
    let context = truncate(&clean(&app.session_context(session)), context_width);
    let age = client::age(session.status_at, app.now);
    let head = 2 + 1 + BADGE_WIDTH + 1 + context_width + 1;
    let age_width = age.chars().count();
    let waiting_for = truncate(
        &clean(&session.waiting_for),
        width.saturating_sub(head + age_width + 1),
    );
    let pad = width.saturating_sub(head + waiting_for.chars().count() + age_width);
    Line::from(vec![
        Span::styled(count, bold()),
        Span::raw(" "),
        Span::styled(format!("{:<w$}", badge_text, w = BADGE_WIDTH), badge_style),
        Span::raw(" "),
        Span::raw(format!("{:<w$}", context, w = context_width)),
        Span::raw(" "),
        Span::raw(waiting_for),
        Span::raw(" ".repeat(pad)),
        Span::styled(age, dim()),
    ])
}

/// The detail of `session`: its fields, its pending items and its origin.
fn session_lines(app: &App, session: &Session) -> Vec<Line<'static>> {
    let (badge_text, badge_style) = status_badge(&session.status);
    let mut head = vec![Span::styled(badge_text, badge_style)];
    if !session.waiting_for.is_empty() {
        head.push(Span::raw(format!("  {}", clean(&session.waiting_for))));
    }
    let since = client::age(session.status_at, app.now);
    head.push(Span::styled(format!("  ·  {since}"), dim()));
    let mut lines = vec![Line::from(head)];
    let shpool = if session.shpool.is_empty() {
        "none: not attachable"
    } else {
        session.shpool.as_str()
    };
    let pid = session.pid.to_string();
    let started = format!("{} ago", client::age(session.started_at, app.now));
    for (label, value) in [
        ("name", session.name.as_str()),
        ("cwd", session.cwd.as_str()),
        ("shpool", shpool),
        ("pid", pid.as_str()),
        ("started", started.as_str()),
        ("kind", session.kind.as_str()),
        ("session", session.session_id.as_str()),
    ] {
        if !value.is_empty() {
            lines.push(Line::styled(format!("{label}: {}", clean(value)), dim()));
        }
    }
    lines.push(Line::default());
    let items = app.session_items(session);
    if items.is_empty() {
        lines.push(Line::styled("no pending items", dim()));
    }
    for item in items {
        let (badge_text, badge_style) = badge(item.kind());
        lines.push(Line::from(vec![
            Span::styled(format!("{:<w$}", badge_text, w = BADGE_WIDTH), badge_style),
            Span::raw(" "),
            Span::raw(clean(item.title())),
        ]));
        if item.kind() == "claude.waiting" {
            let message = item.body().lines().take(WAIT_BODY_LINES);
            lines.extend(message.map(|l| Line::styled(clean(l), dim())));
            if item.body().lines().count() > WAIT_BODY_LINES {
                lines.push(Line::styled("…", dim()));
            }
        }
    }
    let origin = origin_lines(app, &session.origin_file);
    if !origin.is_empty() {
        lines.push(Line::default());
        lines.extend(origin);
    }
    lines
}

/// The detail pane of the sessions list: of the selected session, or focused and scrolling the
/// one that is open.
fn render_session_detail(f: &mut Frame, app: &App, area: Rect) {
    let (session, scroll) = match &app.mode {
        Mode::SessionDetail { pid, scroll } => (app.session_of(*pid), *scroll),
        _ => (app.selected_session(), 0),
    };
    let focused = matches!(app.mode, Mode::SessionDetail { .. });
    let block = Block::bordered()
        .title(" Detail ")
        .border_style(border_style(focused));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = match session {
        Some(s) => session_lines(app, s),
        None if focused => vec![Line::styled("session is gone", dim())],
        None => Vec::new(),
    };
    let scroll = scroll_for(&lines, None, scroll, inner);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        inner,
    );
}

fn render_detail(f: &mut Frame, app: &App, area: Rect) {
    let (item, scroll) = match &app.mode {
        // SessionDetail is drawn by render_session_detail
        Mode::List | Mode::ConfirmDismissAll | Mode::Task { .. } | Mode::SessionDetail { .. } => {
            (app.selected_item(), 0)
        }
        Mode::Detail { id, scroll } => (app.find(id), *scroll),
        Mode::Password { id, .. } | Mode::Answer { id, .. } => (app.find(id), 0),
    };
    let focused = !matches!(
        app.mode,
        Mode::List | Mode::ConfirmDismissAll | Mode::Task { .. }
    );
    let title = match &app.mode {
        Mode::Answer { .. } if !app.raw => " Answer ",
        _ if app.raw => " Raw ",
        _ => " Detail ",
    };
    let block = Block::bordered()
        .title(title)
        .border_style(border_style(focused));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let input = input_line(&app.mode, inner.width);
    let [content, input_area] = if input.is_some() {
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner)
    } else {
        [inner, Rect::default()]
    };
    let (lines, cursor) = match (item, &app.mode) {
        (None, Mode::List | Mode::ConfirmDismissAll | Mode::Task { .. }) => (Vec::new(), None),
        (None, _) => (vec![Line::styled("item is gone", dim())], None),
        (Some(item), Mode::Answer { st, .. }) if !app.raw => answer_view(item, st),
        (Some(item), _) => (detail_view(app, item), None),
    };
    let scroll = scroll_for(&lines, cursor, scroll, content);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        content,
    );
    if let Some(line) = input {
        f.render_widget(Paragraph::new(line), input_area);
    }
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let ask = match &app.mode {
        Mode::ConfirmDismissAll => Some(format!(
            "Dismiss {} items without a reply?",
            app.without_reply().len()
        )),
        _ => None,
    };
    // the task popup has the focus: no status over its hints
    let status = app
        .status()
        .filter(|_| !matches!(app.mode, Mode::Task { .. }));
    let line = if let Some(ask) = ask {
        let mut spans = vec![
            Span::styled(
                ask,
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
        ];
        spans.extend(hint_spans(&[("y", "yes"), ("n", "no")], "  "));
        Line::from(spans)
    } else if let Some(status) = status {
        let color = if status.error {
            Color::Red
        } else {
            Color::Green
        };
        Line::styled(clean(&status.text), Style::new().fg(color))
    } else {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(hint_spans(&app.hints(), "  "));
        Line::from(spans)
    };
    f.render_widget(Paragraph::new(line), area);
}

/// `sessions_dir` is the session registry (`<config dir>/sessions`).
pub fn run(
    base: &str,
    runtime: Option<&Path>,
    sessions_dir: &Path,
    tabs: Vec<Tab>,
) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = execute!(io::stdout(), EnableBracketedPaste)
        .and_then(|()| event_loop(&mut terminal, base, runtime, sessions_dir, tabs));
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    base: &str,
    runtime: Option<&Path>,
    sessions_dir: &Path,
    tabs: Vec<Tab>,
) -> io::Result<()> {
    let mut app = App {
        tabs,
        ..App::default()
    };
    let mut refreshed: Option<Instant> = None;
    loop {
        if refreshed.is_none_or(|t| t.elapsed() >= REFRESH) {
            refresh(&mut app, base, runtime, sessions_dir);
            refreshed = Some(Instant::now());
        }
        app.now = client::now();
        let size = terminal.size()?;
        app.area = Rect::new(0, 0, size.width, size.height);
        terminal.draw(|f| render(f, &app))?;
        if event::poll(POLL)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    app.now = client::now(); // not the time of the draw: the guard is timed
                    let effect = app.on_key(key);
                    if let Effect::NewAgent(task) = &effect {
                        new_agent(terminal, &mut app, task)?;
                        refreshed = None; // a new session shows up in the registry
                        continue;
                    }
                    if perform(&mut app, effect, base, runtime, sessions_dir) {
                        return Ok(());
                    }
                }
                Event::Paste(text) => app.on_paste(&text),
                _ => {}
            }
        }
    }
}

/// Reloads the sessions (they need no router) and the inbox items, then reads the origins they
/// refer to.
fn refresh(app: &mut App, base: &str, runtime: Option<&Path>, sessions_dir: &Path) {
    let sessions = sessions::load(sessions_dir, Path::new("/proc"));
    let items = client::list(base).map_err(|e| e.to_string());
    update(app, sessions, items, runtime);
}

/// Applies a refresh. The origins of new items and sessions are read last and decide what the
/// tab shows, so the selection is restored after that, not only after each list.
fn update(
    app: &mut App,
    sessions: Vec<Session>,
    items: Result<Vec<Item>, String>,
    runtime: Option<&Path>,
) {
    let item = app.selected_item().map(|i| i.id.clone());
    let pid = app.selected_session().map(|s| s.pid);
    app.set_sessions(sessions);
    match items {
        Ok(items) => {
            app.offline = None;
            app.set_items(items);
        }
        Err(e) => app.offline = Some(e),
    }
    load_origins(app, runtime);
    app.select_id(item);
    app.select_pid(pid);
}

/// Reads the origin files of the items and sessions that have not been read yet (the files are
/// written once, before the session starts). Without a runtime dir there is no origin dir and
/// nothing is read.
fn load_origins(app: &mut App, runtime: Option<&Path>) {
    let Some(runtime) = runtime else {
        return;
    };
    let dir = runtime.join("claude-origin");
    let files = app
        .items
        .iter()
        .map(Item::origin_file)
        .chain(app.sessions.iter().map(|s| s.origin_file.as_str()));
    let new: Vec<String> = files
        .filter(|f| !f.is_empty() && !app.origins.contains_key(*f))
        .map(String::from)
        .collect();
    for file in new {
        let origin = Origin::load(Path::new(&file), &dir);
        app.origins.insert(file, origin);
    }
}

/// Carries out an effect, then refreshes; true = quit.
fn perform(
    app: &mut App,
    effect: Effect,
    base: &str,
    runtime: Option<&Path>,
    sessions_dir: &Path,
) -> bool {
    match effect {
        Effect::None => return false,
        Effect::Quit => return true,
        Effect::Dismiss(id) => dismiss(app, base, &id),
        Effect::DismissAll => {
            for id in app.without_reply() {
                dismiss(app, base, &id);
            }
        }
        Effect::Send(id, line) => send(app, base, runtime, &id, &line),
        Effect::Attach(name) => {
            attach(app, &name);
            return false;
        }
        Effect::Kill { pid, start } => kill(app, base, pid, start),
        Effect::Open(url) => {
            open_url(app, &url);
            return false;
        }
        Effect::NewAgent(_) => return false, // needs the terminal: `event_loop` does it
    }
    refresh(app, base, runtime, sessions_dir);
    false
}

/// SIGTERM to the Claude process `pid` if it is still the one that started at `start` (field 22
/// of `/proc/<pid>/stat`, see `sessions::alive`). Without a start time the pid cannot be told
/// from a reused one, so nothing is sent.
fn terminate(proc: &Path, pid: u32, start: Option<u64>) -> Result<(), String> {
    if start.is_none() {
        return Err(format!(
            "pid {pid} not killed: the registry has no start time, it could be another process"
        ));
    }
    // never kill(0 | -1 | -pgid): a pid above i32::MAX would be a negative one
    let raw = i32::try_from(pid)
        .ok()
        .filter(|&p| p > 0)
        .ok_or_else(|| format!("invalid pid {pid}"))?;
    if !sessions::alive(proc, pid, start) {
        return Err(format!("pid {pid} is already gone"));
    }
    // SAFETY: kill only takes integers; the process was just checked to be the session's.
    if unsafe { libc::kill(raw, libc::SIGTERM) } != 0 {
        return Err(format!("kill {pid}: {}", io::Error::last_os_error()));
    }
    Ok(())
}

/// Kills the session of process `pid`, then dismisses its inbox items.
fn kill(app: &mut App, base: &str, pid: u32, start: u64) {
    let place = app
        .session_of(pid)
        .map_or_else(|| format!("pid {pid}"), Session::place);
    let ids = app.session_item_ids(pid);
    if let Err(e) = terminate(Path::new("/proc"), pid, Some(start)) {
        app.notify(e, true);
        return;
    }
    app.notify(format!("killed {place}"), false);
    for id in ids {
        dismiss_replied(app, base, &id);
    }
}

/// Runs `zellij action <args>` without touching the TUI's terminal; Ok = its stdout.
fn zellij(args: &[&str]) -> Result<String, String> {
    let out = Command::new("zellij")
        .arg("action")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("zellij {}: {e}", args[0]))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("zellij {} failed: {}", args[0], err.trim()))
    }
}

/// Goes to the zellij tab named `name`, or opens one attached to the shpool session `name`.
fn attach(app: &mut App, name: &str) {
    if std::env::var_os("ZELLIJ").is_none() {
        app.notify("not inside zellij ($ZELLIJ is unset)", true);
        return;
    }
    let result = zellij(&["query-tab-names"]).and_then(|tabs| {
        if tabs.lines().any(|tab| tab == name) {
            zellij(&["go-to-tab-name", name])
        } else {
            zellij(&[
                "new-tab",
                "--name",
                name,
                "--close-on-exit",
                "--",
                "shpool",
                "attach",
                "-f",
                name,
            ])
        }
    });
    if let Err(e) = result {
        app.notify(e, true);
    }
}

/// Opens `url` in the user's browser with `xdg-open`, which may outlive the call (and prints
/// to the terminal of the TUI: its output is dropped).
fn open_url(app: &mut App, url: &str) {
    let spawned = Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            // its failure (no browser, headless) is not seen: say what was tried
            app.notify(clean(&format!("opening {url}")), false);
        }
        Err(e) => app.notify(format!("xdg-open: {e}"), true),
    }
}

/// The shpool session name from the last `shpool attach <name>` line of the stderr of
/// `jw agent -d` (which prints it when the session is started).
fn started_session(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .filter_map(|l| l.trim().strip_prefix("shpool attach "))
        .map(str::trim)
        .rfind(|name| !name.is_empty())
        .map(String::from)
}

/// What jw prints (last) when the user cancels its fzf.
const JW_CANCELLED: &str = "jw: cancelled";

/// What a `jw agent -d` run came to.
#[derive(Debug, PartialEq)]
enum Outcome {
    /// Go to the session (no task: the user picked the repo in fzf).
    Attach(String),
    /// The session runs in the background (a task was given).
    Started(String),
    Cancelled,
    Failed(String),
}

/// The outcome of `jw agent -d` for `task` from its exit status and its stderr.
fn outcome(task: &str, success: bool, stderr: &str) -> Outcome {
    if success {
        return match started_session(stderr) {
            Some(name) if task.is_empty() => Outcome::Attach(name),
            Some(name) => Outcome::Started(name),
            None => Outcome::Failed("jw agent -d: no session name in its output".into()),
        };
    }
    // with a task the stderr also has the routing lines: only the last one says why jw failed
    match stderr.lines().map(str::trim).rfind(|l| !l.is_empty()) {
        Some(JW_CANCELLED) => Outcome::Cancelled,
        Some(why) => Outcome::Failed(why.into()),
        None => Outcome::Failed("jw agent -d failed".into()),
    }
}

/// The arguments of `jw` for a new agent. `--` keeps a task like `-x` from being an option and
/// its first word from being taken for a repo.
fn jw_args(task: &str) -> Vec<String> {
    let mut args = vec!["agent".to_string(), "-d".to_string()];
    if !task.is_empty() {
        args.push("--".into());
        args.push(task.into());
    }
    args
}

extern "C" fn ignore_sigint(_: libc::c_int) {}

/// Waits for `jw` (its stderr is piped) and returns whether it succeeded and its stderr, which is
/// echoed line by line as jw prints it.
fn wait_echoing(mut jw: Child) -> io::Result<(bool, String)> {
    let pipe = jw.stderr.take().expect("stderr is piped");
    let echo = std::thread::spawn(move || {
        let mut stderr = String::new();
        let mut pipe = BufReader::new(pipe);
        let mut buf = Vec::new();
        // not `lines()`: it stops at output that is not UTF-8
        while matches!(pipe.read_until(b'\n', &mut buf), Ok(n) if n > 0) {
            let line = String::from_utf8_lossy(&buf);
            let line = line.trim_end_matches(['\r', '\n']);
            let _ = writeln!(io::stderr(), "{line}");
            stderr.push_str(line);
            stderr.push('\n');
            buf.clear();
        }
        stderr
    });
    let status = jw.wait();
    let stderr = echo.join().unwrap_or_default();
    Ok((status?.success(), stderr))
}

/// Runs `jw agent -d <task>`: whether it succeeded and its stderr.
fn run_jw(task: &str) -> io::Result<(bool, String)> {
    // jw is in our process group, so a Ctrl-C while it routes reaches us too. A handler that
    // does nothing, not SIG_IGN: jw would inherit that.
    let handler = ignore_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // SAFETY: the handler does nothing, so it is async-signal-safe.
    let previous = unsafe { libc::signal(libc::SIGINT, handler) };
    let result = Command::new("jw")
        .args(jw_args(task))
        .current_dir("/")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(wait_echoing);
    // SAFETY: restores the handler `signal` returned above.
    unsafe { libc::signal(libc::SIGINT, previous) };
    result
}

/// Starts a new agent: leaves the TUI's screen for `jw agent -d <task>` (jw picks the repo
/// the task fits; without a task it asks with fzf, which draws on /dev/tty; the cwd `/` makes it
/// always pick). Without a task it then goes to the session, with one the user stays in the
/// inbox. jw's stderr is shown while it runs and parsed for the name of the session and the
/// reason of a failure.
fn new_agent(terminal: &mut DefaultTerminal, app: &mut App, task: &str) -> io::Result<()> {
    // what `ratatui::init` and `restore` do, without their panic hook
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableBracketedPaste)?;
    if !task.is_empty() {
        println!("New agent: {task}"); // jw's progress follows, e.g. "choosing a repo..."
    }
    let ran = run_jw(task);
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    enable_raw_mode()?;
    // SAFETY: tcflush only takes integers; it drops what was typed after fzf exited, which
    // would otherwise reach the TUI as commands.
    unsafe { libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH) };
    // what crossterm has read already (the rest of a pasted task) must not become commands either
    while event::poll(Duration::ZERO)? {
        event::read()?;
    }
    terminal.clear()?;
    app.now = client::now(); // fzf may have taken a while: the status starts now
    let (success, stderr) = match ran {
        Ok(ran) => ran,
        Err(e) => {
            app.notify(format!("jw: {e}"), true);
            return Ok(());
        }
    };
    match outcome(task, success, &stderr) {
        Outcome::Attach(name) if std::env::var_os("ZELLIJ").is_some() => attach(app, &name),
        Outcome::Attach(name) => {
            app.notify(format!("started {name} (shpool attach {name})"), false);
        }
        Outcome::Started(name) => app.notify(format!("started {name}"), false),
        Outcome::Cancelled => app.notify("cancelled", false), // e.g. Esc in fzf: not an error
        Outcome::Failed(why) => app.notify(why, true),
    }
    Ok(())
}

fn dismiss(app: &mut App, base: &str, id: &str) {
    match client::dismiss(base, id) {
        Ok(true) => {}
        Ok(false) => app.notify("item is gone", true),
        Err(e) => app.notify(format!("dismiss failed: {e}"), true),
    }
}

/// Dismisses an item whose reply was dealt with: already gone (404) is fine and keeps the
/// status of the reply.
fn dismiss_replied(app: &mut App, base: &str, id: &str) {
    if let Err(e) = client::dismiss(base, id) {
        app.notify(format!("dismiss failed: {e}"), true);
    }
}

fn send(app: &mut App, base: &str, runtime: Option<&Path>, id: &str, line: &str) {
    let Some(item) = app.find(id) else {
        app.notify("item is gone", true);
        return;
    };
    let target = item.reply().map(PathBuf::from);
    let prefix = reply::reply_prefix(item.kind());
    let (Some(target), Some(prefix)) = (target, prefix) else {
        app.notify("this item cannot be answered", true);
        return;
    };
    let Some(runtime) = runtime else {
        app.notify("XDG_RUNTIME_DIR is not set", true);
        return;
    };
    match reply::write_reply(runtime, prefix, &target, line) {
        Ok(()) => {
            app.notify("sent", false);
            dismiss_replied(app, base, id);
        }
        Err(ReplyError::Expired) => {
            app.notify("expired: nothing is waiting for the answer", true);
            dismiss_replied(app, base, id);
        }
        Err(e) => app.notify(e.to_string(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::json;

    const AGENT_CWD: &str = "/home/u/projects/infra.agents/s-1012-002032-2abc";
    const HOOK_REPLY: &str = "/run/user/1/claude-hook-x/reply";
    const ORIGIN_FILE: &str = "/run/user/1/claude-origin/o1.json";

    fn item(id: &str, kind: &str, title: &str, received: f64, reply: Option<&str>) -> Item {
        let mut event = json!({"type": kind, "project": "proj", "title": title, "cwd": AGENT_CWD,
            "origin_file": ORIGIN_FILE});
        if let Some(r) = reply {
            event["reply"] = json!(r);
            event["command"] = json!("sudo ls /root");
            event["prompt"] = json!("[sudo] password for me:");
        }
        Item {
            id: id.into(),
            received,
            event,
        }
    }

    fn sudo(id: &str) -> Item {
        item(
            id,
            "sudo.password_request",
            "sudo ls",
            990.0,
            Some("/run/user/1/claude-askpass-x/reply"),
        )
    }

    /// A claude.question without reply or questions.
    fn question(id: &str, received: f64) -> Item {
        item(
            id,
            "claude.question",
            &format!("title-{id}"),
            received,
            None,
        )
    }

    fn app(items: Vec<Item>) -> App {
        let mut app = App {
            area: Rect::new(0, 0, 100, 30),
            now: 1000.0,
            ..App::default()
        };
        app.set_items(items);
        app
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn screen_at(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn screen(app: &App) -> String {
        screen_at(app, 100, 30)
    }

    fn claude(id: &str, kind: &str, tool: &str, reply: Option<&str>, tool_input: Value) -> Item {
        let mut event = json!({"type": kind, "project": "proj", "title": format!("title-{id}"),
            "cwd": AGENT_CWD, "origin_file": ORIGIN_FILE, "tool_name": tool,
            "tool_input": tool_input});
        if let Some(r) = reply {
            event["reply"] = json!(r);
        }
        Item {
            id: id.into(),
            received: 990.0,
            event,
        }
    }

    fn permission(id: &str, reply: Option<&str>) -> Item {
        claude(
            id,
            "claude.permission_request",
            "Bash",
            reply,
            json!({"command": "ls -la", "description": "List files"}),
        )
    }

    fn ask(id: &str, reply: Option<&str>) -> Item {
        claude(
            id,
            "claude.question",
            "AskUserQuestion",
            reply,
            json!({"questions": [
                {"question": "Which db?", "header": "Database", "multiSelect": false, "options": [
                    {"label": "Postgres", "description": "relational"}, {"label": "SQLite"}]},
                {"question": "Which features?", "multiSelect": true, "options": [
                    {"label": "Auth"}, {"label": "Cache"}, {"label": "Logs"}]}
            ]}),
        )
    }

    fn in_session(mut item: Item, session: &str) -> Item {
        item.event["session_id"] = json!(session);
        item
    }

    fn type_text(a: &mut App, text: &str) {
        for c in text.chars() {
            assert_eq!(a.on_key(key(c)), Effect::None);
        }
    }

    fn allow(id: &str) -> Effect {
        Effect::Send(id.into(), r#"{"behavior":"allow"}"#.into())
    }

    fn deny(id: &str) -> Effect {
        Effect::Send(id.into(), r#"{"behavior":"deny"}"#.into())
    }

    // --- layout and rendering

    #[test]
    fn list_row_shows_marker_badge_project_title_and_age() {
        let a = app(vec![question("a", 990.0), ask("q", Some(HOOK_REPLY))]);
        for (w, h) in [(100, 30), (60, 30)] {
            let s = screen_at(&a, w, h);
            let row = s
                .lines()
                .find(|l| l.contains("title-a") && l.contains("ASK"))
                .unwrap_or_else(|| panic!("no list row at {w}x{h}:\n{s}"));
            assert!(row.contains("infra") && row.contains("10s"), "{row}");
            assert!(row.contains('○'), "{row}");
            let row = s.lines().find(|l| l.contains("title-q")).unwrap();
            assert!(row.contains('●') && row.contains("ASK"), "{row}");
        }
    }

    #[test]
    fn header_counts_and_router_status() {
        let mut a = app(vec![
            question("a", 900.0),
            permission("p", Some(HOOK_REPLY)),
            question("b", 950.0),
        ]);
        let s = screen(&a);
        assert!(s.contains("agent-inbox"));
        assert!(s.contains("3 pending · 1 answerable"));
        assert!(s.contains("connected"));
        a.offline = Some("connection refused".into());
        let s = screen(&a);
        assert!(s.contains("router offline: connection refused"));
        assert!(!s.contains("connected"));
        // on a narrow terminal the counts give way, the router status stays
        let s = screen_at(&a, 60, 30);
        assert!(
            s.contains("agent-inbox") && s.contains("router offline: conn"),
            "{s}"
        );
    }

    #[test]
    fn empty_inbox_keeps_the_panes() {
        let s = screen(&app(vec![]));
        assert!(s.contains("No pending requests"), "{s}");
        assert!(s.contains("0 pending"), "{s}");
        assert!(
            s.contains("All 0 · Sessions 0") && s.contains(" Detail "),
            "{s}"
        );
        assert!(s.matches('┌').count() >= 2, "{s}");
    }

    #[test]
    fn empty_sessions_tab_keeps_the_panes_and_the_tab_bar() {
        let s = screen(&sessions_app(vec![], vec![]));
        assert!(s.contains("No Claude sessions"), "{s}");
        assert!(
            s.contains("All 0 · Sessions 0") && s.contains(" Detail "),
            "{s}"
        );
        assert!(s.matches('┌').count() >= 2, "{s}");
    }

    #[test]
    fn long_titles_are_truncated_by_chars() {
        let long = "ä".repeat(200);
        let a = app(vec![item("a", "claude.question", &long, 990.0, None)]);
        let s = screen(&a);
        assert!(s.contains('…'));
        assert!(!s.contains(&"ä".repeat(60)));
    }

    #[test]
    fn permission_detail_shows_command_and_decision_keys() {
        let a = app(vec![permission("p", Some(HOOK_REPLY))]);
        for (w, h) in [(100, 30), (60, 30)] {
            let mut a2 = app(vec![permission("p", Some(HOOK_REPLY))]);
            a2.area = Rect::new(0, 0, w, h);
            let s = screen_at(&a2, w, h);
            assert!(s.contains("Bash"), "{w}x{h}:\n{s}");
            assert!(s.contains("ls -la"), "{w}x{h}:\n{s}");
            assert!(s.contains("List files"), "{w}x{h}:\n{s}");
            assert!(s.contains("y allow · n deny"), "{w}x{h}:\n{s}");
            assert!(s.contains("cwd: /home/u/projects/infra.agents/"), "{w}x{h}");
            // the footer offers the decision only once the detail is open
            assert!(!s.contains("y allow  n deny"), "{w}x{h}:\n{s}");
            a2.on_key(code(KeyCode::Enter));
            let s = screen_at(&a2, w, h);
            assert!(s.contains("y allow  n deny"), "{w}x{h}:\n{s}");
        }
        // no raw JSON unless asked for
        assert!(!screen(&a).contains("\"tool_name\""));
    }

    #[test]
    fn jj_location_replaces_the_cwd() {
        let mut p = permission("p", Some(HOOK_REPLY));
        p.event["jj"] = json!({"repo": "real", "workspace": "s-1", "subdir": "",
            "change": "abc", "stack": {"index": 2, "size": 3}});
        let s = screen(&app(vec![p]));
        let row = s.lines().find(|l| l.contains("title-p")).unwrap();
        assert!(row.contains("real:s-1"), "{row}");
        assert!(s.contains("jj: real ws:s-1 @abc 2/3"), "{s}");
        assert!(!s.contains("cwd:"), "{s}");
    }

    #[test]
    fn permission_detail_per_tool() {
        let edit = |tool: &str, input: Value| {
            claude(
                "e",
                "claude.permission_request",
                tool,
                Some(HOOK_REPLY),
                input,
            )
        };
        let s = screen(&app(vec![edit(
            "Write",
            json!({"file_path": "/etc/motd", "content": "a\nb\nc\n"}),
        )]));
        assert!(s.contains("/etc/motd") && s.contains("3 lines"));
        let s = screen(&app(vec![edit(
            "WebFetch",
            json!({"url": "https://example.com/x"}),
        )]));
        assert!(s.contains("https://example.com/x"));
        let s = screen(&app(vec![edit(
            "Grep",
            json!({"pattern": "needle", "path": "src"}),
        )]));
        assert!(s.contains("pattern: needle") && s.contains("path: src"));
    }

    #[test]
    fn sudo_detail_shows_command_and_prompt() {
        let s = screen(&app(vec![sudo("s")]));
        assert!(s.contains("SUDO"));
        assert!(s.contains("sudo ls /root"));
        assert!(s.contains("[sudo] password for me:"));
        assert!(s.contains("p password"));
    }

    #[test]
    fn question_detail_lists_questions_and_options() {
        let s = screen(&app(vec![ask("q", Some(HOOK_REPLY))]));
        assert!(s.contains("Which db?"));
        assert!(s.contains("[Database]"));
        assert!(s.contains("Postgres"));
        assert!(s.contains("relational"));
        assert!(s.contains("SQLite"));
        assert!(s.contains("Which features?"));
        assert!(s.contains("[multi]"));
        // `n` denies only in the answer choice, not from the list
        assert!(s.contains("enter answer"));
        assert!(!s.contains("n deny"));
    }

    #[test]
    fn read_only_items_say_why() {
        let s = screen(&app(vec![permission("p", None)]));
        assert!(s.contains("read-only"));
        assert!(s.contains("CLAUDE_INBOX_WAIT"));
        assert!(!s.contains("y allow"));
        let s = screen(&app(vec![item(
            "x",
            "sudo.password_request",
            "t",
            990.0,
            None,
        )]));
        assert!(s.contains("read-only"));
        assert!(!s.contains("CLAUDE_INBOX_WAIT"));
    }

    #[test]
    fn question_mark_toggles_raw_json() {
        let mut a = app(vec![permission("p", Some(HOOK_REPLY))]);
        assert_eq!(a.on_key(key('?')), Effect::None);
        let s = screen(&a);
        assert!(s.contains("\"tool_name\": \"Bash\""));
        assert!(s.contains("\"command\": \"ls -la\""));
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('?'));
        assert!(!screen(&a).contains("\"tool_name\""));
        assert!(screen(&a).contains("List files"));
    }

    #[test]
    fn untrusted_text_is_cleaned() {
        let mut a = app(vec![item("a", "claude.question", "t\u{9b}x", 990.0, None)]);
        assert!(!screen(&a).contains('\u{9b}'));
        a.on_key(key('?'));
        assert!(!screen(&a).contains('\u{9b}'));
        let a = app(vec![claude(
            "b",
            "claude.permission_request",
            "Bash",
            Some(HOOK_REPLY),
            json!({"command": "echo \u{1b}[31mred\nsecond"}),
        )]);
        let s = screen(&a);
        assert!(!s.contains('\u{1b}'));
        assert!(s.contains("second"));
    }

    #[test]
    fn status_is_shown_and_expires() {
        let mut a = app(vec![]);
        a.notify("all good", false);
        assert!(screen(&a).contains("all good"));
        a.now += STATUS_TTL + 1.0;
        assert!(!screen(&a).contains("all good"));
    }

    #[test]
    fn wrapped_height_counts_display_width() {
        let h = |s: &str, w: u16| wrapped_height(&[Line::raw(s.to_string())], w);
        assert_eq!(h("aaa bbb ccc", 7), 2);
        assert_eq!(h("aaa bbb ccc", 11), 1);
        assert_eq!(h(&"x".repeat(20), 8), 3);
        // 10 wide characters are 20 cells
        assert_eq!(h(&"字".repeat(10), 10), 2);
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        let key_sets: Vec<(Vec<Item>, Vec<KeyEvent>)> = vec![
            (vec![], vec![]),
            (vec![sudo("s")], vec![]),
            (vec![sudo("s")], vec![key('?')]),
            (vec![sudo("s")], vec![code(KeyCode::Enter)]),
            (vec![sudo("s")], vec![code(KeyCode::Enter), key('?')]),
            (
                vec![sudo("s")],
                vec![
                    code(KeyCode::Enter),
                    code(KeyCode::Enter),
                    key('a'),
                    key('b'),
                ],
            ),
            (
                vec![ask("q", Some(HOOK_REPLY))],
                vec![code(KeyCode::Enter), key('?')],
            ),
            (
                vec![ask("q", Some(HOOK_REPLY))],
                vec![
                    code(KeyCode::Enter),
                    key('j'),
                    key('j'),
                    code(KeyCode::Enter),
                    key('x'),
                ],
            ),
            (vec![question("a", 990.0)], vec![key('D')]),
            (vec![sudo("s")], vec![key('f'), key('x'), key('a')]),
            (vec![sudo("s")], vec![key('f'), code(KeyCode::Enter)]),
            (vec![sudo("s")], vec![key('f'), key('f'), key('f')]),
            (vec![], vec![key('N'), key('a'), ctrl_j(), key('b')]),
        ];
        for (w, h) in [(20, 5), (1, 1), (2, 2)] {
            for (items, keys) in &key_sets {
                let mut a = app(items.clone());
                a.set_sessions(vec![session(1, "s-a", WAITING)]);
                a.area = Rect::new(0, 0, w, h);
                a.notify("status", true);
                a.offline = Some("down".into());
                for k in keys {
                    a.on_key(*k);
                }
                screen_at(&a, w, h);
            }
        }
    }

    #[test]
    fn detail_scrolls_to_the_end_of_long_content() {
        let cmd = (0..60)
            .map(|i| format!("cmd-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut a = app(vec![claude(
            "p",
            "claude.permission_request",
            "Bash",
            Some(HOOK_REPLY),
            json!({"command": cmd}),
        )]);
        assert!(!screen(&a).contains("cmd-39"));
        a.on_key(code(KeyCode::Enter));
        for _ in 0..5 {
            assert_eq!(a.on_key(code(KeyCode::PageDown)), Effect::None);
        }
        let s = screen(&a);
        assert!(s.contains("cmd-39"));
        assert!(!s.contains("cmd-0"));
        // not past the end, and back up again
        for _ in 0..5 {
            a.on_key(code(KeyCode::PageUp));
        }
        assert!(screen(&a).contains("cmd-0"));
    }

    // --- list and focus

    #[test]
    fn selection_moves_newest_first_and_clamps() {
        let mut a = app(vec![question("old", 900.0), question("new", 990.0)]);
        assert_eq!(a.on_key(key('d')), Effect::Dismiss("new".into()));
        a.on_key(key('j'));
        a.on_key(code(KeyCode::Down));
        assert_eq!(a.on_key(key('d')), Effect::Dismiss("old".into()));
        a.on_key(code(KeyCode::Up));
        a.on_key(key('k'));
        assert_eq!(a.on_key(key('d')), Effect::Dismiss("new".into()));
    }

    #[test]
    fn selection_follows_its_item_across_refreshes() {
        let mut a = app(vec![question("old", 900.0), question("new", 990.0)]);
        a.on_key(key('j'));
        a.set_items(vec![
            question("old", 900.0),
            question("new", 990.0),
            question("newer", 995.0),
        ]);
        assert_eq!(a.on_key(key('d')), Effect::Dismiss("old".into()));
    }

    #[test]
    fn detail_shows_the_selected_item_without_focus() {
        let mut a = app(vec![question("old", 900.0), question("new", 990.0)]);
        assert!(screen(&a).contains("Detail"));
        a.on_key(key('j'));
        // the detail pane has the title of the selected item too: two rows mention it
        let s = screen(&a);
        assert_eq!(s.matches("title-old").count(), 2);
        assert_eq!(s.matches("title-new").count(), 1);
    }

    #[test]
    fn quit_keys() {
        assert_eq!(app(vec![]).on_key(key('q')), Effect::Quit);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app(vec![question("a", 990.0)]).on_key(ctrl_c), Effect::Quit);
    }

    #[test]
    fn detail_focus_opens_and_closes_and_dismisses() {
        let mut a = app(vec![question("a", 990.0)]);
        a.on_key(code(KeyCode::Enter));
        assert!(matches!(a.mode, Mode::Detail { .. }));
        assert!(screen(&a).contains("esc back"));
        a.on_key(code(KeyCode::Esc));
        assert_eq!(a.on_key(key('q')), Effect::Quit);
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('d')), Effect::Dismiss("a".into()));
        assert_eq!(a.on_key(key('q')), Effect::Quit);
    }

    #[test]
    fn attach_key_returns_the_shpool_session_from_list_detail_and_answer() {
        let with_shpool = |mut item: Item| {
            item.event["shpool"] = json!("s-1");
            item
        };
        let attach = Effect::Attach("s-1".into());
        let mut a = app(vec![with_shpool(question("a", 990.0))]);
        assert_eq!(a.on_key(key('a')), attach);
        assert!(matches!(a.mode, Mode::List));
        assert!(a.status().is_none());
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('a')), attach);
        assert!(matches!(a.mode, Mode::Detail { .. }));
        // the choice stays open
        let mut a = app(vec![with_shpool(ask("q", Some(HOOK_REPLY)))]);
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('a')), attach);
        assert!(matches!(a.mode, Mode::Answer { .. }));
    }

    #[test]
    fn attach_key_without_shpool_session_is_an_error() {
        let mut a = app(vec![question("a", 990.0)]);
        assert_eq!(a.on_key(key('a')), Effect::None);
        let status = a.status().expect("a status message");
        assert!(status.error && status.text.contains("no shpool session"));
        assert!(matches!(a.mode, Mode::List));
    }

    #[test]
    fn attach_hint_is_in_the_footer() {
        let mut item = question("a", 990.0);
        item.event["shpool"] = json!("s-1");
        let mut a = app(vec![item]);
        assert!(screen(&a).contains("a attach"));
        a.on_key(code(KeyCode::Enter));
        assert!(screen(&a).contains("a attach"));
        // nothing to attach to
        let mut a = app(vec![question("a", 990.0)]);
        assert!(!screen(&a).contains("attach"));
        a.on_key(code(KeyCode::Enter));
        assert!(!screen(&a).contains("attach"));
    }

    #[test]
    fn tab_moves_focus_between_list_and_detail() {
        let mut a = app(vec![question("a", 990.0)]);
        a.on_key(code(KeyCode::Tab));
        assert!(matches!(a.mode, Mode::Detail { .. }));
        a.on_key(code(KeyCode::Tab));
        assert!(matches!(a.mode, Mode::List));
        // nothing to focus
        let mut a = app(vec![]);
        a.on_key(code(KeyCode::Enter));
        assert!(matches!(a.mode, Mode::List));
    }

    #[test]
    fn detail_reports_item_that_disappears() {
        let mut a = app(vec![question("a", 990.0)]);
        a.on_key(code(KeyCode::Enter));
        a.set_items(vec![]);
        assert!(screen(&a).contains("item is gone"));
        // other keys are swallowed, Esc/q leave the dead detail view
        assert_eq!(a.on_key(key('x')), Effect::None);
        assert!(matches!(a.mode, Mode::Detail { .. }));
        assert_eq!(a.on_key(key('q')), Effect::None);
        assert!(matches!(a.mode, Mode::List));
        assert_eq!(a.on_key(key('q')), Effect::Quit);
    }

    #[test]
    fn vanished_detail_never_decides() {
        let mut a = app(vec![permission("p", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.set_items(vec![permission("p2", Some(HOOK_REPLY))]);
        for c in ['y', 'y', 'n', 'd', 'a', 'D'] {
            assert_eq!(a.on_key(key(c)), Effect::None);
        }
        assert!(matches!(a.mode, Mode::Detail { .. }));
        assert_eq!(a.on_key(code(KeyCode::Esc)), Effect::None);
        assert!(matches!(a.mode, Mode::List));
    }

    #[test]
    fn dismiss_all_asks_for_confirmation() {
        let mut a = app(vec![question("a", 900.0), sudo("s"), question("b", 950.0)]);
        let capital_d = KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT);
        assert_eq!(a.on_key(capital_d), Effect::None);
        assert!(screen(&a).contains("Dismiss 2 items without a reply?"));
        // Ctrl+Y does not confirm, nor does anything but `y`
        assert_eq!(
            a.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL)),
            Effect::None
        );
        assert_eq!(a.on_key(key('y')), Effect::DismissAll);
        let mut ids = a.without_reply();
        ids.sort();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
        // cancelling: the key is not a command
        a.on_key(capital_d);
        assert_eq!(a.on_key(key('d')), Effect::None);
        assert!(!screen(&a).contains("Dismiss 2"));
        assert_eq!(a.on_key(key('q')), Effect::Quit);
    }

    #[test]
    fn dismiss_all_without_candidates_asks_nothing() {
        let mut a = app(vec![sudo("s")]);
        a.on_key(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT));
        assert!(matches!(a.mode, Mode::List));
        assert!(screen(&a).contains("no read-only items"));
    }

    // --- sudo password

    #[test]
    fn sudo_item_sends_typed_password() {
        let mut a = app(vec![sudo("s")]);
        // `p` does nothing in the list, the detail opens the prompt
        a.on_key(key('p'));
        assert!(matches!(a.mode, Mode::List));
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('p'));
        type_text(&mut a, "hunter2");
        a.on_key(key('x'));
        a.on_key(code(KeyCode::Backspace));
        assert_eq!(
            a.on_key(code(KeyCode::Enter)),
            Effect::Send("s".into(), "hunter2".into())
        );
        assert!(!screen(&a).contains("password:"));
    }

    #[test]
    fn password_mode_masks_input_inline() {
        let mut a = app(vec![sudo("s")]);
        a.on_key(code(KeyCode::Enter));
        a.on_key(code(KeyCode::Enter));
        type_text(&mut a, "hunter2");
        let s = screen(&a);
        assert!(s.contains("password: *******"));
        assert!(!s.contains("hunter"));
        // still the detail of the item
        assert!(s.contains("sudo ls /root"));
    }

    #[test]
    fn password_mode_swallows_command_keys_and_esc_clears() {
        let mut a = app(vec![sudo("s")]);
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('p'));
        assert_eq!(a.on_key(key('d')), Effect::None);
        assert_eq!(a.on_key(key('q')), Effect::None);
        a.on_key(code(KeyCode::Esc));
        a.on_key(key('p'));
        // the buffer is empty again: nothing to send
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        assert!(screen(&a).contains("password: "));
    }

    #[test]
    fn password_mode_ignores_ctrl_and_alt_characters() {
        let mut a = app(vec![sudo("s")]);
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('p'));
        type_text(&mut a, "ab");
        for modifier in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            assert_eq!(
                a.on_key(KeyEvent::new(KeyCode::Char('x'), modifier)),
                Effect::None
            );
        }
        assert_eq!(
            a.on_key(code(KeyCode::Enter)),
            Effect::Send("s".into(), "ab".into())
        );
    }

    #[test]
    fn password_mode_survives_item_disappearing() {
        let mut a = app(vec![sudo("s")]);
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('p'));
        a.on_key(key('x'));
        a.set_items(vec![]);
        // the rest of the password is not taken for list commands
        for c in "dqD".chars() {
            assert_eq!(a.on_key(key(c)), Effect::None);
        }
        assert!(screen(&a).contains("item is gone"));
        assert!(screen(&a).contains("password: ****"));
        assert_eq!(
            a.on_key(code(KeyCode::Enter)),
            Effect::Send("s".into(), "xdqD".into())
        );
    }

    // --- read-only items

    #[test]
    fn claude_items_without_reply_are_read_only() {
        let mut a = app(vec![permission("p", None)]);
        for c in ['y', 'n', 'a', 'p'] {
            assert_eq!(a.on_key(key(c)), Effect::None);
        }
        a.on_key(code(KeyCode::Enter));
        for c in ['y', 'n', 'a', 'p'] {
            assert_eq!(a.on_key(key(c)), Effect::None);
        }
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        let mut a = app(vec![ask("q", None)]);
        a.on_key(key('a'));
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('a'));
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('n')), Effect::None);
        assert!(!screen(&a).contains("Question 1/2"));
    }

    #[test]
    fn question_without_questions_is_read_only() {
        let mut a = app(vec![claude(
            "q",
            "claude.question",
            "AskUserQuestion",
            Some(HOOK_REPLY),
            json!({}),
        )]);
        a.on_key(code(KeyCode::Enter));
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('n')), Effect::None);
        assert!(screen(&a).contains("read-only"));
    }

    #[test]
    fn waiting_item_shows_its_shpool_session_and_plain_read_only() {
        let mut w = claude("w", "claude.waiting", "", None, json!({}));
        w.event["shpool"] = json!("x");
        let s = screen(&app(vec![w]));
        assert!(s.contains("WAIT"), "{s}");
        assert!(s.contains("shpool: x"), "{s}");
        assert!(s.contains("read-only"), "{s}");
        assert!(!s.contains("hook isn't waiting"), "{s}");
    }

    #[test]
    fn claude_items_never_open_the_password_prompt() {
        for item in [
            permission("p", Some(HOOK_REPLY)),
            ask("q", Some(HOOK_REPLY)),
            question("r", 990.0),
        ] {
            let mut a = app(vec![item]);
            a.on_key(key('p'));
            a.on_key(code(KeyCode::Enter));
            a.on_key(key('p'));
            assert!(!screen(&a).contains("password:"));
        }
    }

    // --- permissions

    #[test]
    fn permission_decisions_work_only_from_the_detail() {
        let mut a = app(vec![permission("p", Some(HOOK_REPLY))]);
        // not from the list
        for c in ['y', 'n', 'a', 'p'] {
            assert_eq!(a.on_key(key(c)), Effect::None);
            assert!(matches!(a.mode, Mode::List));
        }
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('y')), allow("p"));
        // a decision returns the focus to the list: a held key decides at most one item
        assert!(matches!(a.mode, Mode::List));
        assert_eq!(a.on_key(key('y')), Effect::None);
        assert_eq!(a.on_key(key('n')), Effect::None);
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('n')), deny("p"));
        assert_eq!(a.on_key(key('n')), Effect::None);
        // `a` does nothing without a shpool session
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('a')), Effect::None);
        assert!(matches!(a.mode, Mode::Detail { .. }));
    }

    #[test]
    fn held_decision_key_decides_one_item() {
        let mut a = app(vec![
            permission("p1", Some(HOOK_REPLY)),
            permission("p2", Some(HOOK_REPLY)),
        ]);
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('y')), allow("p2"));
        // the next item moves under the selection, the repeats find no detail to decide in
        a.set_items(vec![permission("p1", Some(HOOK_REPLY))]);
        for _ in 0..3 {
            assert_eq!(a.on_key(key('y')), Effect::None);
        }
    }

    #[test]
    fn enter_never_allows_a_permission() {
        let mut a = app(vec![permission("p", Some(HOOK_REPLY))]);
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        assert!(matches!(a.mode, Mode::Detail { .. }));
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        assert!(matches!(a.mode, Mode::Detail { .. }));
    }

    #[test]
    fn decision_keys_need_a_selected_item() {
        let mut a = app(vec![]);
        assert_eq!(a.on_key(key('y')), Effect::None);
        assert_eq!(a.on_key(key('n')), Effect::None);
        assert_eq!(a.on_key(key('d')), Effect::None);
    }

    #[test]
    fn ctrl_or_alt_decision_keys_do_nothing() {
        for modifier in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            for focus_detail in [false, true] {
                let mut a = app(vec![permission("p", Some(HOOK_REPLY))]);
                if focus_detail {
                    a.on_key(code(KeyCode::Enter));
                }
                for c in ['y', 'n', 'd', 'q', 'a', 'p'] {
                    assert_eq!(
                        a.on_key(KeyEvent::new(KeyCode::Char(c), modifier)),
                        Effect::None
                    );
                }
                // still where it was: the plain key decides, but only from the detail
                let expected = if focus_detail {
                    allow("p")
                } else {
                    Effect::None
                };
                assert_eq!(a.on_key(key('y')), expected);
            }
        }
    }

    #[test]
    fn question_n_denies() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        assert_eq!(a.on_key(key('n')), Effect::None);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        assert_eq!(a.on_key(key('n')), deny("q"));
        assert_eq!(a.on_key(key('n')), Effect::None);
    }

    #[test]
    fn enter_opens_the_answer_choice_and_a_held_enter_does_not_pick() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        assert!(matches!(a.mode, Mode::Answer { .. }));
        assert!(screen(&a).contains("Question 1/2"));
        // the first repeat of a held Enter comes after the initial key-repeat delay, then fast
        a.now += 0.6;
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        for _ in 0..50 {
            a.now += 0.03;
            assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        }
        assert!(matches!(&a.mode, Mode::Answer { st, .. } if st.idx == 0 && st.done.is_empty()));
        // released: the next Enter picks the first option
        a.now += 1.1;
        a.on_key(code(KeyCode::Enter));
        assert!(screen(&a).contains("✓ Which db? → Postgres"));
    }

    #[test]
    fn a_different_key_right_after_opening_picks_at_once() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('2'));
        assert!(screen(&a).contains("✓ Which db? → SQLite"));
        // and navigation is not guarded
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('j'));
        assert!(screen(&a).contains("> 2 SQLite"));
    }

    // --- answering questions

    #[test]
    fn answering_single_and_multi_select_sends_allow_with_labels() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        // the list opens the choice
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        let s = screen(&a);
        assert!(s.contains("Question 1/2"));
        assert!(s.contains("> 1 Postgres"));
        assert!(s.contains("2 SQLite"));
        assert!(s.contains("Other…"));
        assert!(s.contains("1-9 pick  j/k move  l/enter select  h back  n deny  d dismiss"));
        // SQLite
        a.on_key(key('j'));
        assert!(screen(&a).contains("> 2 SQLite"));
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        a.now += 1.0;
        let s = screen(&a);
        assert!(s.contains("Question 2/2"));
        assert!(s.contains("✓ Which db? → SQLite"));
        assert!(s.contains("1 [ ] Auth"));
        assert!(s.contains("1-9/space toggle"));
        // Logs, then Auth (the answer follows the option order)
        a.on_key(key('j'));
        a.on_key(key('j'));
        a.on_key(key(' '));
        a.on_key(code(KeyCode::Up));
        a.on_key(key('k'));
        a.on_key(key(' '));
        let s = screen(&a);
        assert!(s.contains("[x] Auth") && s.contains("[x] Logs") && s.contains("[ ] Cache"));
        // space again takes Logs back out, and in again
        a.on_key(key('j'));
        a.on_key(key('j'));
        a.on_key(key(' '));
        a.on_key(key(' '));
        let Effect::Send(id, line) = a.on_key(code(KeyCode::Enter)) else {
            panic!("expected a send after the last question");
        };
        assert_eq!(id, "q");
        let sent: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            sent,
            json!({"behavior": "allow",
                "answers": {"Which db?": "SQLite", "Which features?": "Auth, Logs"}})
        );
        assert!(matches!(a.mode, Mode::List));
    }

    #[test]
    fn answering_with_other_text() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        // Postgres, SQLite, Other…
        a.on_key(key('j'));
        a.on_key(key('j'));
        a.on_key(key('j')); // clamps on Other…
        a.on_key(code(KeyCode::Enter));
        assert!(screen(&a).contains("Other: "));
        type_text(&mut a, "MySQL");
        assert!(screen(&a).contains("Other: MySQL"));
        // Esc goes back to the options, the text is dropped
        assert_eq!(a.on_key(code(KeyCode::Esc)), Effect::None);
        let s = screen(&a);
        assert!(s.contains("Question 1/2") && !s.contains("Other: "));
        // Enter on an empty text does not accept it
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        assert!(screen(&a).contains("Question 1/2"));
        type_text(&mut a, "  My SQL ");
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        a.now += 1.0;
        assert!(screen(&a).contains("Question 2/2"));
        // multi-select: Cache, then Other… finishes the question with both
        a.on_key(key('j'));
        a.on_key(key(' '));
        a.on_key(key('j'));
        a.on_key(key('j'));
        a.on_key(key(' '));
        assert!(screen(&a).contains("Other: "));
        type_text(&mut a, "Metrics");
        let Effect::Send(_, line) = a.on_key(code(KeyCode::Enter)) else {
            panic!("expected a send after the last question");
        };
        let sent: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            sent,
            json!({"behavior": "allow",
                "answers": {"Which db?": "My SQL", "Which features?": "Cache, Metrics"}})
        );
    }

    #[test]
    fn multi_select_needs_an_answer() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        let s = screen(&a);
        assert!(s.contains("Question 2/2"));
        assert!(s.contains("select at least one"));
    }

    #[test]
    fn a_held_enter_answers_only_the_first_question() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        // the repeats find the second question guarded
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        assert!(matches!(&a.mode, Mode::Answer { st, .. } if st.idx == 1 && st.done.len() == 1));
        // released: the next Enter answers it (after one option is picked)
        a.now += 1.0;
        a.on_key(key(' '));
        let Effect::Send(id, line) = a.on_key(code(KeyCode::Enter)) else {
            panic!("expected a send after the last question");
        };
        assert_eq!(id, "q");
        let sent: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            sent,
            json!({"behavior": "allow",
                "answers": {"Which db?": "Postgres", "Which features?": "Auth"}})
        );
    }

    #[test]
    fn esc_and_h_leave_answering_and_it_starts_over() {
        for leave in [code(KeyCode::Esc), key('h')] {
            let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
            a.on_key(code(KeyCode::Enter));
            a.now += 1.0;
            a.on_key(code(KeyCode::Enter));
            assert!(screen(&a).contains("Question 2/2"));
            // leaving is not held back by the guard (a different key) and lifts it
            assert!(a.guard.is_some());
            assert_eq!(a.on_key(leave), Effect::None);
            assert!(matches!(a.mode, Mode::List));
            assert!(a.guard.is_none());
            let s = screen(&a);
            assert!(!s.contains("Question"));
            assert!(s.contains("Which db?"));
            a.on_key(code(KeyCode::Enter));
            let s = screen(&a);
            assert!(s.contains("Question 1/2"));
            assert!(!s.contains("✓"));
        }
    }

    #[test]
    fn long_question_does_not_hide_typed_answer() {
        let long = format!(
            "Which database should the service use for {}?",
            "x".repeat(50)
        );
        let item = claude(
            "q",
            "claude.question",
            "AskUserQuestion",
            Some(HOOK_REPLY),
            json!({"questions": [{"question": long, "multiSelect": false, "options": [
                {"label": "Postgres"}, {"label": "SQLite"}]}]}),
        );
        let mut a = app(vec![item]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        a.on_key(key('j'));
        a.on_key(key('j'));
        a.on_key(code(KeyCode::Enter));
        type_text(&mut a, "my own answer");
        let s = screen(&a);
        assert!(s.contains("Other: my own answer"));
        assert!(s.contains("Question 1/1"));
        assert!(s.contains("Which database"));
    }

    #[test]
    fn cursor_stays_visible_in_a_long_option_list() {
        let options: Vec<Value> = (0..40)
            .map(|i| json!({"label": format!("opt-{i}"), "description": "d"}))
            .collect();
        let item = claude(
            "q",
            "claude.question",
            "AskUserQuestion",
            Some(HOOK_REPLY),
            json!({"questions": [{"question": "Pick", "options": options}]}),
        );
        let mut a = app(vec![item]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        for _ in 0..30 {
            a.on_key(key('j'));
        }
        assert!(screen(&a).contains(">   opt-30"));
    }

    #[test]
    fn answer_mode_survives_item_disappearing() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        a.set_items(vec![]);
        for c in "dqDnyjkhl ".chars() {
            assert_eq!(a.on_key(key(c)), Effect::None);
        }
        assert!(screen(&a).contains("item is gone"));
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        // Esc leaves the dead answer view for the list
        a.on_key(code(KeyCode::Esc));
        assert!(matches!(a.mode, Mode::List));
        assert_eq!(a.on_key(key('q')), Effect::Quit);
    }

    // --- session view

    fn selected_id(a: &App) -> Option<&str> {
        a.selected_item().map(|i| i.id.as_str())
    }

    #[test]
    fn s_shows_only_the_session_of_the_selected_item() {
        let mut a = app(vec![
            sudo("s"),
            in_session(question("other", 950.0), "s-other"),
            in_session(question("mine1", 960.0), "s-mine-0123456789"),
            in_session(question("mine2", 970.0), "s-mine-0123456789"),
        ]);
        assert!(screen(&a).contains("All 4 · Sessions 0"));
        a.on_key(key('s'));
        let s = screen(&a);
        assert!(s.contains("Session infra · s-mine-0 "));
        assert!(s.contains("title-mine1") && s.contains("title-mine2"));
        assert!(!s.contains("title-other") && !s.contains("sudo ls"));
        assert!(s.contains("esc All"));
        assert!(a.hints().contains(&("esc", "All")), "{:?}", a.hints());
        // the header keeps counting everything
        assert!(s.contains("4 pending"));
        // navigation stays within the session
        a.on_key(key('j'));
        a.on_key(key('j'));
        assert_eq!(selected_id(&a), Some("mine1"));
    }

    #[test]
    fn s_needs_a_claude_session() {
        let mut a = app(vec![sudo("s"), question("q", 960.0)]);
        a.on_key(key('s'));
        assert!(a.session.is_none());
        assert!(screen(&a).contains("no Claude session on this item"));
        a.on_key(key('j'));
        a.on_key(key('s'));
        assert!(a.session.is_none());
        // only items with a session offer it
        a.now += 10.0;
        assert!(!screen(&a).contains("s session"));
        let a = app(vec![in_session(question("q", 960.0), "s-1")]);
        assert!(screen(&a).contains("s session"));
    }

    #[test]
    fn s_and_esc_leave_the_session_with_the_same_item_selected() {
        let mut a = app(vec![
            in_session(question("other", 950.0), "s-other"),
            in_session(question("mine1", 960.0), "s-mine"),
            in_session(question("mine2", 970.0), "s-mine"),
            in_session(question("top", 980.0), "s-other"),
        ]);
        a.on_key(key('j'));
        a.on_key(key('j'));
        assert_eq!(selected_id(&a), Some("mine1"));
        a.on_key(key('s'));
        assert_eq!(a.session.as_deref(), Some("s-mine"));
        assert_eq!(selected_id(&a), Some("mine1"));
        a.on_key(key('s'));
        assert!(a.session.is_none());
        assert_eq!(selected_id(&a), Some("mine1"));
        a.on_key(key('s'));
        a.on_key(code(KeyCode::Esc));
        assert!(a.session.is_none());
        assert_eq!(selected_id(&a), Some("mine1"));
        // Esc in the full list does nothing
        assert_eq!(a.on_key(code(KeyCode::Esc)), Effect::None);
        assert_eq!(selected_id(&a), Some("mine1"));
    }

    #[test]
    fn answering_a_permission_opens_the_next_item_of_the_session() {
        let mut a = app(vec![
            in_session(permission("p2", Some(HOOK_REPLY)), "s-x"),
            in_session(permission("p1", Some(HOOK_REPLY)), "s-x"),
            in_session(permission("o", Some(HOOK_REPLY)), "s-y"),
        ]);
        a.on_key(key('j'));
        a.on_key(key('s'));
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('y')), allow("p1"));
        assert!(matches!(&a.mode, Mode::Detail { id, .. } if id == "p2"));
        // the router has dismissed p1; a held key does not decide p2
        a.set_items(vec![
            in_session(permission("p2", Some(HOOK_REPLY)), "s-x"),
            in_session(permission("o", Some(HOOK_REPLY)), "s-y"),
        ]);
        assert_eq!(selected_id(&a), Some("p2"));
        for _ in 0..3 {
            a.now += 0.6;
            assert_eq!(a.on_key(key('y')), Effect::None);
        }
        assert!(matches!(&a.mode, Mode::Detail { id, .. } if id == "p2"));
        a.now += 1.1;
        assert_eq!(a.on_key(key('y')), allow("p2"));
        // the last item of the session: back to the list, and the refresh ends the session
        assert!(matches!(a.mode, Mode::List));
        a.set_items(vec![in_session(permission("o", Some(HOOK_REPLY)), "s-y")]);
        assert!(a.session.is_none());
        assert!(matches!(a.mode, Mode::List));
        assert_eq!(selected_id(&a), Some("o"));
    }

    #[test]
    fn answering_a_question_opens_the_next_item_of_the_session() {
        let mut a = app(vec![
            in_session(ask("q2", Some(HOOK_REPLY)), "s-x"),
            in_session(ask("q1", Some(HOOK_REPLY)), "s-x"),
        ]);
        a.on_key(key('s'));
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        a.on_key(key(' '));
        let Effect::Send(sent, _) = a.on_key(code(KeyCode::Enter)) else {
            panic!("expected a send after the last question");
        };
        assert_eq!(sent, "q1");
        // the next question opens straight in the answer choice, guarded against a held Enter
        assert!(matches!(&a.mode, Mode::Answer { id, .. } if id == "q2"));
        a.now += 0.6;
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        assert!(matches!(&a.mode, Mode::Answer { id, st } if id == "q2" && st.done.is_empty()));
        a.now += 1.1;
        a.on_key(code(KeyCode::Enter));
        assert!(matches!(&a.mode, Mode::Answer { id, st } if id == "q2" && st.idx == 1));
    }

    #[test]
    fn number_keys_pick_instantly_in_single_select() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        assert_eq!(a.on_key(key('2')), Effect::None);
        a.now += 1.0;
        let s = screen(&a);
        assert!(s.contains("✓ Which db? → SQLite"));
        assert!(s.contains("Question 2/2"));
        // multi-select: toggle, never send
        a.on_key(key('1'));
        a.on_key(key('3'));
        let s = screen(&a);
        assert!(s.contains("[x] Auth") && s.contains("[ ] Cache") && s.contains("[x] Logs"));
        let Effect::Send(_, line) = a.on_key(key('l')) else {
            panic!("expected a send after the last question");
        };
        let sent: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            sent,
            json!({"behavior": "allow",
                "answers": {"Which db?": "SQLite", "Which features?": "Auth, Logs"}})
        );
        assert!(matches!(a.mode, Mode::List));
    }

    #[test]
    fn number_keys_beyond_the_options_do_nothing() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        for c in ['3', '9'] {
            assert_eq!(a.on_key(key(c)), Effect::None);
        }
        assert!(matches!(&a.mode, Mode::Answer { st, .. } if st.idx == 0 && st.cursor == 0));
        // Other… has no number; on to the multi-select, which has three options
        a.on_key(key('1'));
        a.now += 1.0;
        for c in ['4', '5'] {
            a.on_key(key(c));
        }
        assert!(matches!(&a.mode, Mode::Answer { st, .. } if st.idx == 1 && st.picked.is_empty()));
    }

    #[test]
    fn g_and_capital_g_jump_to_the_first_and_last_row() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        a.on_key(key('G'));
        assert!(screen(&a).contains(">   Other…"));
        a.on_key(key('g'));
        assert!(screen(&a).contains("> 1 Postgres"));
    }

    #[test]
    fn l_selects_like_enter() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        a.on_key(key('j'));
        a.on_key(key('l'));
        a.now += 1.0;
        assert!(screen(&a).contains("✓ Which db? → SQLite"));
        // multi-select without a choice: like Enter
        a.on_key(key('l'));
        assert!(screen(&a).contains("select at least one"));
        // on Other… it opens the text input, where `l` is text
        a.on_key(key('G'));
        a.on_key(key('l'));
        type_text(&mut a, "hl");
        assert!(screen(&a).contains("Other: hl"));
    }

    #[test]
    fn h_in_answer_mode_returns_to_the_list() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        assert_eq!(a.on_key(key('h')), Effect::None);
        assert!(matches!(a.mode, Mode::List));
    }

    #[test]
    fn answer_mode_denies_dismisses_and_shows_raw() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('?')), Effect::None);
        let s = screen(&a);
        assert!(s.contains("\"project\": \"proj\"") && s.contains(" Raw "));
        a.on_key(key('?'));
        assert!(screen(&a).contains("Question 1/2"));
        assert_eq!(a.on_key(key('n')), deny("q"));
        assert!(matches!(a.mode, Mode::List));
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('d')), Effect::Dismiss("q".into()));
        assert!(matches!(a.mode, Mode::List));
    }

    #[test]
    fn the_raw_view_in_answer_mode_picks_nothing() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('?'));
        assert!(a.raw);
        for c in [code(KeyCode::Enter), key('1'), key('j'), key('l')] {
            assert_eq!(a.on_key(c), Effect::None);
        }
        assert!(matches!(&a.mode, Mode::Answer { st, .. } if st.done.is_empty() && st.cursor == 0));
        // back to the choice: Enter picks
        a.on_key(key('?'));
        assert!(!a.raw);
        a.on_key(code(KeyCode::Enter));
        assert!(screen(&a).contains("✓ Which db? → Postgres"));
        // the raw view is a mode of the list too, but a question opens without it
        a.on_key(code(KeyCode::Esc));
        a.on_key(key('?'));
        assert!(a.raw);
        a.on_key(code(KeyCode::Enter));
        assert!(!a.raw);
    }

    #[test]
    fn raw_view_in_answer_mode_still_denies_dismisses_and_leaves() {
        for (c, effect) in [
            (key('n'), deny("q")),
            (key('d'), Effect::Dismiss("q".into())),
            (key('h'), Effect::None),
            (code(KeyCode::Esc), Effect::None),
        ] {
            let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
            a.on_key(code(KeyCode::Enter));
            a.on_key(key('?'));
            assert_eq!(a.on_key(c), effect);
            assert!(matches!(a.mode, Mode::List));
        }
    }

    #[test]
    fn answer_mode_keys_are_text_in_the_other_input() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        a.on_key(key('G'));
        a.on_key(code(KeyCode::Enter));
        type_text(&mut a, "gGhn1d?a");
        assert!(screen(&a).contains("Other: gGhn1d?a"));
    }

    #[test]
    fn a_held_key_keeps_the_guard_up() {
        let mut a = app(vec![
            in_session(permission("p3", Some(HOOK_REPLY)), "s-x"),
            in_session(permission("p2", Some(HOOK_REPLY)), "s-x"),
            in_session(permission("p1", Some(HOOK_REPLY)), "s-x"),
        ]);
        a.on_key(key('s'));
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('y')), allow("p1"));
        // the first repeat comes after the initial key-repeat delay, the rest fast
        a.now += 0.6;
        assert_eq!(a.on_key(key('y')), Effect::None);
        for _ in 0..50 {
            a.now += 0.03;
            assert_eq!(a.on_key(key('y')), Effect::None);
        }
        assert!(matches!(&a.mode, Mode::Detail { id, .. } if id == "p2"));
        // released: the next press decides
        a.now += 1.1;
        assert_eq!(a.on_key(key('y')), allow("p2"));
    }

    #[test]
    fn a_held_deny_or_dismiss_decides_one_item() {
        for (c, effect) in [('n', deny("p1")), ('d', Effect::Dismiss("p1".into()))] {
            let mut a = app(vec![
                in_session(permission("p3", Some(HOOK_REPLY)), "s-x"),
                in_session(permission("p2", Some(HOOK_REPLY)), "s-x"),
                in_session(permission("p1", Some(HOOK_REPLY)), "s-x"),
            ]);
            a.on_key(key('s'));
            a.on_key(code(KeyCode::Enter));
            assert_eq!(a.on_key(key(c)), effect);
            for _ in 0..3 {
                a.now += 0.6;
                assert_eq!(a.on_key(key(c)), Effect::None);
            }
            assert!(matches!(&a.mode, Mode::Detail { id, .. } if id == "p2"));
        }
    }

    #[test]
    fn the_raw_view_works_during_the_guard() {
        let mut a = app(vec![
            in_session(permission("p2", Some(HOOK_REPLY)), "s-x"),
            in_session(permission("p1", Some(HOOK_REPLY)), "s-x"),
        ]);
        a.on_key(key('s'));
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('y'));
        assert!(!a.raw);
        assert_eq!(a.on_key(key('?')), Effect::None);
        assert!(a.raw);
    }

    #[test]
    fn the_session_stays_under_the_dismiss_all_prompt() {
        let capital_d = KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT);
        let mut a = app(vec![
            in_session(question("o", 950.0), "s-y"),
            in_session(question("a", 960.0), "s-x"),
        ]);
        a.on_key(key('s'));
        a.on_key(capital_d);
        // the session's items are gone while the prompt is open
        a.set_items(vec![in_session(question("o", 950.0), "s-y")]);
        assert_eq!(a.session.as_deref(), Some("s-x"));
        assert!(a.without_reply().is_empty());
        // the next refresh after the prompt ends the session
        a.on_key(key('n'));
        a.set_items(vec![in_session(question("o", 950.0), "s-y")]);
        assert!(a.session.is_none());
    }

    #[test]
    fn dismissing_from_the_session_list_stays_in_the_list() {
        let mut a = app(vec![
            in_session(question("a", 960.0), "s-x"),
            in_session(question("b", 970.0), "s-x"),
        ]);
        a.on_key(key('s'));
        assert_eq!(a.on_key(key('d')), Effect::Dismiss("b".into()));
        assert!(matches!(a.mode, Mode::List));
        // from the detail pane it opens the next item
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('d')), Effect::Dismiss("b".into()));
        assert!(matches!(&a.mode, Mode::Detail { id, .. } if id == "a"));
    }

    #[test]
    fn dismiss_all_is_scoped_to_the_session() {
        let capital_d = KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT);
        let mut a = app(vec![
            in_session(question("c", 950.0), "s-y"),
            in_session(question("a", 960.0), "s-x"),
            in_session(question("b", 970.0), "s-x"),
            in_session(permission("p", Some(HOOK_REPLY)), "s-x"),
            sudo("s"),
        ]);
        a.on_key(key('j'));
        a.on_key(key('s'));
        a.on_key(capital_d);
        assert!(screen(&a).contains("Dismiss 2 items without a reply?"));
        assert_eq!(a.on_key(key('y')), Effect::DismissAll);
        let mut ids = a.without_reply();
        ids.sort();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    // --- sessions view

    fn session(pid: u32, id: &str, status: &str) -> Session {
        Session {
            pid,
            session_id: id.into(),
            cwd: "/home/u/projects/r.agents/w1".into(),
            name: String::new(),
            status: status.into(),
            waiting_for: if status == WAITING {
                "permission prompt".into()
            } else {
                String::new()
            },
            kind: "interactive".into(),
            started_at: 900.0,
            status_at: 990.0,
            start: None,
            shpool: String::new(),
            origin_file: ORIGIN_FILE.into(),
        }
    }

    fn shpooled(mut s: Session, name: &str) -> Session {
        s.shpool = name.into();
        s
    }

    fn current_tab(a: &App) -> &str {
        a.tab_name(a.tab)
    }

    /// Presses `f` until the Sessions tab is reached.
    fn to_sessions_tab(a: &mut App) {
        for _ in 0..8 {
            if current_tab(a) == "Sessions" {
                return;
            }
            assert_eq!(a.on_key(key('f')), Effect::None);
        }
        panic!("no Sessions tab");
    }

    /// The app on the Sessions tab.
    fn sessions_app(items: Vec<Item>, sessions: Vec<Session>) -> App {
        let mut a = app(items);
        a.set_sessions(sessions);
        to_sessions_tab(&mut a);
        a
    }

    fn selected_pid(a: &App) -> Option<u32> {
        a.selected_session().map(|s| s.pid)
    }

    #[test]
    fn capital_s_does_nothing_and_is_not_offered() {
        let mut a = app(vec![question("a", 990.0)]);
        a.set_sessions(vec![session(1, "s-a", "idle")]);
        let s = screen(&a);
        assert!(!s.contains("S sessions") && !s.contains("S inbox"), "{s}");
        assert_eq!(a.on_key(key('S')), Effect::None);
        assert_eq!((a.tab, current_tab(&a)), (0, "All"));
        assert!(matches!(a.mode, Mode::List));
        assert!(a.hints().iter().all(|(k, _)| *k != "S"), "{:?}", a.hints());
        a.on_key(key('f'));
        assert_eq!(current_tab(&a), "Sessions");
        assert_eq!(a.on_key(key('S')), Effect::None);
        assert_eq!(current_tab(&a), "Sessions");
        assert!(matches!(a.mode, Mode::List));
        assert!(a.hints().iter().all(|(k, _)| *k != "S"), "{:?}", a.hints());
        // the inbox keys do nothing on the sessions tab, q quits from every tab
        assert_eq!(a.on_key(key('d')), Effect::None);
        assert_eq!(a.on_key(key('q')), Effect::Quit);
        a.on_key(key('f'));
        assert_eq!(a.on_key(key('q')), Effect::Quit);
    }

    #[test]
    fn sessions_rows_and_detail() {
        let mut a = sessions_app(
            vec![
                in_session(question("a", 950.0), "s-a"),
                in_session(question("b", 960.0), "s-a"),
                in_session(question("c", 970.0), "s-b"),
            ],
            vec![
                shpooled(session(1, "s-a", WAITING), "jw-1"),
                session(2, "s-b", "idle"),
                session(3, "s-c", "busy"),
                session(4, "s-d", "starting"),
            ],
        );
        let s = screen(&a);
        let row = |s: &str, badge: &str| {
            s.lines()
                .find(|l| l.contains(badge))
                .unwrap_or_else(|| panic!("no {badge} row:\n{s}"))
                .to_string()
        };
        let wait = row(&s, "WAIT");
        assert!(wait.contains("2 WAIT") && wait.contains("r:w1"), "{wait}");
        assert!(
            wait.contains("permission prompt") && wait.contains("10s"),
            "{wait}"
        );
        let idle = row(&s, "IDLE");
        assert!(
            idle.contains("1 IDLE") && !idle.contains("permission"),
            "{idle}"
        );
        // no pending items: no count
        assert!(row(&s, "BUSY").contains("│   BUSY"), "{s}");
        // an unknown status is shown as text
        assert!(row(&s, "star…").contains("│   star…"), "{s}");
        // the header counts sessions on every tab
        assert!(
            s.contains("3 pending · 0 answerable · 4 sessions (1 waiting)"),
            "{s}"
        );
        // detail of the selected (first) session
        for want in [
            "WAIT  permission prompt  ·  10s",
            "cwd: /home/u/projects/r.agents/w1",
            "shpool: jw-1",
            "pid: 1",
            "started: 1m ago",
            "kind: interactive",
            "session: s-a",
            "title-a",
            "title-b",
        ] {
            assert!(s.contains(want), "{want}:\n{s}");
        }
        assert!(!s.contains("title-c"));
        a.on_key(key('j'));
        let s = screen(&a);
        assert!(
            s.contains("none: not attachable") && s.contains("title-c"),
            "{s}"
        );
        assert!(!s.contains("title-a"));
        // the header counts sessions on every tab
        a.on_key(key('f'));
        assert_eq!(current_tab(&a), "All");
        assert!(screen(&a).contains("4 sessions (1 waiting)"));
    }

    #[test]
    fn no_sessions_is_an_empty_state() {
        let a = sessions_app(vec![], vec![]);
        let s = screen(&a);
        assert!(s.contains("No Claude sessions"), "{s}");
        assert!(s.contains("0 sessions") && !s.contains("waiting"), "{s}");
    }

    #[test]
    fn s_on_an_item_returns_to_the_tab_it_was_opened_on() {
        let mut a = app(vec![
            in_session(question("a", 960.0), "s-a"),
            in_session(question("b", 970.0), "s-b"),
        ]);
        a.set_sessions(vec![session(1, "s-a", "idle")]);
        a.on_key(key('s'));
        assert_eq!(a.session.as_deref(), Some("s-b"));
        assert!(a.hints().contains(&("esc", "All")), "{:?}", a.hints());
        assert!(screen(&a).contains("esc All"));
        // `f` is ignored inside the drill-down
        assert_eq!(a.on_key(key('f')), Effect::None);
        assert_eq!(current_tab(&a), "All");
        assert_eq!(a.session.as_deref(), Some("s-b"));
        a.on_key(code(KeyCode::Esc));
        assert!(a.session.is_none());
        assert_eq!(current_tab(&a), "All");
        let s = screen(&a);
        assert!(s.contains("title-a") && s.contains("title-b"), "{s}");
    }

    #[test]
    fn a_drill_down_from_an_item_tab_ends_on_that_tab() {
        let mut a = tabbed_app();
        let r = || {
            in_session(
                from_file(permission("p", Some(HOOK_REPLY)), REVIEW_FILE),
                "s-r",
            )
        };
        a.set_items(vec![r()]);
        a.on_key(key('f'));
        assert_eq!(current_tab(&a), "Review");
        a.on_key(key('s'));
        assert_eq!(a.session.as_deref(), Some("s-r"));
        assert!(a.hints().contains(&("esc", "Review")), "{:?}", a.hints());
        a.now += 1.1;
        a.on_key(code(KeyCode::Enter));
        assert!(matches!(a.mode, Mode::Detail { .. }));
        a.set_items(vec![]);
        assert_eq!(a.session.as_deref(), Some("s-r"));
        assert!(matches!(a.mode, Mode::Detail { .. }));
        assert!(screen(&a).contains("item is gone"));
        a.on_key(code(KeyCode::Esc));
        a.set_items(vec![]);
        assert!(a.session.is_none());
        assert_eq!(current_tab(&a), "Review");
    }

    #[test]
    fn a_attaches_the_shpool_session_or_says_why_not() {
        let mut a = sessions_app(
            vec![],
            vec![
                shpooled(session(1, "s-a", "idle"), "jw-1"),
                session(2, "s-b", "idle"),
            ],
        );
        assert_eq!(a.on_key(key('a')), Effect::Attach("jw-1".into()));
        assert!(screen(&a).contains("a attach"));
        a.on_key(key('j'));
        assert_eq!(a.on_key(key('a')), Effect::None);
        let s = screen(&a);
        assert!(s.contains("not in a shpool session"), "{s}");
        // no sessions: nothing to attach, nothing to open
        let mut a = sessions_app(vec![], vec![]);
        assert_eq!(a.on_key(key('a')), Effect::None);
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
    }

    #[test]
    fn session_selection_follows_its_pid_across_refreshes() {
        let mut a = sessions_app(
            vec![],
            vec![
                session(1, "s-1", "idle"),
                session(2, "s-2", "idle"),
                session(3, "s-3", "idle"),
            ],
        );
        a.on_key(key('j'));
        a.on_key(code(KeyCode::Down));
        a.on_key(code(KeyCode::Down));
        assert_eq!(selected_pid(&a), Some(3));
        a.on_key(key('k'));
        assert_eq!(selected_pid(&a), Some(2));
        a.set_sessions(vec![
            session(4, "s-4", "idle"),
            session(1, "s-1", "idle"),
            session(2, "s-2", "idle"),
            session(3, "s-3", "idle"),
        ]);
        assert_eq!(selected_pid(&a), Some(2));
        // gone: the position stays (within the list)
        a.set_sessions(vec![session(4, "s-4", "idle"), session(1, "s-1", "idle")]);
        assert_eq!(selected_pid(&a), Some(1));
        a.set_sessions(vec![]);
        assert_eq!(selected_pid(&a), None);
    }

    #[test]
    fn sessions_load_while_the_router_is_offline() {
        // a process with an origin, without a start time: alive (/proc is the real one)
        let mut child = Command::new("sleep")
            .arg("30")
            .env("CLAUDE_ORIGIN_FILE", ORIGIN_FILE)
            .spawn()
            .unwrap();
        let pid = child.id();
        let dir = std::env::temp_dir().join(format!("agent-inbox-test-tui-{pid}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{pid}.json")),
            format!(r#"{{"pid":{pid},"sessionId":"s-1","cwd":"/x","startedAt":1000}}"#),
        )
        .unwrap();
        let mut a = app(vec![]);
        refresh(&mut a, "127.0.0.1:1", None, &dir);
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(a.offline.is_some());
        assert_eq!(a.sessions.len(), 1);
        assert_eq!(a.sessions[0].origin_file, ORIGIN_FILE);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // --- kill

    fn started(mut s: Session, start: u64) -> Session {
        s.start = Some(start);
        s
    }

    #[test]
    fn x_kills_the_selected_session_at_once() {
        let sessions = vec![
            started(session(7, "s-a", WAITING), 42),
            started(session(8, "s-b", "idle"), 43),
        ];
        let mut a = sessions_app(vec![], sessions);
        assert!(screen(&a).contains("x kill"));
        assert_eq!(a.on_key(key('x')), Effect::Kill { pid: 7, start: 42 });
        assert!(matches!(a.mode, Mode::List));
        // the second session
        a.on_key(key('j'));
        assert_eq!(a.on_key(key('x')), Effect::Kill { pid: 8, start: 43 });
    }

    #[test]
    fn a_session_without_a_start_time_is_refused_at_once() {
        let mut a = sessions_app(vec![], vec![session(7, "s-a", "idle")]);
        assert!(!screen(&a).contains("x kill"));
        assert_eq!(a.on_key(key('x')), Effect::None);
        assert!(matches!(a.mode, Mode::List));
        let s = screen(&a);
        assert!(
            s.contains("pid 7 not killed: the registry has no start time"),
            "{s}"
        );
        assert!(!s.contains("Kill claude"), "{s}");
    }

    #[test]
    fn x_needs_a_selected_session_on_the_sessions_tab() {
        let mut a = sessions_app(vec![], vec![]);
        assert!(!screen(&a).contains("x kill"));
        assert_eq!(a.on_key(key('x')), Effect::None);
        assert!(matches!(a.mode, Mode::List));
        // the All tab ignores it
        let mut a = app(vec![question("a", 990.0)]);
        a.set_sessions(vec![session(7, "s-a", "idle")]);
        assert_eq!(a.on_key(key('x')), Effect::None);
        assert!(matches!(a.mode, Mode::List));
        assert!(!screen(&a).contains("Kill"));
    }

    #[test]
    fn killing_dismisses_only_the_items_of_that_session() {
        let items = vec![
            in_session(question("a", 990.0), "s-a"),
            in_session(question("b", 990.0), "s-b"),
            in_session(question("c", 990.0), "s-a"),
            question("d", 990.0),
        ];
        let sessions = vec![
            session(7, "s-a", "idle"),
            session(8, "s-b", "idle"),
            session(9, "", "idle"),
        ];
        let mut a = sessions_app(items, sessions);
        let mut ids = a.session_item_ids(7);
        ids.sort();
        assert_eq!(ids, ["a", "c"]);
        assert_eq!(a.session_item_ids(8), ["b"]);
        // no session id: the items without one are not its
        assert!(a.session_item_ids(9).is_empty());
        assert!(a.session_item_ids(10).is_empty());
        // the items are stale while the router is unreachable
        a.offline = Some("down".into());
        assert!(a.session_item_ids(7).is_empty());
    }

    /// Start time of process `pid`: field 22 of `/proc/<pid>/stat`.
    fn start_of(pid: u32) -> u64 {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let rest = stat.rsplit_once(')').unwrap().1;
        rest.split_whitespace().nth(19).unwrap().parse().unwrap()
    }

    /// A `sleep 30` that does not outlive the test, whatever the assertions do.
    struct Sleeper(std::process::Child);

    impl Sleeper {
        fn spawn() -> Self {
            Self(Command::new("sleep").arg("30").spawn().unwrap())
        }
    }

    impl Drop for Sleeper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn terminate_sends_sigterm_to_the_process_with_that_start_time() {
        use std::os::unix::process::ExitStatusExt;
        let proc = Path::new("/proc");
        let mut sleeper = Sleeper::spawn();
        let pid = sleeper.0.id();
        assert_eq!(terminate(proc, pid, Some(start_of(pid))), Ok(()));
        assert_eq!(sleeper.0.wait().unwrap().signal(), Some(libc::SIGTERM));
        // gone now
        let err = terminate(proc, pid, Some(1)).unwrap_err();
        assert!(err.contains("already gone"), "{err}");
    }

    #[test]
    fn terminate_leaves_a_process_with_another_start_time_or_none() {
        let proc = Path::new("/proc");
        let mut sleeper = Sleeper::spawn();
        let pid = sleeper.0.id();
        let start = start_of(pid);
        let err = terminate(proc, pid, Some(start + 1)).unwrap_err();
        assert!(err.contains("already gone"), "{err}");
        let err = terminate(proc, pid, None).unwrap_err();
        assert!(err.contains("no start time"), "{err}");
        assert_eq!(sleeper.0.try_wait().unwrap(), None);
    }

    #[test]
    fn terminate_refuses_pids_that_are_not_a_single_process() {
        let proc = Path::new("/proc");
        for pid in [0, u32::MAX, i32::MAX as u32 + 1] {
            let err = terminate(proc, pid, Some(1)).unwrap_err();
            assert!(err.contains("invalid pid"), "{pid}: {err}");
        }
    }

    // --- new agent

    #[test]
    fn capital_n_asks_for_a_task_on_every_tab() {
        let mut a = sessions_app(vec![], vec![session(1, "s-a", "idle")]);
        assert!(screen(&a).contains("N new"));
        assert_eq!(a.on_key(key('N')), Effect::None);
        assert!(matches!(&a.mode, Mode::Task { buf } if buf.is_empty()));
        // without sessions too
        let mut a = sessions_app(vec![], vec![]);
        assert!(screen(&a).contains("N new"));
        assert_eq!(a.on_key(key('N')), Effect::None);
        assert!(matches!(a.mode, Mode::Task { .. }));
        // the item tabs, with and without items
        let mut a = tabbed_sessions_app();
        a.on_key(key('f'));
        assert!(screen(&a).contains("N new"));
        assert_eq!(a.on_key(key('N')), Effect::None);
        assert!(matches!(a.mode, Mode::Task { .. }));
        let mut a = app(vec![question("a", 990.0)]);
        assert!(screen(&a).contains("N new"));
        assert_eq!(a.on_key(key('N')), Effect::None);
        assert!(matches!(a.mode, Mode::Task { .. }));
        let mut a = app(vec![]);
        assert!(screen(&a).contains("N new"));
        assert_eq!(a.on_key(key('N')), Effect::None);
        assert!(matches!(a.mode, Mode::Task { .. }));
    }

    #[test]
    fn enter_starts_the_agent_with_the_edited_task() {
        let mut a = app(vec![question("a", 990.0)]);
        a.on_key(key('N'));
        type_text(&mut a, "fix x");
        assert_eq!(a.on_key(code(KeyCode::Backspace)), Effect::None);
        type_text(&mut a, "y");
        assert_eq!(
            a.on_key(code(KeyCode::Enter)),
            Effect::NewAgent("fix y".into())
        );
        assert!(matches!(a.mode, Mode::List));
    }

    #[test]
    fn an_empty_task_means_pick_a_repo() {
        for text in ["", "  "] {
            let mut a = app(vec![]);
            a.on_key(key('N'));
            type_text(&mut a, text);
            assert_eq!(
                a.on_key(code(KeyCode::Enter)),
                Effect::NewAgent(String::new()),
                "{text:?}"
            );
            assert!(matches!(a.mode, Mode::List));
        }
    }

    #[test]
    fn a_task_of_only_a_line_break_means_pick_a_repo() {
        let mut a = app(vec![]);
        a.on_key(key('N'));
        a.on_key(ctrl_j());
        assert_eq!(
            a.on_key(code(KeyCode::Enter)),
            Effect::NewAgent(String::new())
        );
        assert!(matches!(a.mode, Mode::List));
    }

    #[test]
    fn the_popup_keeps_the_empty_screen_under_it() {
        let mut a = app(vec![]);
        a.on_key(key('N'));
        let s = screen(&a);
        assert!(s.contains(" New agent "), "{s}");
        for shown in ["All 0 · Sessions 0", " Detail ", "No pending requests"] {
            assert!(s.contains(shown), "{shown:?}:\n{s}");
        }
    }

    #[test]
    fn esc_cancels_the_task_and_keeps_the_tab() {
        let mut a = sessions_app(vec![], vec![session(1, "s-a", "idle")]);
        a.on_key(key('N'));
        type_text(&mut a, "fix");
        assert_eq!(a.on_key(code(KeyCode::Esc)), Effect::None);
        assert!(matches!(a.mode, Mode::List));
        assert_eq!(current_tab(&a), "Sessions");
        let mut a = app(vec![question("a", 990.0)]);
        a.on_key(key('N'));
        assert_eq!(a.on_key(code(KeyCode::Esc)), Effect::None);
        assert!(matches!(a.mode, Mode::List));
        assert_eq!(current_tab(&a), "All");
    }

    #[test]
    fn command_keys_are_text_in_the_task() {
        let mut a = app(vec![permission("a", Some("r"))]);
        a.on_key(key('N'));
        type_text(&mut a, "qydx");
        assert!(matches!(&a.mode, Mode::Task { buf } if buf == "qydx"));
        assert_eq!(a.items.len(), 1);
    }

    fn ctrl_j() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)
    }

    #[test]
    fn the_task_is_shown_in_a_popup_over_the_status() {
        let mut a = app(vec![question("a", 990.0)]);
        a.notify("some status", false);
        a.on_key(key('N'));
        let s = screen(&a);
        assert!(s.contains("What should the agent do?"), "{s}");
        type_text(&mut a, "fix");
        let s = screen(&a);
        assert!(s.contains(" New agent "), "{s}");
        assert!(s.contains("fix"), "{s}");
        assert!(!s.contains("What should the agent do?"), "{s}");
        assert!(!s.contains("some status"), "{s}");
        assert!(s.contains("enter start  ctrl-j newline  esc cancel"), "{s}");
    }

    #[test]
    fn a_long_task_shows_its_end() {
        // wrapped: seven rows in a popup of six
        let mut a = app(vec![]);
        a.on_key(key('N'));
        type_text(&mut a, &format!("{}tail-end", "x".repeat(600)));
        let s = screen_at(&a, 100, 10);
        assert!(s.contains("xtail-end"), "{s}");
        // lines
        let mut a = app(vec![]);
        a.on_key(key('N'));
        for i in 0..40 {
            type_text(&mut a, &format!("line{i}"));
            a.on_key(ctrl_j());
        }
        let s = screen(&a);
        assert!(s.contains("line39"), "{s}");
        assert!(!s.contains("line0"), "{s}");
    }

    #[test]
    fn a_task_of_several_lines_is_drawn_on_separate_rows() {
        let mut a = app(vec![]);
        a.on_key(key('N'));
        type_text(&mut a, "first");
        a.on_key(ctrl_j());
        a.on_key(ctrl_j());
        type_text(&mut a, "second");
        let s = screen(&a);
        let row = |t: &str| s.lines().position(|l| l.contains(t)).unwrap();
        assert_eq!(row("second"), row("first") + 2, "{s}");
    }

    #[test]
    fn ctrl_j_and_alt_enter_add_a_line_to_the_task() {
        for newline in [ctrl_j(), KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)] {
            let mut a = app(vec![]);
            a.on_key(key('N'));
            type_text(&mut a, "a");
            assert_eq!(a.on_key(newline), Effect::None);
            assert!(matches!(a.mode, Mode::Task { .. }), "{newline:?}");
            type_text(&mut a, "b");
            assert_eq!(
                a.on_key(code(KeyCode::Enter)),
                Effect::NewAgent("a\nb".into()),
                "{newline:?}"
            );
        }
        // not elsewhere
        let mut a = app(vec![question("a", 990.0)]);
        assert_eq!(a.on_key(ctrl_j()), Effect::None);
        assert!(matches!(a.mode, Mode::List));
    }

    #[test]
    fn backspace_removes_a_line_break_of_the_task() {
        let mut a = app(vec![]);
        a.on_key(key('N'));
        type_text(&mut a, "a");
        a.on_key(ctrl_j());
        assert_eq!(a.on_key(code(KeyCode::Backspace)), Effect::None);
        assert!(matches!(&a.mode, Mode::Task { buf } if buf == "a"));
    }

    #[test]
    fn performing_a_new_agent_leaves_it_to_the_event_loop() {
        let mut a = app(vec![]);
        let effect = Effect::NewAgent("fix".into());
        assert!(!perform(
            &mut a,
            effect,
            "",
            None,
            Path::new("/nonexistent")
        ));
    }

    #[test]
    fn started_session_is_the_name_of_the_last_shpool_attach_line() {
        assert_eq!(started_session("shpool attach jw-1\n"), Some("jw-1".into()));
        // noise before and after, trailing whitespace and carriage return
        assert_eq!(
            started_session("jw: fetch failed, continuing\n  shpool attach jw-2 \t\r\nbye\n"),
            Some("jw-2".into())
        );
        // the last one wins
        assert_eq!(
            started_session("shpool attach jw-1\nshpool attach jw-3\nnoise\n"),
            Some("jw-3".into())
        );
        assert_eq!(
            started_session("no newline\nshpool attach jw-4"),
            Some("jw-4".into())
        );
        // not the line: other text around it, another command, no name
        for stderr in [
            "",
            "jw: no match\n",
            "run shpool attach jw-1\n",
            "shpool attach\n",
            "shpool attach  \n",
            "shpool list jw-1\n",
        ] {
            assert_eq!(started_session(stderr), None, "{stderr:?}");
        }
    }

    #[test]
    fn outcome_of_a_started_session() {
        let stderr = "jw: choosing a repo...\nshpool attach jw-1\n";
        assert_eq!(
            outcome("fix x", true, stderr),
            Outcome::Started("jw-1".into())
        );
        // without a task the user picked the repo: go to the session
        assert_eq!(
            outcome("", true, "shpool attach jw-2\n"),
            Outcome::Attach("jw-2".into())
        );
        // success must come with a name
        for task in ["", "fix x"] {
            assert!(
                matches!(
                    outcome(task, true, "jw: choosing a repo...\n"),
                    Outcome::Failed(_)
                ),
                "{task:?}"
            );
        }
        // a name of a failed run is no session
        assert_eq!(
            outcome("", false, "shpool attach jw-3\njw: boom\n"),
            Outcome::Failed("jw: boom".into())
        );
    }

    #[test]
    fn outcome_of_a_cancelled_or_failed_run() {
        // cancelling fzf: without a task jw says only that, with one the routing lines come first
        assert_eq!(outcome("", false, "jw: cancelled\n"), Outcome::Cancelled);
        assert_eq!(
            outcome(
                "fix x",
                false,
                "jw: choosing a repo...\njw: no repo fits clearly, pick one\njw: cancelled\n"
            ),
            Outcome::Cancelled
        );
        // a failure that says nothing is no cancellation
        assert_eq!(
            outcome("", false, ""),
            Outcome::Failed("jw agent -d failed".into())
        );
        // a real error, also after the routing lines
        assert_eq!(
            outcome("", false, "jw: no match\n"),
            Outcome::Failed("jw: no match".into())
        );
        assert_eq!(
            outcome(
                "fix x",
                false,
                "jw: choosing a repo...\njw: no jj repos under /r\n"
            ),
            Outcome::Failed("jw: no jj repos under /r".into())
        );
    }

    #[test]
    fn jw_args_put_the_task_after_a_double_dash() {
        assert_eq!(jw_args(""), ["agent", "-d"]);
        assert_eq!(jw_args("fix x"), ["agent", "-d", "--", "fix x"]);
        assert_eq!(jw_args("-x"), ["agent", "-d", "--", "-x"]);
    }

    #[test]
    fn ctrl_and_alt_characters_are_not_text_in_the_task() {
        for modifier in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            let mut a = app(vec![]);
            a.on_key(key('N'));
            type_text(&mut a, "ab");
            assert_eq!(
                a.on_key(KeyEvent::new(KeyCode::Char('y'), modifier)),
                Effect::None
            );
            assert!(
                matches!(&a.mode, Mode::Task { buf } if buf == "ab"),
                "{modifier:?}"
            );
        }
    }

    #[test]
    fn a_paste_keeps_the_lines_of_the_task_and_is_one_line_elsewhere() {
        let mut a = app(vec![permission("a", Some("r"))]);
        a.on_key(key('N'));
        type_text(&mut a, "x ");
        a.on_paste("a\nb\r\nc\rd");
        assert!(matches!(&a.mode, Mode::Task { buf } if buf == "x a\nb\nc\nd"));
        let mut a = app(vec![permission("a", Some("r"))]);
        a.on_paste("dD");
        assert!(matches!(a.mode, Mode::List));
        assert_eq!(a.items.len(), 1);
        // the password and the free text of "Other…" take it too
        let mut a = app(vec![sudo("s")]);
        a.on_key(code(KeyCode::Enter));
        a.on_key(code(KeyCode::Enter));
        type_text(&mut a, "pa");
        a.on_paste("ss\nword\r\n");
        assert!(matches!(&a.mode, Mode::Password { buf, .. } if buf == "password"));
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        a.on_key(key('G'));
        a.on_key(code(KeyCode::Enter));
        type_text(&mut a, "x ");
        a.on_paste("a\nb");
        assert!(matches!(&a.mode, Mode::Answer { st, .. } if st.input.as_deref() == Some("x a b")));
        // a choice that is not typing text ignores it
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.on_paste("dD");
        assert!(matches!(&a.mode, Mode::Answer { st, .. } if st.input.is_none()));
    }

    #[test]
    fn tail_counts_columns() {
        assert_eq!(tail("abc", 5), "abc");
        assert_eq!(tail("abcdef", 3), "def");
        // a wide char is two columns: 語abc is 5
        assert_eq!(tail("日本語abc", 5), "語abc");
        assert_eq!(tail("日本語abc", 4), "abc");
        assert_eq!(tail("日本", 3), "本");
        assert_eq!(tail("日本", 0), "");
    }

    // --- origin

    const MR_URL: &str = "https://gitlab/x/-/merge_requests/7";

    /// `ORIGIN_FILE` is a merge request !7 "T" with the link `url`.
    fn with_origin(mut a: App, url: &str) -> App {
        let origin = Origin {
            target: "MergeRequest".into(),
            iid: Some(7),
            title: "T".into(),
            url: url.into(),
            ..Origin::default()
        };
        a.origins.insert(ORIGIN_FILE.into(), Some(origin));
        a
    }

    fn without_origin_file(mut item: Item) -> Item {
        item.event.as_object_mut().unwrap().remove("origin_file");
        item
    }

    #[test]
    fn items_of_hand_started_sessions_are_hidden_but_sudo_stays() {
        let a = app(vec![
            without_origin_file(question("hand", 900.0)),
            question("agent", 950.0),
            without_origin_file(sudo("s")),
        ]);
        let ids: Vec<&str> = a.visible().iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["s", "agent"]);
        let s = screen(&a);
        assert!(
            s.contains("title-agent") && !s.contains("title-hand"),
            "{s}"
        );
        // nothing but hidden items: the empty state
        let a = app(vec![without_origin_file(question("hand", 900.0))]);
        assert!(screen(&a).contains("No pending requests"));
    }

    #[test]
    fn rows_show_the_origin_else_the_place() {
        let a = app(vec![question("a", 990.0)]);
        let s = screen_at(&a, 60, 30);
        let row = s.lines().find(|l| l.contains('○')).unwrap();
        assert!(row.contains("infra") && !row.contains("!7 T"), "{row}");
        let a = with_origin(a, MR_URL);
        let s = screen_at(&a, 60, 30);
        let row = s.lines().find(|l| l.contains('○')).unwrap();
        assert!(row.contains("!7 T") && !row.contains("infra"), "{row}");
        // sessions
        let a = sessions_app(vec![], vec![session(1, "s-a", "idle")]);
        let idle_row = |a: &App| {
            let s = screen(a);
            s.lines().find(|l| l.contains("IDLE")).unwrap().to_string()
        };
        assert!(idle_row(&a).contains("r:w1"), "{}", idle_row(&a));
        let a = with_origin(a, MR_URL);
        let row = idle_row(&a);
        assert!(row.contains("!7 T") && !row.contains("r:w1"), "{row}");
    }

    #[test]
    fn a_long_context_leaves_room_for_the_title_and_the_age() {
        let mut a = app(vec![question("a", 990.0)]);
        let origin = Origin {
            target: "MergeRequest".into(),
            iid: Some(7),
            title: "M".repeat(40),
            ..Origin::default()
        };
        a.origins.insert(ORIGIN_FILE.into(), Some(origin));
        let age = a.items[0].age(a.now);
        let s = screen_at(&a, 100, 30);
        let row = s.lines().find(|l| l.contains('○')).unwrap();
        assert!(row.contains("title-a") && row.contains(&age), "{row}");
    }

    #[test]
    fn o_opens_the_link_from_list_detail_answer_and_sessions() {
        let open = Effect::Open(MR_URL.into());
        let mut a = with_origin(app(vec![question("a", 990.0)]), MR_URL);
        assert!(a.hints().contains(&("o", "open")));
        assert_eq!(a.on_key(key('o')), open);
        assert!(matches!(a.mode, Mode::List));
        a.on_key(code(KeyCode::Enter));
        assert!(a.hints().contains(&("o", "open")));
        assert_eq!(a.on_key(key('o')), open);
        assert!(matches!(a.mode, Mode::Detail { .. }));
        // the choice stays open
        let mut a = with_origin(app(vec![ask("q", Some(HOOK_REPLY))]), MR_URL);
        a.on_key(code(KeyCode::Enter));
        assert!(a.hints().contains(&("o", "open")));
        assert_eq!(a.on_key(key('o')), open);
        assert!(matches!(a.mode, Mode::Answer { .. }));
        let a = sessions_app(vec![], vec![session(1, "s-a", "idle")]);
        let mut a = with_origin(a, MR_URL);
        assert!(a.hints().contains(&("o", "open")));
        assert_eq!(a.on_key(key('o')), open);
    }

    #[test]
    fn o_without_a_usable_link_is_an_error() {
        let cases = [
            (None, "no link for this item"),
            (Some(""), "no link for this item"),
            (Some("file:///x"), "no link for this item"),
            (Some("javascript:alert(1)"), "no link for this item"),
        ];
        for (url, text) in cases {
            let mut a = app(vec![question("a", 990.0)]);
            if let Some(url) = url {
                a = with_origin(a, url);
            }
            assert!(!a.hints().contains(&("o", "open")));
            assert_eq!(a.on_key(key('o')), Effect::None, "{url:?}");
            let status = a.status().expect("a status message");
            assert!(status.error && status.text.contains(text), "{url:?}");
        }
        // sessions: none selected, or without an origin
        for sessions in [vec![], vec![session(1, "s-a", "idle")]] {
            let mut a = sessions_app(vec![], sessions);
            assert_eq!(a.on_key(key('o')), Effect::None);
            assert!(a.status().is_some_and(|s| s.error));
        }
    }

    #[test]
    fn origins_are_read_once_from_the_runtime_dir() {
        let pid = std::process::id();
        let root = std::env::temp_dir().join(format!("agent-inbox-test-origins-{pid}"));
        let dir = root.join("claude-origin");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("o1.json");
        std::fs::write(&file, r#"{"title":"T","iid":7,"target":"MergeRequest"}"#).unwrap();
        let file = file.to_string_lossy().into_owned();
        let outside = root.join("outside.json");
        std::fs::write(&outside, r#"{"title":"X","iid":9,"target":"MergeRequest"}"#).unwrap();
        let outside = outside.to_string_lossy().into_owned();
        let mut hand = question("b", 980.0);
        hand.event["origin_file"] = json!(outside);
        let mut agent = question("a", 990.0);
        agent.event["origin_file"] = json!(file);
        let mut a = app(vec![hand, agent]);
        load_origins(&mut a, None);
        assert!(a.origins.is_empty());
        load_origins(&mut a, Some(&root));
        assert_eq!(a.origin_label(&file), Some("!7 T".into()));
        // a file outside the origin dir is not read
        assert_eq!(a.origins.get(&outside), Some(&None));
        // not read again
        std::fs::remove_dir_all(&root).unwrap();
        load_origins(&mut a, Some(&root));
        assert_eq!(a.origin_label(&file), Some("!7 T".into()));
    }

    /// `with_origin` with a description for the merge request.
    fn with_description(mut a: App, description: &str) -> App {
        let origin = a.origins.get_mut(ORIGIN_FILE).unwrap().as_mut().unwrap();
        origin.description = description.into();
        a
    }

    /// The row of the screen the text `needle` is on.
    fn row_of(s: &str, needle: &str) -> usize {
        s.lines()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} on the screen:\n{s}"))
    }

    fn waiting(id: &str, body: &str) -> Item {
        let mut w = claude(id, "claude.waiting", "", None, json!({}));
        w.event["body"] = json!(body);
        w
    }

    #[test]
    fn detail_shows_the_merge_request_below_the_item() {
        let a = app(vec![permission("p", Some(HOOK_REPLY))]);
        let a = with_description(with_origin(a, MR_URL), "Fixes the flaky test\nsecond line");
        let s = screen(&a);
        // the label is in the list row too
        assert_eq!(s.matches("!7 T").count(), 2, "{s}");
        let command = row_of(&s, "ls -la");
        let url = row_of(&s, MR_URL);
        let description = row_of(&s, "Fixes the flaky test");
        assert!(command < url && url < description, "{s}");
        assert_eq!(row_of(&s, "second line"), description + 1, "{s}");
    }

    #[test]
    fn detail_shows_the_prompt_of_a_task() {
        let mut a = app(vec![permission("p", Some(HOOK_REPLY))]);
        let origin = Origin {
            title: "Tidy up".into(),
            prompt: "Refactor the parser\nthen run the tests".into(),
            ..Origin::default()
        };
        a.origins.insert(ORIGIN_FILE.into(), Some(origin));
        let s = screen(&a);
        for text in ["Tidy up", "Refactor the parser", "then run the tests"] {
            assert!(s.contains(text), "{text}:\n{s}");
        }
        assert!(
            row_of(&s, "ls -la") < row_of(&s, "Refactor the parser"),
            "{s}"
        );
    }

    #[test]
    fn detail_of_a_waiting_item_shows_its_message() {
        let a = app(vec![waiting("w", "Done with the change\nPlease review it")]);
        let s = screen(&with_origin(a, MR_URL));
        assert!(s.contains("Please review it"), "{s}");
        assert!(
            row_of(&s, "Done with the change") < row_of(&s, MR_URL),
            "{s}"
        );
    }

    #[test]
    fn detail_of_an_origin_without_text_shows_the_label_and_the_link_only() {
        let a = with_origin(app(vec![permission("p", Some(HOOK_REPLY))]), MR_URL);
        let lines = origin_lines(&a, ORIGIN_FILE);
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
        assert_eq!(text, ["!7 T", MR_URL]);
    }

    #[test]
    fn session_detail_shows_the_waiting_message_and_then_the_origin() {
        let body = (1..=7)
            .map(|n| format!("msg line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let a = sessions_app(
            vec![in_session(waiting("w", &body), "s-a")],
            vec![session(1, "s-a", "idle")],
        );
        let a = with_description(with_origin(a, MR_URL), "Fixes the flaky test");
        let s = screen(&a);
        let (url, description) = (row_of(&s, MR_URL), row_of(&s, "Fixes the flaky test"));
        assert!(row_of(&s, "pid: 1") < row_of(&s, "msg line 1"), "{s}");
        assert!(row_of(&s, "msg line 5") < url && url < description, "{s}");
        for n in 1..=5 {
            assert!(s.contains(&format!("msg line {n}")), "{n}:\n{s}");
        }
        assert!(!s.contains("msg line 6"), "{s}");
        assert!(s.contains('…'), "{s}");
    }

    #[test]
    fn long_description_scrolls_to_its_end_in_detail_mode() {
        let description = (1..=30)
            .map(|n| format!("desc {n:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let a = app(vec![permission("p", Some(HOOK_REPLY))]);
        let mut a = with_description(with_origin(a, MR_URL), &description);
        a.on_key(code(KeyCode::Enter));
        let s = screen(&a);
        assert!(s.contains("desc 01") && !s.contains("desc 30"), "{s}");
        assert!(row_of(&s, "allow · n deny") < row_of(&s, "desc 01"), "{s}");
        for _ in 0..3 {
            a.on_key(code(KeyCode::PageDown));
        }
        let s = screen(&a);
        assert!(s.contains("desc 30"), "{s}");
    }

    // --- filter tabs

    const REVIEW_FILE: &str = "/run/user/1/claude-origin/review.json";
    const TASK_FILE: &str = "/run/user/1/claude-origin/task.json";

    /// Review shows the kind `review`, Coding the rest.
    fn tabs() -> Vec<Tab> {
        vec![
            Tab {
                name: "Review".into(),
                kinds: Some(vec!["review".into()]),
            },
            Tab {
                name: "Coding".into(),
                kinds: None,
            },
        ]
    }

    fn kind_origin(kind: &str) -> Option<Origin> {
        Some(Origin {
            kind: kind.into(),
            ..Origin::default()
        })
    }

    fn from_file(mut item: Item, file: &str) -> Item {
        item.event["origin_file"] = json!(file);
        item
    }

    fn session_from(mut s: Session, file: &str) -> Session {
        s.origin_file = file.into();
        s
    }

    /// Items (newest first) sudo `s`, review `r`, task `t` and `u` of an origin that has not
    /// been read.
    fn tabbed_app() -> App {
        let mut a = App {
            tabs: tabs(),
            area: Rect::new(0, 0, 100, 30),
            now: 1000.0,
            ..App::default()
        };
        a.origins.insert(REVIEW_FILE.into(), kind_origin("review"));
        a.origins.insert(TASK_FILE.into(), kind_origin("task"));
        a.set_items(vec![
            question("u", 900.0),
            from_file(question("t", 910.0), TASK_FILE),
            from_file(question("r", 920.0), REVIEW_FILE),
            sudo("s"),
        ]);
        a
    }

    /// `tabbed_app` on the Sessions tab, with the sessions 1 (review), 2 (task), 3 (unknown).
    fn tabbed_sessions_app() -> App {
        let mut a = tabbed_app();
        a.set_sessions(vec![
            session_from(session(1, "s-r", "idle"), REVIEW_FILE),
            session_from(session(2, "s-t", "idle"), TASK_FILE),
            session(3, "s-u", "idle"),
        ]);
        to_sessions_tab(&mut a);
        a
    }

    fn visible_ids(a: &App) -> Vec<&str> {
        a.visible().iter().map(|i| i.id.as_str()).collect()
    }

    fn visible_pids(a: &App) -> Vec<u32> {
        a.visible_sessions().iter().map(|s| s.pid).collect()
    }

    #[test]
    fn without_tabs_sessions_is_the_second_tab() {
        let mut a = app(vec![question("a", 990.0), question("b", 980.0)]);
        a.set_sessions(vec![
            session(1, "s-a", "idle"),
            session(2, "s-b", "idle"),
            session(3, "s-c", "idle"),
        ]);
        assert_eq!(current_tab(&a), "All");
        // the tab bar is the same on both tabs
        let s = screen(&a);
        assert!(s.contains("All 2 · Sessions 3"), "{s}");
        assert!(s.contains("title-a") && !s.contains("IDLE"), "{s}");
        assert!(a.hints().contains(&("f", "Sessions")), "{:?}", a.hints());
        assert!(screen(&a).contains("f Sessions"));
        assert_eq!(a.on_key(key('f')), Effect::None);
        assert_eq!(current_tab(&a), "Sessions");
        assert_eq!(visible_pids(&a), vec![1, 2, 3]);
        let s = screen(&a);
        assert!(s.contains("All 2 · Sessions 3"), "{s}");
        assert!(s.contains("IDLE") && !s.contains("title-a"), "{s}");
        assert!(a.hints().contains(&("f", "All")), "{:?}", a.hints());
        assert!(screen(&a).contains("f All"));
        assert_eq!(a.on_key(key('f')), Effect::None);
        assert_eq!(current_tab(&a), "All");
        assert!(screen(&a).contains("title-a"));
    }

    #[test]
    fn f_cycles_through_the_tabs_and_back() {
        let mut a = tabbed_app();
        a.set_sessions(vec![
            session_from(session(1, "s-r", "idle"), REVIEW_FILE),
            session_from(session(2, "s-t", "idle"), TASK_FILE),
            session(3, "s-u", "idle"),
        ]);
        assert_eq!(visible_ids(&a), vec!["s", "r", "t", "u"]);
        let s = screen(&a);
        assert!(
            s.contains("All 4 · Review 2 · Coding 3 · Sessions 3"),
            "{s}"
        );
        assert!(!s.contains(" Inbox "), "{s}");
        for (name, next) in [
            ("Review", "Coding"),
            ("Coding", "Sessions"),
            ("Sessions", "All"),
            ("All", "Review"),
        ] {
            assert_eq!(a.on_key(key('f')), Effect::None);
            assert_eq!(a.tab_name(a.tab), name);
            match name {
                "Review" => assert_eq!(visible_ids(&a), vec!["s", "r"]),
                "Coding" => assert_eq!(visible_ids(&a), vec!["s", "t", "u"]),
                "Sessions" => assert_eq!(visible_pids(&a), vec![1, 2, 3]),
                _ => assert_eq!(visible_ids(&a), vec!["s", "r", "t", "u"]),
            }
            assert!(a.hints().contains(&("f", next)), "{:?}", a.hints());
            assert!(screen(&a).contains(&format!("f {next}")));
        }
    }

    #[test]
    fn the_active_tab_is_bold_in_the_title() {
        let mut a = tabbed_sessions_app();
        let bold_tabs = |a: &App| -> Vec<String> {
            let title = a.tabs_title();
            let bold_spans = title
                .spans
                .iter()
                .filter(|s| s.style.add_modifier.contains(Modifier::BOLD));
            bold_spans.map(|s| s.content.to_string()).collect()
        };
        assert_eq!(bold_tabs(&a), vec!["Sessions 3"]);
        a.on_key(key('f'));
        assert_eq!(bold_tabs(&a), vec!["All 4"]);
        a.on_key(key('f'));
        assert_eq!(bold_tabs(&a), vec!["Review 2"]);
        a.on_key(key('f'));
        assert_eq!(bold_tabs(&a), vec!["Coding 3"]);
    }

    #[test]
    fn capital_s_does_nothing_on_any_tab() {
        let mut a = tabbed_sessions_app();
        for _ in 0..4 {
            let (tab, name) = (a.tab, current_tab(&a).to_string());
            assert_eq!(a.on_key(key('S')), Effect::None);
            assert_eq!((a.tab, current_tab(&a)), (tab, name.as_str()));
            assert!(matches!(a.mode, Mode::List));
            assert!(a.hints().iter().all(|(k, _)| *k != "S"), "{:?}", a.hints());
            a.on_key(key('f'));
        }
    }

    #[test]
    fn esc_on_the_sessions_tab_stays_there() {
        let mut a = tabbed_sessions_app();
        assert_eq!(a.on_key(code(KeyCode::Esc)), Effect::None);
        assert_eq!(current_tab(&a), "Sessions");
        assert!(a.session.is_none());
        assert!(screen(&a).contains("IDLE"));
    }

    #[test]
    fn the_selection_follows_its_item_across_tabs_else_clamps() {
        let mut a = tabbed_app();
        a.on_key(key('f'));
        a.on_key(key('f'));
        a.on_key(key('j'));
        a.on_key(key('j'));
        assert_eq!(selected_id(&a), Some("u"));
        // through the sessions tab
        a.on_key(key('f'));
        assert_eq!(current_tab(&a), "Sessions");
        a.on_key(key('f'));
        assert_eq!(current_tab(&a), "All");
        assert_eq!(selected_id(&a), Some("u"));
        assert_eq!(a.selected, 3);
        // not in Review: the position, within the shorter list
        a.on_key(key('f'));
        assert_eq!(selected_id(&a), Some("r"));
        assert_eq!(a.selected, 1);
    }

    #[test]
    fn an_empty_tab_says_so() {
        let mut a = tabbed_app();
        a.set_items(vec![from_file(question("t", 910.0), TASK_FILE)]);
        a.on_key(key('f'));
        let s = screen(&a);
        assert!(s.contains("No pending items in Review"), "{s}");
        a.on_key(key('f'));
        assert!(screen(&a).contains("title-t"));
    }

    #[test]
    fn dismiss_all_follows_the_tab() {
        let capital_d = KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT);
        let mut a = tabbed_app();
        for (n, ids) in [
            (3, vec!["r", "t", "u"]),
            (1, vec!["r"]),
            (2, vec!["t", "u"]),
        ] {
            a.on_key(capital_d);
            let s = screen(&a);
            let prompt = format!("Dismiss {n} items without a reply?");
            assert!(s.contains(&prompt), "{s}");
            assert_eq!(a.on_key(key('y')), Effect::DismissAll);
            let mut without_reply = a.without_reply();
            without_reply.sort();
            assert_eq!(without_reply, ids);
            a.on_key(key('f'));
        }
    }

    #[test]
    fn f_is_ignored_inside_a_session() {
        let mut a = tabbed_app();
        a.set_items(vec![
            in_session(question("a", 950.0), "s-x"),
            in_session(question("b", 960.0), "s-y"),
        ]);
        a.on_key(key('s'));
        assert_eq!(a.session.as_deref(), Some("s-y"));
        assert_eq!(a.on_key(key('f')), Effect::None);
        assert_eq!(a.tab, 0);
        let s = screen(&a);
        assert!(s.contains(" Session "), "{s}");
        assert!(
            s.contains("All 2 · Review 0 · Coding 2 · Sessions 0"),
            "{s}"
        );
        assert!(a.hints().iter().all(|(k, _)| *k != "f"));
        assert!(a.hints().contains(&("esc", "All")), "{:?}", a.hints());
        a.on_key(code(KeyCode::Esc));
        a.on_key(key('f'));
        assert_eq!(a.tab, 1);
    }

    #[test]
    fn a_session_opened_on_a_tab_returns_to_that_tab() {
        let mut a = tabbed_app();
        a.set_items(vec![
            in_session(from_file(question("a", 950.0), TASK_FILE), "s-x"),
            in_session(from_file(question("b", 960.0), TASK_FILE), "s-y"),
        ]);
        a.on_key(key('f'));
        a.on_key(key('f'));
        assert_eq!(current_tab(&a), "Coding");
        a.on_key(key('s'));
        assert_eq!(a.session.as_deref(), Some("s-y"));
        assert!(a.hints().contains(&("esc", "Coding")), "{:?}", a.hints());
        assert_eq!(a.on_key(key('f')), Effect::None);
        assert_eq!(current_tab(&a), "Coding");
        a.on_key(code(KeyCode::Esc));
        assert!(a.session.is_none());
        assert_eq!(current_tab(&a), "Coding");
        assert_eq!(visible_ids(&a), vec!["b", "a"]);
    }

    #[test]
    fn a_refresh_keeps_the_selection_when_new_origins_change_the_tab() {
        let pid = std::process::id();
        let root = std::env::temp_dir().join(format!("agent-inbox-test-tabs-{pid}"));
        let dir = root.join("claude-origin");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&dir).unwrap();
        let new = dir.join("new.json");
        std::fs::write(&new, r#"{"kind":"review"}"#).unwrap();
        let new = new.to_string_lossy().into_owned();
        // the new item (newest) has an origin that is not read yet: until it is, it shows in
        // Coding, afterwards in Review. Sessions are not filtered by tab; tab 3 is Sessions.
        for (tab, old) in [(1, REVIEW_FILE), (2, TASK_FILE), (3, TASK_FILE)] {
            let items = |ids: &[&str]| -> Vec<Item> {
                let file = |id: &str| if id == "n" { new.as_str() } else { old };
                let item = |id: &str| from_file(question(id, 900.0), file(id));
                ids.iter().map(|id| item(id)).collect()
            };
            let sessions = |pids: &[u32]| -> Vec<Session> {
                let file = |pid: u32| if pid == 4 { new.as_str() } else { old };
                let make = |pid: u32| session(pid, &format!("s-{pid}"), "idle");
                pids.iter()
                    .map(|&p| session_from(make(p), file(p)))
                    .collect()
            };
            let mut a = tabbed_app();
            a.tab = tab;
            a.set_items(items(&["i1", "i2", "i3"]));
            a.set_sessions(sessions(&[1, 2, 3]));
            a.selected = 1;
            a.session_sel = 1;
            assert_eq!(selected_pid(&a), Some(2));
            if tab < 3 {
                assert_eq!(selected_id(&a), Some("i2"));
            }
            let new_items = items(&["i1", "i2", "i3", "n"]);
            update(&mut a, sessions(&[4, 1, 2, 3]), Ok(new_items), Some(&root));
            assert_eq!(selected_pid(&a), Some(2), "tab {tab}");
            assert_eq!(visible_pids(&a), vec![4, 1, 2, 3], "tab {tab}");
            if tab < 3 {
                assert_eq!(selected_id(&a), Some("i2"), "tab {tab}");
                assert_eq!(visible_ids(&a).contains(&"n"), tab == 1);
            }
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_sessions_tab_shows_every_session_and_the_selection_follows_its_pid() {
        let mut a = tabbed_sessions_app();
        // whatever the kind of the origin
        assert_eq!(visible_pids(&a), vec![1, 2, 3]);
        let s = screen(&a);
        assert!(
            s.contains("All 4 · Review 2 · Coding 3 · Sessions 3"),
            "{s}"
        );
        assert!(a.hints().contains(&("f", "All")), "{:?}", a.hints());
        a.on_key(key('j'));
        a.on_key(key('j'));
        a.on_key(key('j'));
        assert_eq!(selected_pid(&a), Some(3));
        // All, Review, Coding, and round to the sessions again
        for _ in 0..4 {
            a.on_key(key('f'));
        }
        assert_eq!(current_tab(&a), "Sessions");
        assert_eq!(selected_pid(&a), Some(3));
        // refreshes follow the pid
        a.set_sessions(vec![
            session(4, "s-4", "idle"),
            session_from(session(2, "s-t", "idle"), TASK_FILE),
            session(3, "s-u", "idle"),
        ]);
        assert_eq!(selected_pid(&a), Some(3));
        assert_eq!(visible_pids(&a), vec![4, 2, 3]);
    }

    #[test]
    fn an_empty_sessions_tab_says_so_and_keeps_the_tab_bar_and_the_panes() {
        let mut a = tabbed_sessions_app();
        a.set_sessions(vec![]);
        let s = screen(&a);
        assert!(s.contains("No Claude sessions"), "{s}");
        assert!(!s.contains("No sessions in"), "{s}");
        assert!(
            s.contains("All 4 · Review 2 · Coding 3 · Sessions 0"),
            "{s}"
        );
        assert!(s.contains(" Detail "), "{s}");
        assert!(s.matches('┌').count() >= 2, "{s}");
        // the empty state still switches
        assert_eq!(a.on_key(key('f')), Effect::None);
        assert_eq!(current_tab(&a), "All");
    }

    // --- titles repeated by their text

    fn plain(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| &*s.content).collect::<String>())
            .collect()
    }

    fn waiting_titled(title: &str, body: &str) -> Item {
        let mut w = waiting("w", body);
        w.event["title"] = json!(title);
        w
    }

    #[test]
    fn starts_with_loosely_compares_letters_and_digits_only() {
        for (text, head) in [
            ("On why: `jw` was set", "On why jw was"),
            ("Hello, World", "hello world"),
            ("anything", ""),
        ] {
            assert!(starts_with_loosely(text, head), "{text:?} {head:?}");
        }
        for (text, head) in [("Hello", "Hello world"), ("Other text", "Title"), ("", "x")] {
            assert!(!starts_with_loosely(text, head), "{text:?} {head:?}");
        }
    }

    #[test]
    fn other_lines_skip_a_title_the_body_starts_with() {
        let title = "On why Shift+N skips: jw was set";
        let body = "On why Shift+N skips: `jw` was set to write the thing";
        let mut lines = Vec::new();
        other_lines(&waiting_titled(title, body), &mut lines);
        assert_eq!(plain(&lines), vec![body.to_string()]);

        let mut lines = Vec::new();
        other_lines(
            &waiting_titled("Build finished", "Something else entirely"),
            &mut lines,
        );
        assert_eq!(
            plain(&lines),
            vec![
                "Build finished".to_string(),
                "Something else entirely".into()
            ]
        );
    }

    #[test]
    fn detail_shows_a_title_the_body_starts_with_once() {
        let item = waiting_titled("Fix the *bug* now", "Fix the bug now, then more");
        let a = app(vec![item.clone()]);
        let text = plain(&detail_view(&a, &item)).join("\n");
        assert_eq!(text.matches("Fix the").count(), 1, "{text}");
        assert!(text.contains("then more"), "{text}");
    }

    #[test]
    fn origin_lines_skip_a_label_the_description_starts_with() {
        let origin = |description: &str| Origin {
            target: "MergeRequest".into(),
            iid: Some(12),
            title: "Fix foo".into(),
            description: description.into(),
            ..Origin::default()
        };
        let mut a = app(vec![]);
        a.origins.insert(
            ORIGIN_FILE.into(),
            Some(origin("!12 Fix foo\n\nMore details")),
        );
        let text = plain(&origin_lines(&a, ORIGIN_FILE)).join("\n");
        assert_eq!(text.matches("Fix foo").count(), 1, "{text}");
        assert!(text.contains("More details"), "{text}");

        a.origins
            .insert(ORIGIN_FILE.into(), Some(origin("Something different")));
        let lines = plain(&origin_lines(&a, ORIGIN_FILE));
        assert_eq!(lines.first().map(String::as_str), Some("!12 Fix foo"));
        assert!(
            lines.contains(&"Something different".to_string()),
            "{lines:?}"
        );
    }

    /// Enter on the selected session, then past the key guard.
    fn open_session(a: &mut App) {
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        assert!(matches!(a.mode, Mode::SessionDetail { .. }));
        a.now += 1.1;
    }

    fn scroll_of(a: &App) -> u16 {
        match a.mode {
            Mode::SessionDetail { scroll, .. } => scroll,
            _ => panic!("not in the session detail"),
        }
    }

    /// Whether the detail pane's border is focused.
    fn cyan_border(a: &App) -> bool {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| render(f, a)).unwrap();
        // the detail pane's right border, below the header and tab rows
        terminal.backend().buffer()[(99, 5)].fg == ratatui::style::Color::Cyan
    }

    /// A session (pid 1, s-a) with `n` pending items, so that its detail overflows.
    fn long_session_app(n: usize) -> App {
        let items = (0..n)
            .map(|i| in_session(question(&format!("i{i:02}"), 900.0 + i as f64), "s-a"))
            .collect();
        sessions_app(items, vec![session(1, "s-a", "idle")])
    }

    #[test]
    fn enter_and_tab_open_the_session_detail() {
        for open in [KeyCode::Enter, KeyCode::Tab] {
            let mut a = sessions_app(
                vec![],
                vec![session(1, "s-a", "idle"), session(2, "s-b", "idle")],
            );
            a.on_key(key('j'));
            assert_eq!(a.on_key(code(open)), Effect::None, "{open:?}");
            assert!(
                matches!(a.mode, Mode::SessionDetail { pid: 2, scroll: 0 }),
                "{open:?}"
            );
            assert!(a.session.is_none());
            assert_eq!(current_tab(&a), "Sessions");
            let s = screen(&a);
            assert!(s.contains("pid: 2") && s.contains("session: s-b"), "{s}");
        }
    }

    #[test]
    fn the_session_detail_border_is_focused() {
        let mut a = sessions_app(vec![], vec![session(1, "s-a", "idle")]);
        assert!(!cyan_border(&a));
        open_session(&mut a);
        assert!(cyan_border(&a));
        a.on_key(code(KeyCode::Esc));
        assert!(!cyan_border(&a));
    }

    #[test]
    fn enter_without_sessions_does_nothing() {
        for open in [KeyCode::Enter, KeyCode::Tab] {
            let mut a = sessions_app(vec![], vec![]);
            assert_eq!(a.on_key(code(open)), Effect::None);
            assert!(matches!(a.mode, Mode::List));
        }
    }

    #[test]
    fn the_session_detail_scrolls_and_clamps() {
        let mut a = long_session_app(60);
        open_session(&mut a);
        let s = screen(&a);
        assert!(s.contains("pid: 1") && !s.contains("title-i00"), "{s}");
        // not above the top
        a.on_key(key('k'));
        a.on_key(code(KeyCode::Up));
        a.on_key(code(KeyCode::PageUp));
        assert_eq!(scroll_of(&a), 0);
        // single steps and pages
        assert_eq!(a.on_key(key('j')), Effect::None);
        assert_eq!(scroll_of(&a), 1);
        a.on_key(code(KeyCode::Down));
        assert_eq!(scroll_of(&a), 2);
        a.on_key(code(KeyCode::PageDown));
        assert_eq!(scroll_of(&a), 12);
        a.on_key(code(KeyCode::PageUp));
        assert_eq!(scroll_of(&a), 2);
        a.on_key(key('k'));
        a.on_key(code(KeyCode::Up));
        assert_eq!(scroll_of(&a), 0);
        // down to the end, and not past it
        for _ in 0..10 {
            a.on_key(code(KeyCode::PageDown));
        }
        let end = scroll_of(&a);
        assert!(end > 12, "{end}");
        let s = screen(&a);
        assert!(s.contains("title-i00") && !s.contains("pid: 1"), "{s}");
        a.on_key(key('j'));
        a.on_key(code(KeyCode::Down));
        a.on_key(code(KeyCode::PageDown));
        assert_eq!(scroll_of(&a), end);
        // one step up is one row
        a.on_key(key('k'));
        assert_eq!(scroll_of(&a), end - 1);
        // the session stays selected
        assert!(matches!(a.mode, Mode::SessionDetail { pid: 1, .. }));
    }

    #[test]
    fn a_short_session_detail_does_not_scroll() {
        let mut a = sessions_app(vec![], vec![session(1, "s-a", "idle")]);
        open_session(&mut a);
        for c in [KeyCode::PageDown, KeyCode::Down, KeyCode::Char('j')] {
            a.on_key(code(c));
            assert_eq!(scroll_of(&a), 0);
        }
    }

    #[test]
    fn esc_q_backspace_and_tab_leave_the_session_detail_with_the_session_selected() {
        for leave in [
            KeyCode::Esc,
            KeyCode::Char('q'),
            KeyCode::Backspace,
            KeyCode::Tab,
        ] {
            let mut a = sessions_app(
                vec![],
                vec![
                    session(1, "s-a", "idle"),
                    session(2, "s-b", "idle"),
                    session(3, "s-c", "idle"),
                ],
            );
            a.on_key(key('j'));
            a.on_key(key('j'));
            open_session(&mut a);
            assert_eq!(a.on_key(code(leave)), Effect::None, "{leave:?}");
            assert!(matches!(a.mode, Mode::List), "{leave:?}");
            assert_eq!(selected_pid(&a), Some(3), "{leave:?}");
            assert_eq!(current_tab(&a), "Sessions");
        }
    }

    #[test]
    fn a_in_the_session_detail_attaches_or_says_why_not() {
        let mut a = sessions_app(vec![], vec![shpooled(session(1, "s-a", "idle"), "jw-1")]);
        open_session(&mut a);
        assert_eq!(a.on_key(key('a')), Effect::Attach("jw-1".into()));
        let mut a = sessions_app(vec![], vec![session(1, "s-a", "idle")]);
        open_session(&mut a);
        assert_eq!(a.on_key(key('a')), Effect::None);
        assert!(matches!(a.mode, Mode::SessionDetail { pid: 1, .. }));
        let status = a.status().expect("a status message");
        assert!(status.error && status.text.contains("not in a shpool session"));
    }

    #[test]
    fn o_in_the_session_detail_opens_the_link_or_says_why_not() {
        let a = sessions_app(vec![], vec![session(1, "s-a", "idle")]);
        let mut a = with_origin(a, MR_URL);
        open_session(&mut a);
        assert_eq!(a.on_key(key('o')), Effect::Open(MR_URL.into()));
        let mut a = sessions_app(vec![], vec![session(1, "s-a", "idle")]);
        open_session(&mut a);
        assert_eq!(a.on_key(key('o')), Effect::None);
        assert!(matches!(a.mode, Mode::SessionDetail { pid: 1, .. }));
        assert!(a.status().is_some_and(|s| s.error));
    }

    #[test]
    fn x_in_the_session_detail_kills_the_session() {
        let mut a = sessions_app(
            vec![],
            vec![
                started(session(7, "s-a", WAITING), 42),
                started(session(8, "s-b", "idle"), 43),
            ],
        );
        a.on_key(key('j'));
        open_session(&mut a);
        assert_eq!(a.on_key(key('x')), Effect::Kill { pid: 8, start: 43 });
        // no start time: refused, like from the list
        let mut a = sessions_app(vec![], vec![session(7, "s-a", "idle")]);
        open_session(&mut a);
        assert_eq!(a.on_key(key('x')), Effect::None);
        let status = a.status().expect("a status message");
        assert!(status.error && status.text.contains("not killed"));
    }

    #[test]
    fn other_keys_do_nothing_in_the_session_detail() {
        let mut a = tabbed_sessions_app();
        a.set_items(vec![in_session(question("a", 950.0), "s-r")]);
        a.on_key(key('j'));
        open_session(&mut a);
        let tab = a.tab;
        for c in [
            KeyCode::Enter,
            KeyCode::Char('s'),
            KeyCode::Char('f'),
            KeyCode::Char('N'),
            KeyCode::Char('d'),
            KeyCode::Char('?'),
            KeyCode::Char('y'),
            KeyCode::Char('D'),
        ] {
            a.now += 1.1;
            assert_eq!(a.on_key(code(c)), Effect::None, "{c:?}");
            assert!(
                matches!(a.mode, Mode::SessionDetail { pid: 2, scroll: 0 }),
                "{c:?}"
            );
            assert_eq!(a.tab, tab, "{c:?}");
            assert!(a.session.is_none(), "{c:?}");
        }
    }

    #[test]
    fn a_session_that_is_gone_says_so_and_only_esc_and_q_leave() {
        let gone = || {
            let mut a = sessions_app(
                vec![],
                vec![session(1, "s-a", "idle"), session(2, "s-b", "idle")],
            );
            a.on_key(key('j'));
            open_session(&mut a);
            a.set_sessions(vec![session(1, "s-a", "idle")]);
            a
        };
        let mut a = gone();
        assert!(matches!(a.mode, Mode::SessionDetail { pid: 2, .. }));
        let s = screen(&a);
        assert!(s.contains("session is gone"), "{s}");
        for c in [
            KeyCode::Enter,
            KeyCode::Tab,
            KeyCode::Backspace,
            KeyCode::Char('a'),
            KeyCode::Char('o'),
            KeyCode::Char('x'),
            KeyCode::Char('s'),
            KeyCode::Char('f'),
            KeyCode::Char('N'),
            KeyCode::Char('j'),
            KeyCode::PageDown,
        ] {
            a.now += 1.1;
            assert_eq!(a.on_key(code(c)), Effect::None, "{c:?}");
            assert!(
                matches!(a.mode, Mode::SessionDetail { pid: 2, .. }),
                "{c:?}"
            );
            assert!(screen(&a).contains("session is gone"), "{c:?}");
        }
        for leave in [KeyCode::Esc, KeyCode::Char('q')] {
            let mut a = gone();
            a.now += 1.1;
            assert_eq!(a.on_key(code(leave)), Effect::None, "{leave:?}");
            assert!(matches!(a.mode, Mode::List), "{leave:?}");
        }
    }

    // --- s: the items of a session

    #[test]
    fn s_shows_the_items_of_the_session_and_esc_returns() {
        let mut a = sessions_app(
            vec![
                in_session(question("a", 960.0), "s-a"),
                in_session(question("b", 970.0), "s-b"),
            ],
            vec![session(1, "s-a", "idle"), session(2, "s-b", "idle")],
        );
        a.on_key(key('j'));
        assert_eq!(a.on_key(key('s')), Effect::None);
        assert!(matches!(a.mode, Mode::List));
        assert_eq!(current_tab(&a), "Sessions");
        assert_eq!(a.session.as_deref(), Some("s-b"));
        assert_eq!(a.selected, 0);
        assert_eq!(selected_id(&a), Some("b"));
        let s = screen(&a);
        assert!(s.contains(" Session ") && !s.contains("IDLE"), "{s}");
        assert!(s.contains("title-b") && !s.contains("title-a"), "{s}");
        assert!(a.hints().contains(&("esc", "Sessions")), "{:?}", a.hints());
        assert!(s.contains("esc Sessions"), "{s}");
        a.on_key(code(KeyCode::Esc));
        assert!(a.session.is_none());
        assert_eq!(current_tab(&a), "Sessions");
        assert_eq!(selected_pid(&a), Some(2));
        assert!(screen(&a).contains("IDLE"));
        // `s` leaves the drill-down the same way (once the guard is down)
        a.now += 1.1;
        a.on_key(key('s'));
        assert_eq!(a.session.as_deref(), Some("s-b"));
        a.now += 1.1;
        a.on_key(key('s'));
        assert!(a.session.is_none());
        assert_eq!(current_tab(&a), "Sessions");
        assert_eq!(selected_pid(&a), Some(2));
        assert!(screen(&a).contains("IDLE"));
    }

    #[test]
    fn s_without_items_says_so() {
        let mut a = sessions_app(
            vec![in_session(question("a", 960.0), "s-other")],
            vec![session(1, "s-a", "idle")],
        );
        assert_eq!(a.on_key(key('s')), Effect::None);
        assert_eq!(current_tab(&a), "Sessions");
        assert!(a.session.is_none());
        assert!(matches!(a.mode, Mode::List));
        let s = screen(&a);
        assert!(
            s.contains("no pending items") && !s.contains("a to attach"),
            "{s}"
        );
        // attachable: the way on is named
        let mut a = sessions_app(vec![], vec![shpooled(session(1, "s-a", "idle"), "jw-1")]);
        a.on_key(key('s'));
        assert!(a.session.is_none());
        assert!(screen(&a).contains("no pending items — a to attach"));
        // the items of an unreachable router are stale
        let mut a = sessions_app(
            vec![in_session(question("a", 960.0), "s-a")],
            vec![shpooled(session(1, "s-a", "idle"), "jw-1")],
        );
        a.offline = Some("down".into());
        a.on_key(key('s'));
        assert_eq!(current_tab(&a), "Sessions");
        assert!(a.session.is_none());
        assert!(screen(&a).contains("no pending items — a to attach"));
        // no sessions at all
        let mut a = sessions_app(vec![], vec![]);
        assert_eq!(a.on_key(key('s')), Effect::None);
        assert!(a.session.is_none());
    }

    #[test]
    fn a_held_s_opens_the_session_but_not_its_first_item() {
        let mut a = sessions_app(
            vec![in_session(question("a", 960.0), "s-a")],
            vec![session(1, "s-a", "idle")],
        );
        a.on_key(key('s'));
        assert_eq!(a.session.as_deref(), Some("s-a"));
        // the repeats are swallowed: the drill-down is not left either
        a.now += 0.6;
        assert_eq!(a.on_key(key('s')), Effect::None);
        assert_eq!(a.session.as_deref(), Some("s-a"));
        assert!(matches!(a.mode, Mode::List));
        // released: Enter opens the item
        a.now += 1.1;
        a.on_key(code(KeyCode::Enter));
        assert!(matches!(a.mode, Mode::Detail { .. }));
    }

    #[test]
    fn f_is_ignored_in_a_drill_down_from_the_sessions_tab() {
        let mut a = sessions_app(
            vec![in_session(question("a", 960.0), "s-a")],
            vec![session(1, "s-a", "idle")],
        );
        a.on_key(key('s'));
        assert!(a.session.is_some());
        assert_eq!(a.on_key(key('f')), Effect::None);
        assert_eq!(current_tab(&a), "Sessions");
        assert!(a.session.is_some());
        assert!(a.hints().iter().all(|(k, _)| *k != "f"));
    }

    #[test]
    fn the_last_item_answered_returns_to_the_sessions() {
        let mut a = sessions_app(
            vec![in_session(permission("p", Some(HOOK_REPLY)), "s-a")],
            vec![session(1, "s-a", WAITING)],
        );
        a.on_key(key('s'));
        a.now += 1.1;
        a.on_key(code(KeyCode::Enter));
        a.now += 1.1;
        assert_eq!(a.on_key(key('y')), allow("p"));
        assert_eq!(a.session.as_deref(), Some("s-a"));
        // the router has dismissed it
        a.set_items(vec![]);
        assert!(a.session.is_none());
        assert_eq!(current_tab(&a), "Sessions");
        let s = screen(&a);
        assert!(s.contains("WAIT") && !s.contains(" Session "), "{s}");
    }

    #[test]
    fn an_item_vanishing_under_an_open_detail_keeps_the_drill_down_until_esc() {
        let mut a = sessions_app(
            vec![in_session(permission("p", Some(HOOK_REPLY)), "s-a")],
            vec![session(1, "s-a", WAITING)],
        );
        a.on_key(key('s'));
        a.now += 1.1;
        a.on_key(code(KeyCode::Enter));
        assert!(matches!(a.mode, Mode::Detail { .. }));
        a.set_items(vec![]);
        assert_eq!(a.session.as_deref(), Some("s-a"));
        assert!(matches!(a.mode, Mode::Detail { .. }));
        assert!(screen(&a).contains("item is gone"));
        a.on_key(code(KeyCode::Esc));
        assert!(matches!(a.mode, Mode::List));
        a.set_items(vec![]);
        assert!(a.session.is_none());
        assert_eq!(current_tab(&a), "Sessions");
        assert!(screen(&a).contains("WAIT"));
    }

    #[test]
    fn items_of_a_session_entered_from_the_sessions_tab_stay_visible() {
        let mut a = tabbed_sessions_app();
        let item = in_session(from_file(question("r", 950.0), REVIEW_FILE), "s-r");
        let task = in_session(from_file(question("t", 940.0), TASK_FILE), "s-t");
        a.set_items(vec![item, task]);
        assert_eq!(selected_pid(&a), Some(1));
        a.on_key(key('s'));
        assert_eq!(a.session.as_deref(), Some("s-r"));
        assert_eq!(current_tab(&a), "Sessions");
        assert_eq!(visible_ids(&a), vec!["r"]);
        a.on_key(code(KeyCode::Esc));
        a.on_key(key('j'));
        a.now += 1.1;
        a.on_key(key('s'));
        assert_eq!(a.session.as_deref(), Some("s-t"));
        assert_eq!(visible_ids(&a), vec!["t"]);
    }

    // --- hints

    #[test]
    fn sessions_list_hints() {
        // everything on offer, and no j/k
        let a = sessions_app(
            vec![in_session(question("a", 960.0), "s-a")],
            vec![started(shpooled(session(1, "s-a", "idle"), "jw-1"), 42)],
        );
        let a = with_origin(a, MR_URL);
        assert_eq!(
            a.hints(),
            vec![
                ("enter", "open"),
                ("s", "items"),
                ("a", "attach"),
                ("o", "open"),
                ("x", "kill"),
                ("f", "All"),
                ("N", "new"),
                ("q", "quit"),
            ]
        );
        let s = screen(&a);
        assert!(s.contains("enter open") && s.contains("s items"), "{s}");
        // the bare minimum: no items, nothing to attach, open or kill
        let a = sessions_app(vec![], vec![session(1, "s-a", "idle")]);
        assert_eq!(
            a.hints(),
            vec![("enter", "open"), ("f", "All"), ("N", "new"), ("q", "quit")]
        );
        // items of another session do not count
        let a = sessions_app(
            vec![in_session(question("a", 960.0), "s-other")],
            vec![session(1, "s-a", "idle")],
        );
        assert!(a.hints().iter().all(|(k, _)| *k != "s"));
        // no sessions
        let a = sessions_app(vec![], vec![]);
        assert_eq!(a.hints(), vec![("f", "All"), ("N", "new"), ("q", "quit")]);
    }

    #[test]
    fn session_detail_hints() {
        let a = sessions_app(
            vec![],
            vec![started(shpooled(session(1, "s-a", "idle"), "jw-1"), 42)],
        );
        let mut a = with_origin(a, MR_URL);
        open_session(&mut a);
        assert_eq!(
            a.hints(),
            vec![
                ("j/k", "scroll"),
                ("a", "attach"),
                ("o", "open"),
                ("x", "kill"),
                ("esc", "back"),
            ]
        );
        let s = screen(&a);
        assert!(s.contains("j/k scroll") && s.contains("esc back"), "{s}");
        let mut a = sessions_app(vec![], vec![session(1, "s-a", "idle")]);
        open_session(&mut a);
        assert_eq!(a.hints(), vec![("j/k", "scroll"), ("esc", "back")]);
    }

    // --- slice D: one set of navigation keys

    #[test]
    fn answer_mode_ignores_other_command_and_ctrl_keys() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        for c in "Dyp0".chars() {
            assert_eq!(a.on_key(key(c)), Effect::None);
        }
        for modifier in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            assert_eq!(
                a.on_key(KeyEvent::new(KeyCode::Char('j'), modifier)),
                Effect::None
            );
        }
        let s = screen(&a);
        assert!(s.contains("Question 1/2"));
        assert!(s.contains("> 1 Postgres"));
    }

    fn three_questions() -> App {
        app(vec![
            question("old", 900.0),
            question("mid", 950.0),
            question("new", 990.0),
        ])
    }

    #[test]
    fn g_and_capital_g_jump_to_the_first_and_last_item_row() {
        let mut a = three_questions();
        assert_eq!(selected_id(&a), Some("new"));
        assert_eq!(a.on_key(key('G')), Effect::None);
        assert_eq!(selected_id(&a), Some("old"));
        assert_eq!(a.on_key(key('G')), Effect::None);
        assert_eq!(selected_id(&a), Some("old"));
        assert_eq!(a.on_key(key('g')), Effect::None);
        assert_eq!(selected_id(&a), Some("new"));
        assert!(matches!(a.mode, Mode::List));
        // on a filter tab: the rows of that tab
        let mut a = tabbed_app();
        a.on_key(key('f'));
        let ids: Vec<String> = visible_ids(&a).iter().map(|s| s.to_string()).collect();
        assert!(!ids.is_empty());
        a.on_key(key('G'));
        assert_eq!(selected_id(&a), Some(ids[ids.len() - 1].as_str()));
        a.on_key(key('g'));
        assert_eq!(selected_id(&a), Some(ids[0].as_str()));
        // empty list: nothing happens
        let mut a = app(vec![]);
        assert_eq!(a.on_key(key('G')), Effect::None);
        assert_eq!(a.on_key(key('g')), Effect::None);
    }

    #[test]
    fn g_and_capital_g_jump_in_the_sessions_list() {
        let mut a = sessions_app(
            vec![],
            vec![
                session(1, "s-a", "idle"),
                session(2, "s-b", "idle"),
                session(3, "s-c", "idle"),
            ],
        );
        let pids: Vec<u32> = visible_pids(&a);
        let (first, last) = (pids[0], pids[pids.len() - 1]);
        assert_eq!(a.on_key(key('G')), Effect::None);
        assert_eq!(selected_pid(&a), Some(last));
        assert_eq!(a.on_key(key('g')), Effect::None);
        assert_eq!(selected_pid(&a), Some(first));
        assert_eq!(current_tab(&a), "Sessions");
    }

    #[test]
    fn g_and_capital_g_jump_in_a_session_drill_down() {
        let mut a = sessions_app(
            vec![
                in_session(question("a1", 960.0), "s-a"),
                in_session(question("a2", 970.0), "s-a"),
                in_session(question("a3", 980.0), "s-a"),
                in_session(question("b", 990.0), "s-b"),
            ],
            vec![session(1, "s-a", "idle"), session(2, "s-b", "idle")],
        );
        a.on_key(key('s'));
        assert_eq!(a.session.as_deref(), Some("s-a"));
        let ids: Vec<String> = visible_ids(&a).iter().map(|s| s.to_string()).collect();
        assert_eq!(ids.len(), 3);
        a.on_key(key('G'));
        assert_eq!(selected_id(&a), Some(ids[2].as_str()));
        a.on_key(key('g'));
        assert_eq!(selected_id(&a), Some(ids[0].as_str()));
        assert_eq!(a.session.as_deref(), Some("s-a"));
    }

    #[test]
    fn l_opens_like_enter_from_the_lists() {
        // item list: the detail
        let mut a = app(vec![question("a", 990.0)]);
        assert_eq!(a.on_key(key('l')), Effect::None);
        assert!(matches!(&a.mode, Mode::Detail { id, .. } if id == "a"));
        // sessions list: the session detail
        let mut a = sessions_app(vec![], vec![session(1, "s-a", "idle")]);
        assert_eq!(a.on_key(key('l')), Effect::None);
        assert!(matches!(a.mode, Mode::SessionDetail { pid: 1, .. }));
        // drill-down list: the item detail
        let mut a = sessions_app(
            vec![in_session(question("a", 960.0), "s-a")],
            vec![session(1, "s-a", "idle")],
        );
        a.on_key(key('s'));
        a.now += 1.1;
        assert_eq!(a.on_key(key('l')), Effect::None);
        assert!(matches!(&a.mode, Mode::Detail { id, .. } if id == "a"));
    }

    #[test]
    fn l_opens_a_question_and_a_held_l_does_not_pick() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        assert_eq!(a.on_key(key('l')), Effect::None);
        assert!(matches!(a.mode, Mode::Answer { .. }));
        assert!(screen(&a).contains("Question 1/2"));
        a.now += 0.6;
        assert_eq!(a.on_key(key('l')), Effect::None);
        for _ in 0..50 {
            a.now += 0.03;
            assert_eq!(a.on_key(key('l')), Effect::None);
        }
        assert!(matches!(&a.mode, Mode::Answer { st, .. } if st.idx == 0 && st.done.is_empty()));
        // released: the next `l` picks the first option
        a.now += 1.1;
        a.on_key(key('l'));
        assert!(screen(&a).contains("✓ Which db? → Postgres"));
    }

    #[test]
    fn h_does_nothing_in_a_top_level_list() {
        let mut a = three_questions();
        a.on_key(key('j'));
        assert_eq!(a.on_key(key('h')), Effect::None);
        assert!(matches!(a.mode, Mode::List));
        assert_eq!(selected_id(&a), Some("mid"));
        // on the Sessions tab it stays there
        let mut a = sessions_app(
            vec![],
            vec![session(1, "s-a", "idle"), session(2, "s-b", "idle")],
        );
        a.on_key(key('j'));
        let pid = selected_pid(&a);
        assert_eq!(a.on_key(key('h')), Effect::None);
        assert!(matches!(a.mode, Mode::List));
        assert_eq!(current_tab(&a), "Sessions");
        assert_eq!(selected_pid(&a), pid);
        assert!(a.session.is_none());
    }

    #[test]
    fn h_leaves_a_drill_down_like_esc() {
        let mk = || {
            sessions_app(
                vec![in_session(question("a", 960.0), "s-a")],
                vec![session(1, "s-a", "idle"), session(2, "s-b", "idle")],
            )
        };
        let mut a = mk();
        a.on_key(key('s'));
        assert!(a.session.is_some());
        a.now += 1.1;
        assert_eq!(a.on_key(key('h')), Effect::None);
        assert!(a.session.is_none());
        assert!(matches!(a.mode, Mode::List));
        assert_eq!(current_tab(&a), "Sessions");
        // same result as Esc
        let mut b = mk();
        b.on_key(key('s'));
        b.now += 1.1;
        b.on_key(code(KeyCode::Esc));
        assert_eq!(a.session, b.session);
        assert_eq!(selected_pid(&a), selected_pid(&b));
        // from an item tab's drill-down
        let mut a = app(vec![
            in_session(question("x", 960.0), "s-1"),
            in_session(question("y", 970.0), "s-1"),
        ]);
        a.on_key(key('s'));
        assert_eq!(a.session.as_deref(), Some("s-1"));
        a.now += 1.1;
        a.on_key(key('h'));
        assert!(a.session.is_none());
        assert_eq!(selected_id(&a), Some("y"));
    }

    fn long_permission_app() -> App {
        let cmd = (0..60)
            .map(|i| format!("cmd-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        app(vec![claude(
            "p",
            "claude.permission_request",
            "Bash",
            Some(HOOK_REPLY),
            json!({"command": cmd}),
        )])
    }

    #[test]
    fn g_and_capital_g_scroll_the_item_detail_to_the_top_and_the_end() {
        let mut a = long_permission_app();
        a.on_key(code(KeyCode::Enter));
        assert!(screen(&a).contains("cmd-0"));
        assert_eq!(a.on_key(key('G')), Effect::None);
        // the end of the detail: the (cut) command, then the options
        let s = screen(&a);
        assert!(
            s.contains("y allow · n deny") && !s.contains("cmd-0"),
            "{s}"
        );
        let Mode::Detail { scroll: end, .. } = a.mode else {
            panic!("left the detail");
        };
        assert!(end > 0);
        // G is the same end as paging down as far as it goes
        let mut b = long_permission_app();
        b.on_key(code(KeyCode::Enter));
        for _ in 0..20 {
            b.on_key(code(KeyCode::PageDown));
        }
        assert!(matches!(b.mode, Mode::Detail { scroll, .. } if scroll == end));
        assert_eq!(a.on_key(key('g')), Effect::None);
        assert!(matches!(a.mode, Mode::Detail { scroll: 0, .. }));
        assert!(screen(&a).contains("cmd-0"));
    }

    #[test]
    fn h_leaves_the_item_detail() {
        let mut a = app(vec![question("a", 990.0), question("b", 995.0)]);
        a.on_key(key('j'));
        a.on_key(code(KeyCode::Enter));
        assert!(matches!(a.mode, Mode::Detail { .. }));
        assert_eq!(a.on_key(key('h')), Effect::None);
        assert!(matches!(a.mode, Mode::List));
        assert_eq!(selected_id(&a), Some("a"));
    }

    #[test]
    fn g_and_capital_g_scroll_the_session_detail() {
        let mut a = long_session_app(60);
        open_session(&mut a);
        assert_eq!(a.on_key(key('G')), Effect::None);
        let end = scroll_of(&a);
        assert!(end > 12, "{end}");
        // the same end as paging down as far as it goes
        let mut b = long_session_app(60);
        open_session(&mut b);
        for _ in 0..20 {
            b.on_key(code(KeyCode::PageDown));
        }
        assert_eq!(scroll_of(&b), end);
        a.on_key(key('G'));
        assert_eq!(scroll_of(&a), end);
        assert_eq!(a.on_key(key('g')), Effect::None);
        assert_eq!(scroll_of(&a), 0);
        // a short detail has nothing to scroll
        let mut a = sessions_app(vec![], vec![session(1, "s-a", "idle")]);
        open_session(&mut a);
        a.on_key(key('G'));
        assert_eq!(scroll_of(&a), 0);
        a.on_key(key('g'));
        assert_eq!(scroll_of(&a), 0);
    }

    #[test]
    fn h_leaves_the_session_detail_with_the_session_selected() {
        let mut a = sessions_app(
            vec![],
            vec![
                session(1, "s-a", "idle"),
                session(2, "s-b", "idle"),
                session(3, "s-c", "idle"),
            ],
        );
        a.on_key(key('j'));
        a.on_key(key('j'));
        open_session(&mut a);
        assert_eq!(a.on_key(key('h')), Effect::None);
        assert!(matches!(a.mode, Mode::List));
        assert_eq!(selected_pid(&a), Some(3));
        assert_eq!(current_tab(&a), "Sessions");
    }

    #[test]
    fn q_leaves_answering_to_the_list_like_esc_and_h() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        a.on_key(code(KeyCode::Enter));
        assert!(screen(&a).contains("Question 2/2"));
        assert_eq!(a.on_key(key('q')), Effect::None);
        assert!(matches!(a.mode, Mode::List));
        // it starts over
        a.on_key(code(KeyCode::Enter));
        let s = screen(&a);
        assert!(s.contains("Question 1/2") && !s.contains("✓"), "{s}");
    }

    #[test]
    fn q_h_g_and_l_are_text_in_the_other_input_and_the_other_inputs() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        a.on_key(key('G'));
        a.on_key(code(KeyCode::Enter));
        type_text(&mut a, "qhgl");
        assert!(matches!(a.mode, Mode::Answer { .. }));
        assert!(screen(&a).contains("Other: qhgl"));
        // password
        let mut a = app(vec![sudo("s")]);
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('p'));
        for c in "qhgl".chars() {
            assert_eq!(a.on_key(key(c)), Effect::None);
        }
        assert!(matches!(&a.mode, Mode::Password { buf, .. } if buf == "qhgl"));
        // task
        let mut a = app(vec![question("a", 990.0)]);
        a.on_key(key('N'));
        type_text(&mut a, "qhgl");
        assert!(matches!(&a.mode, Mode::Task { buf } if buf == "qhgl"));
    }
}
