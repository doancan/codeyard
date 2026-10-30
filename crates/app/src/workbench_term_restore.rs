//! Workbench: terminals outside the panel across restarts. Which editor group held which terminal (in order) and where
//! detached terminal windows were is saved with the session, with each terminal's token. On the next start a terminal
//! still running in the window's session is taken back into its place, as the panel's are (`workbench_term_daemon.rs`);
//! otherwise a NEW shell takes the place, with the title, colour and folder of the old one and a dimmed notice. Nothing
//! of the old session is run again: no command, no input. The saved value is versioned and bounded; a damaged one
//! restores nothing and never blocks startup.

use super::*;
use gpui::{Bounds, Pixels};

use crate::terminal::TerminalView;

/// State key of the saved arrangement, per workspace.
const SAVED_KEY: &str = "moved_terminals";
const VERSION: u8 = 1;
const MAX_TEXT: usize = 64 * 1024;
const MAX_TITLE: usize = 256;
const MAX_CWD: usize = 4096;
/// The smallest restored window.
const MIN_W: f32 = 200.;
const MIN_H: f32 = 150.;

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedTerminals {
    version: u8,
    terminals: Vec<SavedTerminal>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedTerminal {
    place: Place,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    /// The token to take it back from the window's session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<crate::terminal_session::SavedPty>,
}

/// Where a terminal was: an editor group (by id), or a window of its own (x, y, width, height, when known).
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Place {
    Group(crate::editor_layout::GroupId),
    Window(Option<[f32; 4]>),
}

impl SavedTerminals {
    /// The saved arrangement, if it is one this version wrote and within bounds; anything else is ignored whole.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        if text.len() > MAX_TEXT {
            return None;
        }
        let saved: Self = serde_json::from_str(text).ok()?;
        if saved.version != VERSION || saved.terminals.len() > MAX_TERMINALS {
            return None;
        }
        let fits = |s: &Option<String>, max: usize| s.as_ref().is_none_or(|s| s.len() <= max && !s.contains('\0'));
        let valid = saved
            .terminals
            .iter()
            .all(|t| fits(&t.title, MAX_TITLE) && fits(&t.cwd, MAX_CWD) && t.color.as_deref().is_none_or(|c| crate::terminal_profiles::COLORS.contains(&c)));
        let hex = |s: &str| s.len() <= 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        let valid = valid && saved.terminals.iter().filter_map(|t| t.token.as_ref()).all(|k| hex(&k.instance) && hex(&k.secret));
        valid.then_some(saved)
    }

    /// The saved tokens, by position.
    pub(crate) fn tokens(&self) -> Vec<(usize, crate::terminal_session::SavedPty)> {
        self.terminals.iter().enumerate().filter_map(|(ix, t)| Some((ix, t.token.clone()?))).collect()
    }

    #[cfg(test)]
    pub(crate) fn places(&self) -> Vec<Place> {
        self.terminals.iter().map(|t| t.place).collect()
    }
}

