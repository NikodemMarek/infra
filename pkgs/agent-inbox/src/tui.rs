//! The terminal UI: `App` is the state and key handling (pure, returns an `Effect` for the loop
//! to carry out), `render` draws it, `run` is the loop around the router client.
//!
//! Layout: a header line, the list of items next to (below, on narrow terminals) a detail pane
//! for the selected item, and a footer with key hints or the last status message. `S` switches
//! to the same layout over the live Claude sessions (list and detail of the selected session).

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use serde_json::{json, Map, Value};

use crate::client::{self, clean, Item, Question};
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
const PROJECT_MAX_WIDTH: usize = 24;
/// The status of a session that waits for the user.
const WAITING: &str = "waiting";
/// Longest multi-line value (a command, ...) shown in the detail pane.
const MAX_LINES: usize = 40;

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
}

#[derive(Default)]
enum Mode {
    /// Focus in the list; the detail pane shows the selected item.
    #[default]
    List,
    /// Focus in the detail pane (scrolling).
    Detail { id: String, scroll: u16 },
    /// Masked input; the buffer is moved out into `Effect::Send` on Enter.
    Password { id: String, buf: String },
    /// Choosing the answers of a claude.question item.
    Answer { id: String, st: Answering },
    /// One key (`y`) confirms dismissing every item without a reply target, any other cancels.
    ConfirmDismissAll,
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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum View {
    /// The inbox items.
    #[default]
    Inbox,
    /// The live Claude sessions (the mode is `List`).
    Sessions,
}

#[derive(Default)]
struct App {
    view: View,
    /// Newest first.
    items: Vec<Item>,
    /// Index into `visible()`.
    selected: usize,
    /// Show only the items of this Claude session.
    session: Option<String>,
    /// The session view was entered from the sessions list: leaving it returns there.
    from_sessions: bool,
    /// Newest first.
    sessions: Vec<Session>,
    /// Index into `sessions`.
    session_sel: usize,
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
        // A session without items is over (but not under the dismiss-all prompt: it would
        // widen to all items; the next refresh ends it).
        if self.visible().is_empty() && !matches!(self.mode, Mode::ConfirmDismissAll) {
            self.session = None;
            self.session_ended();
        }
        self.select_id(followed);
        // The mode is kept even if the open item is gone: switching to the list would turn the
        // rest of a typed password or answer into list commands. The detail pane and send()
        // report the loss.
    }