/// Where a restored terminal window opens: on the display that shows most of its saved bounds, else (its display was
/// removed, the coordinates are off every screen) on the first display, the primary one. The size fits that display
/// and the window lies wholly on it. Missing or non-finite bounds give the default size, centred. `None` when no
/// display is known.
pub(crate) fn place_window(saved: Option<[f32; 4]>, displays: &[Bounds<Pixels>]) -> Option<Bounds<Pixels>> {
    let first = *displays.first()?;
    let saved = saved
        .filter(|b| b.iter().all(|v| v.is_finite()) && b[2] > 0. && b[3] > 0.)
        .map(|[x, y, w, h]| Bounds::new(gpui::point(px(x), px(y)), gpui::size(px(w), px(h))));
    let area = |b: Bounds<Pixels>| f32::from(b.size.width) * f32::from(b.size.height);
    let shown = |d: &Bounds<Pixels>| saved.map_or(0., |s| area(d.intersect(&s)));
    let display = displays.iter().copied().filter(|d| shown(d) > 0.).max_by(|a, b| shown(a).total_cmp(&shown(b))).unwrap_or(first);
    let wanted = saved.unwrap_or_else(|| {
        let size = views_glue::DETACHED_SIZE;
        Bounds::new(display.origin + gpui::point((display.size.width - size.width) / 2., (display.size.height - size.height) / 2.), size)
    });
    let fit = |v: Pixels, min: f32, max: Pixels| v.max(px(min)).min(max);
    let size = gpui::size(fit(wanted.size.width, MIN_W, display.size.width), fit(wanted.size.height, MIN_H, display.size.height));
    let clamp = |v: Pixels, lo: Pixels, room: Pixels| v.max(lo).min(lo + room);
    let origin = gpui::point(
        clamp(wanted.origin.x, display.origin.x, display.size.width - size.width),
        clamp(wanted.origin.y, display.origin.y, display.size.height - size.height),
    );
    Some(Bounds::new(origin, size))
}

impl Workbench {
    /// Save which group holds which terminal and where the detached terminal windows are (with the session).
    pub(in crate::workbench) fn persist_moved_terminals(&self, cx: &mut App) {
        let (Some(ws), Some(s)) = (self.persist.workspace_id, &self.persist.state) else { return };
        let mut terminals = Vec::new();
        // What `parse` would refuse is left out, so one long title or folder does not lose the whole arrangement.
        let fits = |s: Option<String>, max: usize| s.filter(|s| s.len() <= max && !s.contains('\0'));
        let mut entry = |term: &Entity<TerminalView>, place: Place, cx: &App| {
            let t = term.read(cx);
            // A task's output has no shell to start again.
            if !t.has_shell() {
                return;
            }
            let color = t.color.clone().filter(|c| crate::terminal_profiles::COLORS.contains(&c.as_str()));
            let cwd = fits(t.live_cwd().or_else(|| t.start_dir()), MAX_CWD);
            terminals.push(SavedTerminal { place, title: fits(t.fixed_title(), MAX_TITLE), color, cwd, token: t.token().cloned() });
        };
        for id in self.tabs.layout.groups() {
            for term in self.tabs.group(id).map(|g| g.terminals.as_slice()).unwrap_or_default() {
                entry(term, Place::Group(id), cx);
            }
        }
        for d in &self.chrome.detached {
            let at = d.window.update(cx, |_, window, _| window.bounds()).ok();
            let at = at.map(|b| [b.origin.x, b.origin.y, b.size.width, b.size.height].map(f32::from));
            entry(&d.term, Place::Window(at), cx);
        }
        terminals.truncate(MAX_TERMINALS);
        if let Ok(json) = serde_json::to_string(&SavedTerminals { version: VERSION, terminals }) {
            let _ = s.set(&format!("{SAVED_KEY}:{ws}"), &json);
        }
    }

    /// The arrangement saved for workspace `ws`; `None` when there is none or it is damaged.
    pub(in crate::workbench) fn saved_moved_terminals(&self, ws: i64) -> Option<SavedTerminals> {
        let text = self.persist.state.as_ref()?.get(&format!("{SAVED_KEY}:{ws}"))?;
        SavedTerminals::parse(&text)
    }

    /// Start a new shell in each saved place. Only when no terminal is outside the panel (a start, not a folder switch
    /// whose terminals are still running; the panel's own terminals are restored separately and may already be back);
    /// a group that no longer exists gives its terminals to the focused group, and a window that cannot open gives its
    /// terminal to the panel.
    pub(in crate::workbench) fn restore_moved_terminals(&mut self, saved: Option<SavedTerminals>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(saved) = saved else { return };
        if !self.chrome.detached.is_empty() || self.editor_terminal_count() > 0 {
            return;
        }
        // The window's session is still being taken back: these are placed once it is (some may be taken back too).
        if self.persist.term_restoring {
            self.persist.pending_moved = Some(saved);
            return;
        }
        let back = std::mem::take(&mut self.persist.moved_back);
        self.place_moved_terminals(saved, back, window, cx);
    }