    fn find(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// The items of the session view, or all of them.
    fn visible(&self) -> Vec<&Item> {
        let session = self.session.as_deref();
        self.items
            .iter()
            .filter(|i| session.is_none_or(|s| i.session_id() == s))
            .collect()
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
        if self.session.is_none() {
            self.session_ended();
        }
    }

    /// After the session view ended: back to the sessions list if it was entered from there
    /// (not while an item is open: the list is not in focus).
    fn session_ended(&mut self) {
        if std::mem::take(&mut self.from_sessions) && matches!(self.mode, Mode::List) {
            self.view = View::Sessions;
        }
    }

    /// The live sessions as `sessions::load` returns them. The selection follows its pid.
    fn set_sessions(&mut self, sessions: Vec<Session>) {
        let followed = self.selected_session().map(|s| s.pid);
        self.sessions = sessions;
        self.session_sel = followed
            .and_then(|pid| self.sessions.iter().position(|s| s.pid == pid))
            .unwrap_or(self.session_sel)
            .min(self.sessions.len().saturating_sub(1));
    }

    fn selected_session(&self) -> Option<&Session> {
        self.sessions.get(self.session_sel)
    }

    /// The pending items of `session` (none while the router is unreachable: they are stale).
    fn session_items(&self, session: &Session) -> Vec<&Item> {
        if self.offline.is_some() || session.session_id.is_empty() {
            return Vec::new();
        }
        self.items
            .iter()
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
            Mode::List if self.view == View::Sessions => self.sessions_key(code),
            Mode::List => self.list_key(code),
            Mode::Detail { id, scroll } => self.detail_key(id, scroll, code),
            Mode::Password { id, buf } => self.password_key(id, buf, code),
            Mode::Answer { id, st } => self.answer_key(id, st, code),
            Mode::ConfirmDismissAll if code == KeyCode::Char('y') => Effect::DismissAll,
            Mode::ConfirmDismissAll => Effect::None,
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
            KeyCode::Char('S') => self.view = View::Sessions,
            KeyCode::Esc | KeyCode::Char('s') if self.session.is_some() => self.set_session(None),
            KeyCode::Char('s') => {
                let session = self.selected_item().map(|i| i.session_id().to_string());
                match session.filter(|s| !s.is_empty()) {
                    Some(session) => self.set_session(Some(session)),
                    None => self.notify("no Claude session on this item", false),
                }
            }
            KeyCode::Enter | KeyCode::Tab => {
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
            // Dismiss, attach and raw view only: decisions and input modes are for the detail
            // pane (a held key must not decide the items that move up under the selection).
            KeyCode::Char('d' | 'a' | '?') => {
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
        let last = self.sessions.len().saturating_sub(1);
        match code {
            KeyCode::Char('q') => return Effect::Quit,
            KeyCode::Char('j') | KeyCode::Down => {
                self.session_sel = (self.session_sel + 1).min(last)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.session_sel = self.session_sel.saturating_sub(1)
            }
            KeyCode::Esc | KeyCode::Char('S') => self.view = View::Inbox,
            KeyCode::Enter => {
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
                    self.guard = Some((self.now, code)); // a held Enter must not open an item too
                    self.view = View::Inbox;
                    self.set_session(Some(id));
                    self.selected = 0;
                    self.from_sessions = true;
                }
            }
            KeyCode::Char('a') => {
                let name = self.selected_session().map(|s| s.shpool.clone());
                match name.filter(|n| !n.is_empty()) {
                    Some(name) => return Effect::Attach(name),
                    None => self.notify("not in a shpool session", true),
                }
            }
            _ => {}
        }
        Effect::None
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

    /// Keys that act on the item `id` (from the detail pane and the answer choice, `d`, `a` and
    /// `?` also from the list); None = not one of them. Decisions need a plain key (no Enter)
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
        let step = |delta: i32| (i32::from(scroll) + delta).clamp(0, i32::from(max)) as u16;
        let scrolled = match code {
            KeyCode::Char('j') | KeyCode::Down => Some(step(1)),
            KeyCode::Char('k') | KeyCode::Up => Some(step(-1)),
            KeyCode::PageDown => Some(step(10)),
            KeyCode::PageUp => Some(step(-10)),
            _ => None,
        };
        self.mode = Mode::Detail {
            id: id.clone(),
            scroll: scrolled.unwrap_or(scroll),
        };
        if scrolled.is_some() {
            return Effect::None;
        }
        match code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace | KeyCode::Tab => {
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
        let inner = Block::bordered().inner(panes(self.area).detail);
        let rows = wrapped_height(&detail_view(self, item), inner.width);
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
                KeyCode::Esc | KeyCode::Char('h') => {
                    self.mode = Mode::List;
                    return Effect::None;
                }
                // deny, dismiss and the raw view, as in the detail pane
                KeyCode::Char('n' | 'd' | '?') => match self.item_key(id.clone(), code) {
                    Some(effect) if effect != Effect::None => return effect,
                    _ => {}
                },
                // attach keeps the choice open (item_key leaves the mode alone)
                KeyCode::Char('a') => {
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

    /// Key hints for the footer (key, label).
    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.view == View::Sessions {
            let mut h = vec![("j/k", "move")];
            if let Some(session) = self.selected_session() {
                if !self.session_items(session).is_empty() {
                    h.push(("enter", "items"));
                }
                if !session.shpool.is_empty() {
                    h.push(("a", "attach"));
                }
            }
            h.push(("S", "inbox"));
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
                    if item.reply().is_none() {
                        h.push(("D", "dismiss read-only"));
                    }
                    h.push(("?", "raw"));
                    if self.session.is_none() && !item.session_id().is_empty() {
                        h.push(("s", "session"));
                    }
                }
                if self.session.is_some() {
                    let back = if self.from_sessions {
                        "sessions"
                    } else {
                        "all items"
                    };
                    h.push(("esc", back));
                }
                h.push(("S", "sessions"));
                h.push(("q", "quit"));
                h
            }
            Mode::Detail { id, .. } => {
                let mut h = vec![("j/k", "scroll")];
                if let Some(item) = self.find(id) {
                    h.extend(action_hints(item));
                    h.push(("d", "dismiss"));
                    if !item.shpool().is_empty() {
                        h.push(("a", "attach"));
                    }
                    h.push(("?", "raw"));
                }
                h.push(("esc", "back"));
                h
            }
            Mode::Password { .. } => vec![("enter", "send"), ("esc", "cancel")],
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
                h
            }
        }
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

/// Items of an unknown kind (and questions without any): the title and what else there is.
fn other_lines(item: &Item, lines: &mut Vec<Line<'static>>) {
    lines.push(Line::styled(clean(item.title()), bold()));
    for value in [item.command(), item.prompt()] {
        if !value.is_empty() && value != item.title() {
            lines.extend(lines_of(value, Style::new()));
        }
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
            // the end of a long text, so what is typed stays visible
            let room = usize::from(width).saturating_sub("Other: ".len() + 1);
            let skip = buf.chars().count().saturating_sub(room);
            let tail: String = buf.chars().skip(skip).collect();
            Line::from(vec![
                Span::styled("Other: ", bold()),
                Span::raw(clean(&tail)),
                cursor,
            ])
        }),
        _ => None,
    }
}

struct Panes {
    header: Rect,
    list: Rect,
    detail: Rect,
    footer: Rect,
}

fn panes(area: Rect) -> Panes {
    let [header, body, footer] = Layout::vertical([
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
    let list_focused = matches!(app.mode, Mode::List | Mode::ConfirmDismissAll);
    if app.view == View::Sessions {
        if app.sessions.is_empty() {
            render_empty(f, &p, "No Claude sessions");
        } else {
            render_sessions(f, app, p.list);
            render_session_detail(f, app, p.detail);
        }
    } else if app.visible().is_empty() && list_focused {
        render_empty(f, &p, "No pending requests");
    } else {
        render_list(f, app, p.list);
        render_detail(f, app, p.detail);
    }
    render_footer(f, app, p.footer);
}

/// `text` in the middle of the body.
fn render_empty(f: &mut Frame, p: &Panes, text: &str) {
    let body = p.list.union(p.detail);
    let row = Rect::new(body.x, body.y + body.height / 2, body.width, 1).intersection(body);
    f.render_widget(
        Paragraph::new(text)
            .style(dim())
            .alignment(Alignment::Center),
        row,
    );
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let answerable = app.items.iter().filter(|i| action(i).is_some()).count();
    let waiting = match app.sessions.iter().filter(|s| s.status == WAITING).count() {
        0 => String::new(),
        n => format!(" ({n} waiting)"),
    };
    let left = Line::from(vec![
        Span::styled(" agent-inbox", bold()),
        Span::styled(
            format!(
                "  {} pending · {answerable} answerable · {} sessions{waiting}",
                app.items.len(),
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
    let title = match &app.session {
        Some(session) => {
            let place = visible.first().map(|i| i.place()).unwrap_or_default();
            let short: String = session.chars().take(8).collect();
            clean(&format!(" Session {place} · {short} "))
        }
        None => " Inbox ".to_string(),
    };
    let block = Block::bordered()
        .title(title)
        .border_style(border_style(focused));
    let width = usize::from(block.inner(area).width);
    let project_width = visible
        .iter()
        .map(|i| clean(&i.place()).chars().count())
        .max()
        .unwrap_or(0)
        .min(PROJECT_MAX_WIDTH);
    let rows: Vec<ListItem> = visible
        .iter()
        .map(|i| ListItem::new(row(app, i, width, project_width)))
        .collect();
    let list = List::new(rows)
        .block(block)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state =
        ListState::default().with_selected((!visible.is_empty()).then_some(app.selected));
    f.render_stateful_widget(list, area, &mut state);
}

/// marker, badge, project, title, age right-aligned; exactly `width` chars if it fits.
fn row(app: &App, item: &Item, width: usize, project_width: usize) -> Line<'static> {
    let (badge_text, badge_style) = badge(item.kind());
    let marker = if action(item).is_some() {
        Span::styled("●", bold())
    } else {
        Span::styled("○", dim())
    };
    let project = truncate(&clean(&item.place()), project_width);
    let age = item.age(app.now);
    let head = 2 + BADGE_WIDTH + 1 + project_width + 1;
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
        Span::raw(format!("{:<w$}", project, w = project_width)),
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
    let block = Block::bordered()
        .title(" Sessions ")
        .border_style(border_style(true));
    let width = usize::from(block.inner(area).width);
    let place_width = app
        .sessions
        .iter()
        .map(|s| clean(&s.place()).chars().count())
        .max()
        .unwrap_or(0)
        .min(PROJECT_MAX_WIDTH);
    let rows: Vec<ListItem> = app
        .sessions
        .iter()
        .map(|s| ListItem::new(session_row(app, s, width, place_width)))
        .collect();
    let list = List::new(rows)
        .block(block)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(app.session_sel));
    f.render_stateful_widget(list, area, &mut state);
}

/// pending items, badge, place, what it waits for, status age right-aligned; exactly `width`
/// chars if it fits.
fn session_row(app: &App, session: &Session, width: usize, place_width: usize) -> Line<'static> {
    let (badge_text, badge_style) = status_badge(&session.status);
    let pending = app.session_items(session).len();
    let count = if pending == 0 {
        "  ".to_string()
    } else {
        format!("{:>2}", pending.min(99))
    };
    let place = truncate(&clean(&session.place()), place_width);
    let age = client::age(session.status_at, app.now);
    let head = 2 + 1 + BADGE_WIDTH + 1 + place_width + 1;
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
        Span::raw(format!("{:<w$}", place, w = place_width)),
        Span::raw(" "),
        Span::raw(waiting_for),
        Span::raw(" ".repeat(pad)),
        Span::styled(age, dim()),
    ])
}

/// The detail of `session`: its fields and its pending items.
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
    }
    lines
}

/// The detail pane of the sessions list (never focused).
fn render_session_detail(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .title(" Detail ")
        .border_style(border_style(false));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = app
        .selected_session()
        .map(|s| session_lines(app, s))
        .unwrap_or_default();
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_detail(f: &mut Frame, app: &App, area: Rect) {
    let (item, scroll) = match &app.mode {
        Mode::List | Mode::ConfirmDismissAll => (app.selected_item(), 0),
        Mode::Detail { id, scroll } => (app.find(id), *scroll),
        Mode::Password { id, .. } | Mode::Answer { id, .. } => (app.find(id), 0),
    };
    let focused = !matches!(app.mode, Mode::List | Mode::ConfirmDismissAll);
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
    let line = if matches!(app.mode, Mode::ConfirmDismissAll) {
        let ask = format!(
            "Dismiss {} items without a reply?",
            app.without_reply().len()
        );
        let mut spans = vec![
            Span::styled(
                ask,
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
        ];
        spans.extend(hint_spans(&[("y", "yes"), ("n", "no")], "  "));
        Line::from(spans)
    } else if let Some(status) = app.status() {
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
pub fn run(base: &str, runtime: Option<&Path>, sessions_dir: &Path) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, base, runtime, sessions_dir);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    base: &str,
    runtime: Option<&Path>,
    sessions_dir: &Path,
) -> io::Result<()> {
    let mut app = App::default();
    let mut refreshed: Option<Instant> = None;
    loop {
        if refreshed.is_none_or(|t| t.elapsed() >= REFRESH) {
            refresh(&mut app, base, sessions_dir);
            refreshed = Some(Instant::now());
        }
        app.now = client::now();
        let size = terminal.size()?;
        app.area = Rect::new(0, 0, size.width, size.height);
        terminal.draw(|f| render(f, &app))?;
        if event::poll(POLL)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.now = client::now(); // not the time of the draw: the guard is timed
                    let effect = app.on_key(key);
                    if perform(&mut app, effect, base, runtime, sessions_dir) {
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Reloads the sessions (they need no router) and the inbox items.
fn refresh(app: &mut App, base: &str, sessions_dir: &Path) {
    app.set_sessions(sessions::load(sessions_dir, Path::new("/proc")));
    match client::list(base) {
        Ok(items) => {
            app.offline = None;
            app.set_items(items);
        }
        Err(e) => app.offline = Some(e.to_string()),
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
    }
    refresh(app, base, sessions_dir);
    false
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

    fn item(id: &str, kind: &str, title: &str, received: f64, reply: Option<&str>) -> Item {
        let mut event = json!({"type": kind, "project": "proj", "title": title, "cwd": AGENT_CWD});
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
            "cwd": AGENT_CWD, "tool_name": tool, "tool_input": tool_input});
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
    fn empty_state_is_centred_text() {
        let s = screen(&app(vec![]));
        assert!(s.contains("No pending requests"));
        assert!(s.contains("0 pending"));
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
            (vec![sudo("s")], vec![key('S')]),
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
    fn answer_mode_ignores_other_command_and_ctrl_keys() {
        let mut a = app(vec![ask("q", Some(HOOK_REPLY))]);
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        for c in "qDyp0".chars() {
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
        assert!(screen(&a).contains("Inbox"));
        a.on_key(key('s'));
        let s = screen(&a);
        assert!(s.contains("Session infra · s-mine-0 "));
        assert!(s.contains("title-mine1") && s.contains("title-mine2"));
        assert!(!s.contains("title-other") && !s.contains("sudo ls"));
        assert!(s.contains("esc all items"));
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
        }
    }

    fn shpooled(mut s: Session, name: &str) -> Session {
        s.shpool = name.into();
        s
    }

    /// The app on the sessions view.
    fn sessions_app(items: Vec<Item>, sessions: Vec<Session>) -> App {
        let mut a = app(items);
        a.set_sessions(sessions);
        assert_eq!(a.on_key(key('S')), Effect::None);
        a
    }

    fn selected_pid(a: &App) -> Option<u32> {
        a.selected_session().map(|s| s.pid)
    }

    #[test]
    fn capital_s_toggles_the_sessions_view_both_ways() {
        let mut a = app(vec![question("a", 990.0)]);
        a.set_sessions(vec![session(1, "s-a", "idle")]);
        assert!(screen(&a).contains(" Inbox "));
        assert!(screen(&a).contains("S sessions"));
        a.on_key(key('S'));
        let s = screen(&a);
        assert!(s.contains(" Sessions ") && !s.contains(" Inbox "), "{s}");
        assert!(s.contains("S inbox"), "{s}");
        a.on_key(key('S'));
        assert!(screen(&a).contains(" Inbox "));
        a.on_key(key('S'));
        a.on_key(code(KeyCode::Esc));
        assert!(screen(&a).contains(" Inbox "));
        // the inbox keys do nothing in the sessions view, q quits from both
        a.on_key(key('S'));
        assert_eq!(a.on_key(key('d')), Effect::None);
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
        // the header counts sessions in both views
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
        a.on_key(key('S'));
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
    fn enter_shows_the_items_of_the_session_and_esc_returns() {
        let mut a = sessions_app(
            vec![
                in_session(question("a", 960.0), "s-a"),
                in_session(question("b", 970.0), "s-b"),
            ],
            vec![session(1, "s-a", "idle"), session(2, "s-b", "idle")],
        );
        a.on_key(key('j'));
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.view, View::Inbox);
        assert_eq!(a.session.as_deref(), Some("s-b"));
        assert_eq!(selected_id(&a), Some("b"));
        let s = screen(&a);
        assert!(s.contains("title-b") && !s.contains("title-a"), "{s}");
        assert!(s.contains("esc sessions"), "{s}");
        a.on_key(code(KeyCode::Esc));
        assert_eq!(a.view, View::Sessions);
        assert!(a.session.is_none() && !a.from_sessions);
        assert_eq!(selected_pid(&a), Some(2));
        // `s` leaves the filter the same way
        a.on_key(code(KeyCode::Enter));
        a.on_key(key('s'));
        assert_eq!(a.view, View::Sessions);
        // a filter entered from the inbox does not
        a.on_key(key('S'));
        a.on_key(key('s'));
        assert!(screen(&a).contains("esc all items"));
        a.on_key(code(KeyCode::Esc));
        assert_eq!(a.view, View::Inbox);
        // Esc in the sessions view goes to the inbox
        a.on_key(key('S'));
        a.on_key(code(KeyCode::Esc));
        assert_eq!(a.view, View::Inbox);
    }

    #[test]
    fn the_last_item_answered_returns_to_the_sessions() {
        let mut a = sessions_app(
            vec![in_session(permission("p", Some(HOOK_REPLY)), "s-a")],
            vec![session(1, "s-a", WAITING)],
        );
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.on_key(key('y')), allow("p"));
        assert_eq!(a.view, View::Inbox);
        // the router has dismissed it
        a.set_items(vec![]);
        assert_eq!(a.view, View::Sessions);
        assert!(a.session.is_none() && !a.from_sessions);
        assert!(screen(&a).contains(" Sessions "));
    }

    #[test]
    fn a_session_ending_under_an_open_item_stays_in_the_inbox() {
        let mut a = sessions_app(
            vec![in_session(permission("p", Some(HOOK_REPLY)), "s-a")],
            vec![session(1, "s-a", WAITING)],
        );
        a.on_key(code(KeyCode::Enter));
        a.now += 1.0;
        a.on_key(code(KeyCode::Enter));
        a.set_items(vec![]);
        assert!(a.session.is_none() && !a.from_sessions);
        assert_eq!(a.view, View::Inbox);
        assert!(matches!(a.mode, Mode::Detail { .. }));
    }

    #[test]
    fn enter_without_items_says_so() {
        let mut a = sessions_app(
            vec![in_session(question("a", 960.0), "s-other")],
            vec![session(1, "s-a", "idle")],
        );
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        assert_eq!(a.view, View::Sessions);
        assert!(a.session.is_none());
        let s = screen(&a);
        assert!(
            s.contains("no pending items") && !s.contains("a to attach"),
            "{s}"
        );
        // nothing to open: no hint either
        a.now += STATUS_TTL;
        assert!(!screen(&a).contains("enter items"));
        // attachable: the way on is named
        let mut a = sessions_app(vec![], vec![shpooled(session(1, "s-a", "idle"), "jw-1")]);
        a.on_key(code(KeyCode::Enter));
        assert!(screen(&a).contains("no pending items — a to attach"));
        // the items of an unreachable router are stale
        let mut a = sessions_app(
            vec![in_session(question("a", 960.0), "s-a")],
            vec![shpooled(session(1, "s-a", "idle"), "jw-1")],
        );
        assert!(screen(&a).contains("enter items"));
        a.offline = Some("down".into());
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.view, View::Sessions);
        assert!(screen(&a).contains("no pending items — a to attach"));
    }

    #[test]
    fn a_held_enter_opens_the_session_but_not_its_first_item() {
        let mut a = sessions_app(
            vec![in_session(question("a", 960.0), "s-a")],
            vec![session(1, "s-a", "idle")],
        );
        a.on_key(code(KeyCode::Enter));
        assert_eq!(a.view, View::Inbox);
        // the repeats are swallowed
        a.now += 0.6;
        assert_eq!(a.on_key(code(KeyCode::Enter)), Effect::None);
        assert!(matches!(a.mode, Mode::List));
        // released: the next Enter opens the item
        a.now += 1.1;
        a.on_key(code(KeyCode::Enter));
        assert!(matches!(a.mode, Mode::Detail { .. }));
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
        // this very process, without a start time: alive
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("agent-inbox-test-tui-{pid}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{pid}.json")),
            format!(r#"{{"pid":{pid},"sessionId":"s-1","cwd":"/x","startedAt":1000}}"#),
        )
        .unwrap();
        let mut a = app(vec![]);
        refresh(&mut a, "127.0.0.1:1", &dir);
        assert!(a.offline.is_some());
        assert_eq!(a.sessions.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