    /// Put each saved terminal in its place: the ones in `back` (by position) as they are, the others as new shells.
    pub(in crate::workbench) fn place_moved_terminals(
        &mut self,
        saved: SavedTerminals,
        mut back: Vec<(usize, host::Pty, u64)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.chrome.detached.is_empty() || self.editor_terminal_count() > 0 {
            return;
        }
        let root = self.active_root(cx);
        let local = !self.remote.host.is_remote();
        let displays: Vec<Bounds<Pixels>> = cx.primary_display().into_iter().chain(cx.displays()).map(|d| d.bounds()).collect();
        let mut displays_unique = Vec::new();
        for d in displays {
            if !displays_unique.contains(&d) {
                displays_unique.push(d);
            }
        }
        for (ix, saved) in saved.terminals.into_iter().enumerate() {
            if let Some(at) = back.iter().position(|(i, _, _)| *i == ix) {
                let (_, pty, dropped) = back.swap_remove(at);
                let shown = crate::terminal_session::SavedTerminal {
                    cwd: saved.cwd.clone(),
                    color: saved.color.clone(),
                    title: saved.title.clone(),
                    token: saved.token.clone(),
                    ..Default::default()
                };
                let term = self.reattached_terminal(pty, &shown, dropped, window, cx);
                self.put_moved_terminal(term, saved.place, &displays_unique, window, cx);
                continue;
            }
            // The folder it was in, if it is still there (on a remote host: if it is inside the open folder).
            let cwd = saved
                .cwd
                .filter(|cwd| if local { std::path::Path::new(cwd).is_dir() } else { root.as_deref().is_some_and(|r| cwd.starts_with(r)) })
                .or_else(|| root.clone());
            let mut launch = self.default_launch(cx);
            if saved.color.is_some() {
                launch.color = saved.color.clone();
            }
            let Some(term) = self.spawn_terminal(cwd, launch, window, cx) else { break };
            term.update(cx, |t, cx| {
                if let Some(title) = saved.title {
                    t.set_fixed_title(title);
                }
                t.show_restored_notice(cx);
            });
            self.put_moved_terminal(term, saved.place, &displays_unique, window, cx);
        }
        self.refresh_active_flags(cx);
        // Restoring the groups saved an arrangement without these; save it again with them.
        self.persist_moved_terminals(cx);
        cx.notify();
    }

    /// Put a restored terminal in its saved place: its group (else the focused one), or a window of its own on a
    /// display (else the panel).
    fn put_moved_terminal(&mut self, term: Entity<TerminalView>, place: Place, displays: &[Bounds<Pixels>], window: &mut Window, cx: &mut Context<Self>) {
        match place {
            Place::Group(id) => {
                let id = if self.tabs.group(id).is_some() { id } else { self.tabs.layout.focused() };
                let group = self.tabs.group_mut(id).expect("layout group has state");
                group.terminals.push(term.clone());
                if group.page.is_none() && group.editors.is_empty() {
                    group.page = Some(Page::Terminal(term));
                    group.page_active = true;
                }
            }
            Place::Window(at) => {
                let bounds = place_window(at, displays)
                    .unwrap_or_else(|| Bounds::new(window.bounds().origin + gpui::point(px(40.), px(40.)), views_glue::DETACHED_SIZE));
                match self.open_detached(term.clone(), bounds, cx) {
                    Ok(handle) => self.chrome.detached.push(views_glue::Detached { term, window: handle }),
                    Err(_) => {
                        self.terminals.push(term);
                        self.active_terminal = self.terminals.len() - 1;
                        self.chrome.bottom = Bottom::Terminal;
                        self.chrome.panel_open = true;
                    }
                }
            }
        }
    }
}
