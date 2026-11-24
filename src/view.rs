use crate::{
    display,
    field::TextInput,
    theme::{Theme, skins},
};
use anyhow::{Context as _, Result, ensure};
use bastion_bridge_gpui::{mcp::Endpoint, store};
use gpui::prelude::*;
use gpui::*;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    ops::Range,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::selection::{self, BufferCell, Cell, Unit};
fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn state_label(v: &Value) -> &'static str {
    match str_at(v, "state") {
        "disconnected" => "已断开",
        _ if v["owner"] == "human" => "你在操作",
        "ready" => "就绪",
        "busy" => "执行中",
        "connecting" => "连接中",
        "synchronizing" => "同步中",
        _ => "已断开",
    }
}
struct HoverTip {
    text: SharedString,
    theme: Theme,
}

impl Render for HoverTip {
    fn render(&mut self, w: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(520.).min(w.viewport_size().width))
            .max_h(w.viewport_size().height)
            .overflow_hidden()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(4.))
            .bg(rgb(self.theme.color("panel")))
            .border_1()
            .border_color(rgb(self.theme.color("border-strong")))
            .text_color(rgb(self.theme.color("text")))
            .text_size(px(12.))
            .child(self.text.clone())
    }
}

fn profile_label_height() -> Pixels {
    px(if cfg!(windows) { 15.33 } else { 17. })
}

fn profile_picker_height() -> Pixels {
    px(if cfg!(windows) { 36. } else { 33. })
}

fn picker_arrow(theme: Theme) -> AnyElement {
    #[cfg(target_os = "macos")]
    {
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let left = bounds.left();
                let top = bounds.top();
                let mut path = PathBuilder::stroke(px(1.25));
                for (edge, tip) in [(4.5, 1.5), (7.5, 10.5)] {
                    path.move_to(point(left + px(1.), top + px(edge)));
                    path.line_to(point(left + px(4.), top + px(tip)));
                    path.line_to(point(left + px(7.), top + px(edge)));
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, rgb(theme.color("text")));
                }
            },
        )
        .w(px(8.))
        .h(px(12.))
        .flex_none()
        .into_any_element()
    }
    #[cfg(not(target_os = "macos"))]
    {
        div().flex_none().text_color(rgb(theme.color("text"))).child("▾").into_any_element()
    }
}

fn focus_outline(style: StyleRefinement, theme: Theme) -> StyleRefinement {
    StyleRefinement {
        outline: Some(Outline {
            width: px(2.),
            offset: px(2.),
            color: rgb(theme.color("green")).into(),
        }),
        ..style
    }
}

fn outline(theme: Theme) -> Vec<BoxShadow> {
    [("green", 4.), ("panel", 2.)]
        .into_iter()
        .map(|(color, spread)| BoxShadow {
            color: rgb(theme.color(color)).into(),
            offset: Point::default(),
            blur_radius: px(0.),
            spread_radius: px(spread),
        })
        .collect()
}

fn hover_tip(
    text: impl Into<SharedString>,
    theme: Theme,
) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let text = text.into();
    move |_, cx| {
        cx.new(|_| HoverTip {
            text: text.clone(),
            theme,
        })
        .into()
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Dialog {
    None,
    Profiles,
    Skins,
    Mcp,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum TerminalFullscreen {
    Off,
    Entering,
    Entered,
    AlreadyFullscreen,
    CancelOnEntry,
    Leaving,
}

impl TerminalFullscreen {
    fn active(self) -> bool {
        matches!(
            self,
            Self::Entering | Self::Entered | Self::AlreadyFullscreen
        )
    }

    fn toggle(&mut self, window: &mut Window) {
        *self = match *self {
            Self::Off if window.is_fullscreen() => Self::AlreadyFullscreen,
            Self::Off => {
                window.toggle_fullscreen();
                Self::Entering
            }
            Self::AlreadyFullscreen => Self::Off,
            Self::Entering if !window.is_fullscreen() => Self::CancelOnEntry,
            Self::Entering | Self::Entered if window.is_fullscreen() => {
                window.toggle_fullscreen();
                Self::Leaving
            }
            Self::Entered | Self::Entering => Self::Off,
            Self::CancelOnEntry | Self::Leaving => return,
        };
    }

    fn sync(&mut self, window: &mut Window) {
        // Native fullscreen transitions finish asynchronously on macOS.
        *self = match (*self, window.is_fullscreen()) {
            (Self::Entering, true) => Self::Entered,
            (Self::CancelOnEntry, true) => {
                window.toggle_fullscreen();
                Self::Leaving
            }
            (Self::Entered | Self::AlreadyFullscreen | Self::Leaving, false) => Self::Off,
            _ => return,
        };
    }
}

#[derive(Clone)]
enum Confirm {
    Close(String, String),
    Delete(String, String),
}
struct Call {
    client: Endpoint,
    name: &'static str,
    args: Value,
}
struct Reply {
    name: &'static str,
    target: String,
    result: Result<Value>,
}
impl Call {
    fn run(self, replies: &mpsc::Sender<Reply>) {
        let result = self.client.action(self.name, &self.args);
        let _ = replies.send(Reply {
            name: self.name,
            target: str_at(&self.args, "session_id").into(),
            result,
        });
    }
}
struct ClipboardPopup {
    position: Point<Pixels>,
    origin: FocusHandle,
    enabled: [bool; 2],
    highlighted: Option<usize>,
}

#[derive(Clone, Copy, PartialEq)]
struct TerminalFit {
    width: f32,
    height: f32,
    advance: f32,
    line_height: f32,
    history_open: bool,
}

impl TerminalFit {
    // xterm's DOM renderer measures a whole canvas in CSS pixels, then divides
    // its rounded extent by the current grid size before proposing the next grid.
    fn cell(measure: f32, count: u16) -> f64 {
        let count = f64::from(count);
        (f64::from(measure) * count).round() / count
    }

    fn grid(self, (cols, rows): (u16, u16)) -> (u16, u16) {
        let cols = (f64::from(self.width.floor()) / Self::cell(self.advance, cols)).floor();
        let rows = (f64::from(self.height.floor()) / Self::cell(self.line_height, rows)).floor();
        (cols.clamp(20., 500.) as u16, rows.clamp(5., 200.) as u16)
    }
}

#[cfg(test)]
mod fit_tests {
    use super::TerminalFit;

    #[test]
    fn narrow_window_with_history_fits_the_last_column() {
        let fit = TerminalFit {
            width: 352.,
            height: 678.,
            advance: 7.8266602,
            line_height: 19.5,
            history_open: true,
        };
        let first = fit.grid((74, 34));
        assert_eq!(first, (44, 34));
        assert_eq!(fit.grid(first), (45, 34));
        assert_eq!(fit.grid((45, 34)), (45, 34));
    }
}

pub struct View {
    #[cfg(target_os = "macos")]
    disabled_colors: crate::mac_window::DisabledButtonColors,
    clipboard_popup: Option<ClipboardPopup>,
    popup_focus: FocusHandle,
    dir: PathBuf,
    endpoint: Endpoint,
    requests: mpsc::Sender<Call>,
    replies: mpsc::Receiver<Reply>,
    selected: String,
    terminal_generation: u64,
    state: Value,
    sessions: Vec<Value>,
    jobs: Vec<Value>,
    history_previews: Option<(Pixels, Vec<(SharedString, SharedString)>)>,
    history_text: crate::history_text::HistoryText,
    history_focus: FocusHandle,
    offset: u64,
    connection: Value,
    parser: vt100::Parser,
    focus: FocusHandle,
    page_focus: FocusHandle,
    keyboard_focus: bool,
    dialog_origin: Option<FocusHandle>,
    confirm_origin: Option<FocusHandle>,
    theme: Theme,
    history: bool,
    history_hovered: bool,
    fullscreen: TerminalFullscreen,
    consumed_escape: bool,
    dialog: Dialog,
    confirm: Option<Confirm>,
    confirm_buttons: [FocusHandle; 2],
    modal_dismiss: FocusHandle,
    dialog_scroll: ScrollHandle,
    validation: Option<crate::form_validation::Hint>,
    target_changes: Vec<Subscription>,
    profiles: Vec<Value>,
    profile: Value,
    fields: Vec<Entity<TextInput>>,
    chooser: bool,
    save: bool,
    remember: bool,
    desktop: Value,
    pi: bool,
    chooser_focus: FocusHandle,
    highlighted_pi: bool,
    highlighted_profile: usize,
    profile_scroll: ScrollHandle,
    profile_prefix: String,
    last_profile_key: Instant,
    copied_config: bool,
    config_area: Entity<crate::config_text::ConfigText>,
    note: String,
    geometry: (u16, u16),
    last_fit: Option<TerminalFit>,
    cell_width: f32,
    row_height: f32,
    sent_size: Option<(String, u16, u16)>,
    selection: Option<(BufferCell, BufferCell)>,
    initial_selection: Option<(BufferCell, BufferCell)>,
    column_selection: bool,
    drag: Option<(BufferCell, Unit)>,
    pressed_link: Option<display::LinkSpan>,
    drag_next_scroll: Instant,
    cursor_click_started: Option<Instant>,
    wheel_remainder: f32,
    scrollbar: crate::scrollbar::Scrollbar,
    composition: crate::terminal_ime::Composition,
    busy: bool,
    reconnecting: HashSet<String>,
    blink: bool,
    blink_started: Instant,
    terminal_focused: bool,
    reported_focus: Option<(u64, bool)>,
    mouse_buttons: u8,
    last_mouse_move: Option<(vt100::MouseProtocolEncoding, u8, u32, u32)>,
    cursor_initialized: bool,
    animate_cursor: bool,
}
impl View {
    pub fn new(dir: PathBuf, endpoint: Endpoint, w: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let page_focus = cx.focus_handle();
        w.focus(&page_focus);
        cx.on_focus(&focus, w, |v, w, cx| {
            v.focus_terminal();
            v.sync_focus(w);
            cx.notify();
        })
        .detach();
        cx.on_blur(&focus, w, |v, w, cx| {
            v.terminal_focused = false;
            v.sync_focus(w);
            cx.notify();
        })
        .detach();
        cx.observe_window_activation(w, |v, w, cx| {
            if !w.is_window_active() {
                v.mouse_buttons = 0;
                v.last_mouse_move = None;
                v.dismiss_clipboard(w);
                v.scrollbar.drag = None;
                v.drag = None;
            }
            v.sync_focus(w);
            v.restart_blink();
            cx.notify();
        })
        .detach();
        w.on_window_should_close(cx, |w, _| {
            w.minimize_window();
            false
        });
        let (requests, commands) = mpsc::channel::<Call>();
        let (results, replies) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(call) = commands.recv() {
                // A handshake may wait for the remote server. Keep the FIFO for
                // typing, resizing and handing back an already connected shell.
                if call.name == "reconnect" {
                    let results = results.clone();
                    std::thread::spawn(move || call.run(&results));
                } else {
                    call.run(&results);
                }
            }
        });
        let theme = Theme::saved(&dir);
        let desktop = endpoint
            .action("desktop-info", &json!({}))
            .unwrap_or_default();
        let window = w.window_handle();
        cx.spawn(async move |this, cx| {
            loop {
                let Ok((client, generation, args, dir)) = this.update(cx, |v, cx| {
                    if std::fs::remove_file(v.dir.join("show-window")).is_ok() {
                        crate::show(cx);
                    }
                    if v.validation.as_ref().is_some_and(|hint| Instant::now() >= hint.expires) {
                        v.validation = None;
                        cx.notify();
                    }
                    while let Ok(reply) = v.replies.try_recv() {
                        v.receive(reply, window, cx);
                        cx.notify();
                    }
                    (
                        v.endpoint.clone(),
                        v.terminal_generation,
                        json!({"session_id":v.selected,"offset":v.offset}),
                        v.dir.clone(),
                    )
                }) else {
                    break;
                };
                let (packet, fresh) = cx
                    .background_executor()
                    .spawn(async move {
                        let result = client.action("snapshot", &args);
                        let fresh = if result.is_err() {
                            Endpoint::find(&dir).filter(|e| e != &client)
                        } else {
                            None
                        };
                        (result, fresh)
                    })
                    .await;
                let delay = if packet.is_err() { 500 } else { 70 };
                if this
                    .update(cx, |v, cx| {
                        // A tab can be reselected while its old incremental read is in flight.
                        if v.terminal_generation != generation {
                            return;
                        }
                        if let Some(e) = fresh {
                            v.endpoint = e;
                            let id = v.selected.clone();
                            v.select(&id);
                            cx.notify();
                            return;
                        }
                        match packet {
                            Ok(value) => {
                                if v.state["state"] == "unavailable" {
                                    v.note.clear();
                                }
                                v.update_snapshot(value, cx);
                            }
                            Err(e) => {
                                v.state = json!({"state":"unavailable","owner":"agent"});
                                let note = format!("后台连接中断 · {e}");
                                if v.note != note {
                                    v.note = note;
                                    cx.notify();
                                }
                            }
                        }
                        if v.parser.expire_frame() {
                            cx.notify();
                        }
                        if v.animate_cursor {
                            let visible = (v.blink_started.elapsed().as_millis() / 500) % 2 == 0;
                            if v.blink != visible {
                                v.blink = visible;
                                cx.notify();
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
                Timer::after(Duration::from_millis(delay)).await;
            }
        })
        .detach();
        let config_area = cx.new(crate::config_text::ConfigText::new);
        cx.observe(&config_area, |_, _, cx| cx.notify()).detach();
        Self {
            dir,
            #[cfg(target_os = "macos")]
            disabled_colors: Default::default(),
            endpoint,
            requests,
            replies,
            selected: String::new(),
            terminal_generation: 0,
            state: Value::Null,
            sessions: vec![],
            jobs: vec![],
            history_previews: None,
            history_text: Default::default(),
            history_focus: cx.focus_handle(),
            offset: 0,
            connection: Value::Null,
            parser: theme.viewport(40, 160),
            focus,
            keyboard_focus: false,
            page_focus,
            clipboard_popup: None,
            popup_focus: cx.focus_handle(),
            dialog_origin: None,
            confirm_origin: None,
            theme,
            history: false,
            history_hovered: false,
            fullscreen: TerminalFullscreen::Off,
            consumed_escape: false,
            dialog: Dialog::None,
            confirm: None,
            confirm_buttons: [cx.focus_handle(), cx.focus_handle()],
            modal_dismiss: cx.focus_handle().tab_stop(true),
            dialog_scroll: ScrollHandle::new(),
            validation: None,
            target_changes: Vec::new(),
            profiles: vec![],
            profile: Value::Null,
            fields: vec![],
            chooser: false,
            save: true,
            remember: true,
            desktop,
            pi: false,
            chooser_focus: cx.focus_handle().tab_stop(true),
            highlighted_pi: false,
            highlighted_profile: 0,
            profile_scroll: ScrollHandle::new(),
            profile_prefix: String::new(),
            last_profile_key: Instant::now(),
            copied_config: false,
            config_area,
            note: String::new(),
            geometry: (160, 40),
            last_fit: None,
            cell_width: 7.83,
            row_height: 19.5,
            sent_size: None,
            selection: None,
            initial_selection: None,
            column_selection: false,
            drag: None,
            pressed_link: None,
            drag_next_scroll: Instant::now(),
            cursor_click_started: None,
            wheel_remainder: 0.,
            scrollbar: Default::default(),
            composition: Default::default(),
            busy: false,
            reconnecting: HashSet::new(),
            blink: true,
            blink_started: Instant::now(),
            terminal_focused: false,
            reported_focus: None,
            mouse_buttons: 0,
            last_mouse_move: None,
            cursor_initialized: false,
            animate_cursor: false,
        }
    }
    fn restart_blink(&mut self) {
        self.blink_started = Instant::now();
        self.blink = true;
    }

    fn focus_terminal(&mut self) {
        self.history_text.clear_selection();
        self.terminal_focused = true;
        self.cursor_initialized = true;
        self.restart_blink();
    }

    fn sync_focus(&mut self, w: &Window) {
        if !self.manual() {
            self.reported_focus = None;
            self.mouse_buttons = 0;
            self.last_mouse_move = None;
            return;
        }
        let focus = self.parser.screen().focus_request().map(|request| {
            (
                request,
                self.terminal_focused && self.focus.is_focused(w) && w.is_window_active(),
            )
        });
        if focus != self.reported_focus {
            self.reported_focus = focus;
            if let Some((_, focused)) = focus {
                let data = if focused { "\x1b[I" } else { "\x1b[O" };
                self.queue("input", json!({"session_id":self.selected, "data":data}));
            }
        }
    }

    fn select(&mut self, id: &str) {
        self.last_fit = None;
        self.reported_focus = None;
        self.mouse_buttons = 0;
        self.last_mouse_move = None;
        self.terminal_generation = self.terminal_generation.wrapping_add(1);
        self.clipboard_popup = None;
        self.selected = id.into();
        self.state = self
            .sessions
            .iter()
            .find(|s| s["session_id"] == id)
            .cloned()
            .unwrap_or(Value::Null);
        self.connection = Value::Null;
        self.offset = 0;
        self.jobs.clear();
        self.history_previews = None;
        self.history_text = Default::default();
        self.clear_selection();
        self.composition.finish();
        self.note.clear();
        self.sent_size = None;
        self.wheel_remainder = 0.;
        self.scrollbar = Default::default();
        self.drag = None;
        self.pressed_link = None;
        self.parser = self.theme.viewport(self.geometry.1, self.geometry.0);
        self.terminal_focused = false;
        self.cursor_initialized = false;
    }
    fn update_snapshot(&mut self, mut v: Value, cx: &mut Context<Self>) {
        let sessions = items(&v["sessions"]).to_vec();
        let jobs = items(&v["jobs"]).to_vec();
        let new_stream = v["truncated"] == true;
        if new_stream || v["connection_id"] != self.connection {
            self.cursor_click_started = None;
            self.reported_focus = None;
            self.mouse_buttons = 0;
            self.last_mouse_move = None;
            self.pressed_link = None;
        }
        if v["connection_id"] != self.connection {
            self.sent_size = None;
        }
        let data = str_at(&v, "data");
        let has_data = !data.is_empty();
        if new_stream {
            self.wheel_remainder = 0.;
            self.scrollbar = Default::default();
            self.drag = None;
            self.parser = self.theme.viewport(self.geometry.1, self.geometry.0);
            self.sent_size = None;
            self.clear_selection();
        }
        if v["state"] != "manual" {
            self.cursor_click_started = None;
            self.composition.finish();
        }
        if has_data {
            let (epoch, origin) = self.parser.screen().history_origin();
            self.parser.process(data.as_bytes());
            let (next_epoch, next_origin) = self.parser.screen().history_origin();
            if epoch != next_epoch || origin != next_origin {
                self.pressed_link = None;
            }
            if epoch != next_epoch {
                self.clear_selection();
            } else if let Some(removed) = next_origin.checked_sub(origin).filter(|n| *n > 0) {
                self.selection = self
                    .selection
                    .map(|(a, b)| (selection::trim(a, removed), selection::trim(b, removed)))
                    .filter(|(a, b)| a != b);
                self.initial_selection = self
                    .initial_selection
                    .map(|(a, b)| (selection::trim(a, removed), selection::trim(b, removed)))
                    .filter(|(a, b)| a != b);
                self.drag = self
                    .drag
                    .map(|(a, unit)| (selection::trim(a, removed), unit));
            }
            self.restart_blink();
        }
        self.offset = v["offset"].as_u64().unwrap_or(0);
        if self.connection != v["connection_id"] && self.dialog == Dialog::None {
            self.clipboard_popup = None;
        }
        self.connection = v["connection_id"].clone();
        for k in ["sessions", "jobs", "offset", "truncated", "data"] {
            v.as_object_mut().unwrap().remove(k);
        }
        if has_data
            || self.state != v
            || self.sessions != sessions
            || self.jobs != jobs
            || new_stream
        {
            self.state = v;
            self.sessions = sessions;
            if self.jobs != jobs {
                self.jobs = jobs;
                self.history_previews = None;
                self.history_text.clear_selection();
            }
            if !self
                .sessions
                .iter()
                .any(|s| s["session_id"] == self.selected)
            {
                let id = self
                    .sessions
                    .first()
                    .map(|s| str_at(s, "session_id"))
                    .unwrap_or("")
                    .to_owned();
                if id != self.selected {
                    self.select(&id);
                }
            }
            cx.notify();
        }
    }
    fn queue(&self, name: &'static str, args: Value) {
        let _ = self.requests.send(Call {
            client: self.endpoint.clone(),
            name,
            args,
        });
    }
    fn receive(&mut self, reply: Reply, window: AnyWindowHandle, cx: &mut Context<Self>) {
        if reply.name == "reconnect" {
            self.reconnecting.remove(&reply.target);
        } else if matches!(
            reply.name,
            "connect" | "close-session" | "takeover" | "release"
        ) {
            self.busy = false;
        }
        match reply.result {
            Err(e) if reply.name == "reconnect" => {
                let name = self
                    .sessions
                    .iter()
                    .find(|s| s["session_id"] == reply.target)
                    .map(|s| str_at(s, "name"))
                    .unwrap_or(&reply.target);
                self.note = format!("“{name}”重连失败：{e}");
            }
            Err(e) => self.note = e.to_string(),
            Ok(v) => match reply.name {
                "connect" => {
                    self.close_dialog();
                    self.select(str_at(&v, "session_id"));
                    let origin = self
                        .dialog_origin
                        .take()
                        .unwrap_or_else(|| self.page_focus.clone());
                    let _ = window.update(cx, |_, w, _| w.focus(&origin));
                }
                "reconnect" if v["needs_credentials"] == true => {
                    if self.dialog != Dialog::None || self.confirm.is_some() {
                        self.note = format!(
                            "“{}”需要密码，请先完成当前操作，再点此标签重连。",
                            str_at(&v, "name")
                        );
                        return;
                    }
                    self.dialog_origin = window.update(cx, |_, w, cx| w.focused(cx)).ok().flatten();
                    self.open_profiles(cx);
                    if self.dialog != Dialog::Profiles {
                        return;
                    }
                    let p = self
                        .profiles
                        .iter()
                        .find(|p| {
                            p["id"] == v["profile_id"]
                                && ["host", "port", "user"]
                                    .iter()
                                    .all(|k| p[k] == v["target"][k])
                        })
                        .cloned()
                        .unwrap_or(Value::Null);
                    let mut f = v["target"].clone();
                    f["name"] = v["name"].clone();
                    self.fill(p, f, cx);
                    self.dialog_scroll.scroll_to_bottom();
                    let focus = self.fields[4].read(cx).focus_handle(cx);
                    let _ = window.update(cx, |_, w, _| w.focus(&focus));
                    self.note =
                        "请补充密码以重新连接原主机。重连会创建新 shell，不会重放旧命令。".into();
                }
                "reconnect" => {
                    if reply.target == self.selected {
                        self.select(&reply.target);
                        self.note = "SSH 已重新连接；使用新 shell，旧命令不会重放。".into();
                    }
                }
                "close-session" => {
                    if self.selected == reply.target {
                        self.select("");
                    }
                }
                "takeover" | "release" if self.selected == reply.target => {
                    if let Some(fit) = self.last_fit {
                        self.apply_grid(fit, fit.grid(self.geometry));
                    }
                }
                _ => {}
            },
        }
    }
    fn close_dialog(&mut self) {
        self.dialog = Dialog::None;
        self.validation = None;
        self.target_changes.clear();
        self.fields.clear();
        self.profile = Value::Null;
        self.chooser = false;
        self.note.clear();
    }
    fn fill(&mut self, profile: Value, values: Value, cx: &mut Context<Self>) {
        self.validation = None;
        let reset_history = self.profile["id"] != profile["id"];
        for (i, k) in ["name", "host", "port", "user", "password"]
            .into_iter()
            .enumerate()
        {
            let value = if k == "port" {
                values[k].as_u64().unwrap_or(22).to_string()
            } else if k == "password" {
                String::new()
            } else {
                str_at(&values, k).into()
            };
            let hint = match k {
                "name" => "例如：测试服务器",
                "password" if profile["has_password"] == true => {
                    "已保存，留空沿用；输入新密码可更新"
                }
                "password" => "输入 SSH 密码",
                _ => "",
            };
            if let Some(field) = self.fields.get(i) {
                field.update(cx, |field, cx| {
                    field.load_value(value, reset_history || k == "password", cx);
                    field.set_placeholder(hint, cx);
                });
            } else {
                self.fields
                    .push(cx.new(|cx| TextInput::new(k, value, hint, cx)));
            }
        }
        if self.target_changes.is_empty() {
            self.target_changes = self.fields[..]
                .iter()
                .map(|field| {
                    cx.observe(field, |v, _, cx| {
                        let hint = if v.reusable_password(cx) {
                            "已保存，留空沿用；输入新密码可更新"
                        } else {
                            "输入 SSH 密码"
                        };
                        v.fields[4].update(cx, |field, cx| field.set_placeholder(hint, cx));
                        cx.notify();
                    })
                })
                .collect();
        }
        self.remember = profile.is_null() || profile["has_password"] == true;
        self.profile = profile;
        self.highlighted_profile = self.profile_index();
        self.save = true;
        self.chooser = false;
    }
    fn matches_profile_target(&self, cx: &Context<Self>) -> bool {
        self.profile["id"].is_string()
            && self.fields[1].read(cx).content.trim() == str_at(&self.profile, "host")
            && self.fields[2].read(cx).content.trim() == self.profile["port"].to_string()
            && self.fields[3].read(cx).content.trim() == str_at(&self.profile, "user")
    }
    fn reusable_password(&self, cx: &Context<Self>) -> bool {
        self.profile["has_password"] == true && self.matches_profile_target(cx)
    }
    fn commit_profile(&mut self, cx: &mut Context<Self>) {
        let profile = self
            .highlighted_profile
            .checked_sub(1)
            .and_then(|i| self.profiles.get(i))
            .cloned()
            .unwrap_or(Value::Null);
        if self.profile["id"] != profile["id"] {
            self.note.clear();
            self.fill(profile.clone(), profile, cx);
        }
        self.chooser = false;
    }

    fn profile_index(&self) -> usize {
        self.profiles
            .iter()
            .position(|p| p["id"] == self.profile["id"])
            .map_or(0, |i| i + 1)
    }

    fn open_profile_menu(&mut self) {
        self.highlighted_profile = self.profile_index();
        self.profile_prefix.clear();
        self.chooser = true;
        self.profile_scroll.scroll_to_item(self.highlighted_profile);
    }

    #[cfg(target_os = "macos")]
    fn show_native_picker(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let dialog = self.dialog;
        let (titles, current) = match dialog {
            Dialog::Mcp => (vec!["通用 MCP", "pi agent"], usize::from(self.pi)),
            Dialog::Profiles => (
                std::iter::once("新建连接")
                    .chain(self.profiles.iter().map(|p| str_at(p, "name")))
                    .collect(),
                self.profile_index(),
            ),
            _ => return false,
        };
        let Some(menu) = crate::mac_window::PickerMenu::new(window, &titles, current) else {
            return false;
        };
        let profile_ids: Vec<_> = if dialog == Dialog::Profiles {
            self.profiles.iter().map(|p| str_at(p, "id").to_owned()).collect()
        } else {
            Vec::new()
        };
        // AppKit tracks menus in its own event loop. Release the GPUI view
        // borrow first so terminal and service updates can still be delivered.
        cx.spawn(async move |this, cx| {
            let Some(index) = menu.show().filter(|&index| index != current) else {
                return;
            };
            let _ = this.update(cx, |view, cx| {
                if view.dialog != dialog || view.confirm.is_some() {
                    return;
                }
                if dialog == Dialog::Mcp {
                    view.pi = index == 1;
                    view.refresh_config(cx);
                } else {
                    view.highlighted_profile = if index == 0 {
                        0
                    } else if let Some(position) = profile_ids.get(index - 1).and_then(|id| {
                        view.profiles.iter().position(|p| str_at(p, "id") == id)
                    }) {
                        position + 1
                    } else {
                        return;
                    };
                    view.commit_profile(cx);
                }
                cx.notify();
            });
        }).detach();
        true
    }

    fn profile_menu_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let last = self.profiles.len();
        let current = if self.chooser {
            self.highlighted_profile
        } else {
            self.profile_index()
        };
        let next = match key {
            "up" | "down" if cfg!(target_os = "macos") && !self.chooser => {
                self.open_profile_menu();
                return true;
            }
            "up" => current.saturating_sub(1),
            "down" => (current + 1).min(last),
            "home" => 0,
            "end" => last,
            "escape" if self.chooser => {
                self.chooser = false;
                self.highlighted_profile = self.profile_index();
                return true;
            }
            typed if typed.chars().count() == 1 => {
                let typed = typed.to_lowercase();
                let cycle = self.last_profile_key.elapsed() >= Duration::from_secs(1)
                    || self.profile_prefix == typed;
                if cycle {
                    self.profile_prefix.clear();
                }
                self.profile_prefix.push_str(&typed);
                self.last_profile_key = Instant::now();
                (0..=last)
                    .map(|step| (current + step + usize::from(cycle)) % (last + 1))
                    .find(|&i| {
                        let name = if i == 0 {
                            "新建连接"
                        } else {
                            str_at(&self.profiles[i - 1], "name")
                        };
                        name.to_lowercase().starts_with(&self.profile_prefix)
                    })
                    .unwrap_or(current)
            }
            _ => return false,
        };
        self.highlighted_profile = next;
        if self.chooser {
            self.profile_scroll.scroll_to_item(next);
        } else {
            self.commit_profile(cx);
        }
        true
    }

    fn open_profiles(&mut self, cx: &mut Context<Self>) {
        match store::list(&self.dir) {
            Ok(v) => {
                self.profiles = items(&v["profiles"]).to_vec();
                let p = self
                    .profiles
                    .iter()
                    .find(|p| p["id"] == v["default_id"])
                    .cloned()
                    .unwrap_or(Value::Null);
                self.fill(p.clone(), p, cx);
                self.dialog = Dialog::Profiles;
                self.note.clear();
            }
            Err(e) => self.note = e.to_string(),
        }
    }
    fn submit(&mut self, connect: bool, w: &mut Window, cx: &mut Context<Self>) -> Result<()> {
        ensure!(!self.busy, "正在连接");
        let mut args = json!({"id":self.profile["id"],"remember_password":self.remember});
        for (k, f) in ["name", "host", "port", "user", "password"]
            .iter()
            .zip(&self.fields)
        {
            args[*k] = json!(f.read(cx).content.to_string());
        }
        for (index, key) in [(1, "host"), (2, "port"), (3, "user")] {
            let value = args[key].as_str().unwrap_or_default();
            if let Some(message) = crate::form_validation::message(value, key == "port") {
                let field = self.fields[index].read(cx);
                self.validation = Some(crate::form_validation::Hint::new(
                    index, field.content.clone(), message,
                ));
                w.focus(&field.focus_handle(cx));
                self.reveal_dialog_focus(w);
                self.note.clear();
                // Scrolling and focus just changed; the next frame has the field's new bounds.
                let view = cx.weak_entity();
                w.on_next_frame(move |_, cx| {
                    let _ = view.update(cx, |_, cx| cx.notify());
                });
                cx.notify();
                return Ok(());
            }
        }
        self.validation = None;
        self.fields[4].update(cx, |field, cx| field.clear_secret(cx));
        if self.save {
            let saved = store::upsert(&self.dir, &args)?;
            self.profiles = items(&store::list(&self.dir)?["profiles"]).to_vec();
            self.fill(saved.clone(), saved, cx);
        }
        if connect {
            let payload = if self.matches_profile_target(cx) {
                json!({"profile_id":self.profile["id"],"password":args["password"]})
            } else {
                args
            };
            self.busy = true;
            self.queue("connect", payload);
        } else {
            self.note = if self.profile["has_password"] == true {
                "已保存连接配置和密码。"
            } else {
                "已保存连接配置。"
            }
            .into();
        }
        Ok(())
    }
    fn manual(&self) -> bool {
        self.state["state"] == "manual"
            && self.state["owner"] == "human"
            && !self.busy
            && !self.reconnecting.contains(&self.selected)
    }
    fn input(&mut self, text: &str) {
        if !self.manual() || text.is_empty() {
            return;
        }
        self.clear_selection();
        self.focus_terminal();
        self.parser.set_scrollback(0);
        let mut buf = String::new();
        for ch in text.chars() {
            buf.push(ch);
            if buf.len() >= 8192 {
                self.queue("input", json!({"session_id":self.selected,"data":buf}));
                buf.clear();
            }
        }
        if !buf.is_empty() {
            self.queue("input", json!({"session_id":self.selected,"data":buf}));
        }
    }
    fn clear_selection(&mut self) {
        self.selection = None;
        self.initial_selection = None;
        self.drag = None;
    }
    fn sorted_selection(&self) -> Option<(BufferCell, BufferCell)> {
        self.selection
            .filter(|(a, b)| a != b && (!self.column_selection || a.1 != b.1))
            .map(|(a, b)| (a.min(b), a.max(b)))
    }
    fn copy_terminal(&mut self, visible_fallback: bool, cx: &mut Context<Self>) {
        let s = self.parser.screen();
        let text = self
            .sorted_selection()
            .map(|(a, b)| selection::text(s, a, b, self.column_selection))
            .unwrap_or_default();
        let text = if visible_fallback && text.is_empty() {
            s.rows(0, s.size().1).collect::<Vec<_>>().join("\n")
        } else if self.sorted_selection().is_some() {
            text
        } else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.note = "已复制终端内容。".into();
    }
    fn select_all_terminal(&mut self) {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        self.column_selection = false;
        self.selection = Some((
            (0, 0),
            (screen.history_rows() + usize::from(rows) - 1, cols),
        ));
        self.initial_selection = self.selection;
    }

    fn paste_terminal(&mut self, cx: &Context<Self>) {
        if !self.manual() {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            // The macOS WebKit terminal delivers canonically composed clipboard text.
            #[cfg(target_os = "macos")]
            let text = objc2_foundation::NSString::from_str(&text)
                .precomposedStringWithCanonicalMapping()
                .to_string();
            let text = text.replace("\r\n", "\r").replace('\n', "\r");
            if self.parser.screen().bracketed_paste() {
                self.input(&format!("\x1b[200~{text}\x1b[201~"));
            } else {
                self.input(&text);
            }
        }
    }

    fn menu_allows(
        &self,
        command: crate::menu::EditCommand,
        w: &Window,
        cx: &Context<Self>,
    ) -> bool {
        use crate::menu::EditCommand::*;
        if self.confirm.is_some() {
            return false;
        }
        if self.dialog == Dialog::Profiles {
            return self
                .fields
                .iter()
                .map(|f| f.read(cx))
                .find(|f| f.focus_handle(cx).is_focused(w))
                .is_some_and(|f| f.can_edit(command));
        }
        if self.dialog == Dialog::Mcp && self.config_area.read(cx).focused(w) {
            return command == SelectAll || command == Copy && self.config_area.read(cx).can_copy();
        }
        if self.dialog == Dialog::None && self.history && self.history_focus.is_focused(w) {
            return command == SelectAll || command == Copy && self.history_text.can_copy();
        }
        self.dialog == Dialog::None
            && self.focus.is_focused(w)
            && match command {
                Copy => self.sorted_selection().is_some(),
                Paste => self.manual(),
                SelectAll => true,
                _ => false,
            }
    }

    pub(crate) fn menu_edit(
        &mut self,
        command: crate::menu::EditCommand,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::menu::EditCommand::*;
        if !self.menu_allows(command, w, cx) {
            return;
        }
        match self.dialog {
            Dialog::Profiles => w.dispatch_action(command.action(), cx),
            Dialog::Mcp => self.config_area.update(cx, |area, cx| {
                if command == Copy {
                    area.copy(cx);
                } else {
                    area.select_all();
                }
                cx.notify();
            }),
            Dialog::None if self.history_focus.is_focused(w) => match command {
                Copy => cx.write_to_clipboard(ClipboardItem::new_string(
                    self.history_text.selected_text(),
                )),
                SelectAll => self.history_text.select_all(),
                _ => {}
            },
            Dialog::None => match command {
                Copy => self.copy_terminal(false, cx),
                Paste => self.paste_terminal(cx),
                SelectAll => self.select_all_terminal(),
                _ => {}
            },
            Dialog::Skins => {}
        }
        cx.notify();
    }

    fn open_clipboard(
        &mut self,
        e: &crate::menu::OpenClipboardMenu,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(origin) = w.focused(cx) else { return };
        use crate::menu::EditCommand::{Copy, Paste};
        let enabled = [
            self.menu_allows(Copy, w, cx),
            self.menu_allows(Paste, w, cx),
        ];
        self.clipboard_popup = Some(ClipboardPopup {
            position: e.position,
            origin,
            enabled,
            highlighted: None,
        });
        w.focus(&self.popup_focus);
        cx.notify();
    }

    fn dismiss_clipboard(&mut self, w: &mut Window) {
        if let Some(menu) = self.clipboard_popup.take() {
            if self.popup_focus.is_focused(w) {
                w.focus(&menu.origin);
            }
        }
    }

    fn choose_clipboard(&mut self, index: usize, w: &mut Window, cx: &mut Context<Self>) {
        if !self
            .clipboard_popup
            .as_ref()
            .is_some_and(|m| m.enabled[index])
        {
            return;
        }
        self.dismiss_clipboard(w);
        let command = [
            crate::menu::EditCommand::Copy,
            crate::menu::EditCommand::Paste,
        ][index];
        self.menu_edit(command, w, cx);
        cx.notify();
    }

    fn clipboard_overlay(
        &self,
        menu: &ClipboardPopup,
        w: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let t = self.theme;
        let viewport = w.viewport_size();
        let mut card = div()
            .absolute()
            .left(menu.position.x.min((viewport.width - px(124.)).max(px(0.))))
            .top(menu.position.y.min((viewport.height - px(68.)).max(px(0.))))
            .w(px(124.))
            .p(px(4.))
            .rounded(px(5.))
            .border_1()
            .border_color(rgb(t.color("border-strong")))
            .shadow_md()
            .bg(rgb(t.color("panel")))
            .flex()
            .flex_col();
        for (i, label) in ["复制", "粘贴"].into_iter().enumerate() {
            card = card.child(
                div()
                    .id(("clipboard-command", i))
                    .h(px(28.))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .rounded(px(3.))
                    .child(label)
                    .when(menu.highlighted == Some(i), |d| d.bg(rgb(t.color("hover"))))
                    .when(menu.enabled[i], |d| {
                        d.cursor_pointer().hover(|s| s.bg(rgb(t.color("hover"))))
                    })
                    .when(!menu.enabled[i], |d| d.opacity(0.45))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, _, w, cx| {
                            w.prevent_default();
                            cx.stop_propagation();
                            v.choose_clipboard(i, w, cx);
                        }),
                    ),
            );
        }
        let overlay = div()
            .id("clipboard-popup")
            .absolute()
            .inset_0()
            .size_full()
            .occlude()
            .track_focus(&self.popup_focus)
            .on_key_down(cx.listener(|v, e: &KeyDownEvent, w, cx| {
                let Some(menu) = v.clipboard_popup.as_mut() else {
                    return;
                };
                match e.keystroke.key.as_str() {
                    "escape" => v.dismiss_clipboard(w),
                    "up" | "down" | "tab" => {
                        let start = menu
                            .highlighted
                            .map_or(usize::from(e.keystroke.key == "up"), |i| 1 - i);
                        menu.highlighted =
                            [start, 1 - start].into_iter().find(|i| menu.enabled[*i]);
                    }
                    "enter" | "space" => {
                        if let Some(i) = menu.highlighted {
                            v.choose_clipboard(i, w, cx)
                        }
                    }
                    _ => {}
                }
                w.prevent_default();
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, _, w, cx| {
                    v.dismiss_clipboard(w);
                    w.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|v, _, w, cx| {
                    v.dismiss_clipboard(w);
                    w.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(card);
        div()
            .absolute()
            .inset_0()
            .child(deferred(overlay).with_priority(8))
    }

    fn act(&mut self, id: &str, w: &mut Window, cx: &mut Context<Self>) -> Result<()> {
        self.dismiss_clipboard(w);
        self.validation = None;
        let before = self.dialog;
        let confirming = self.confirm.is_some();
        let origin = w.focused(cx);
        self.note.clear();
        #[cfg(target_os = "macos")]
        if matches!(id, "client" | "choose") && !self.chooser {
            w.focus(&self.chooser_focus);
            if self.show_native_picker(w, cx) {
                return Ok(());
            }
        }
        match id {
            #[cfg(windows)]
            "quit" => cx.quit(),
            "profiles" | "add" => self.open_profiles(cx),
            "dismiss" => self.close_dialog(),
            "new" => self.fill(Value::Null, Value::Null, cx),
            "choose" => {
                if self.chooser {
                    self.commit_profile(cx);
                } else {
                    self.open_profile_menu();
                }
                w.focus(&self.chooser_focus);
            }
            "remember" => self.remember = !self.remember,
            "save-toggle" => self.save = !self.save,
            "save" => self.submit(false, w, cx)?,
            "connect" => self.submit(true, w, cx)?,
            "delete" => {
                self.confirm = Some(Confirm::Delete(
                    str_at(&self.profile, "id").into(),
                    str_at(&self.profile, "name").into(),
                ))
            }
            "cancel" => self.confirm = None,
            "confirm" => match self.confirm.take() {
                Some(Confirm::Delete(id, _)) => {
                    store::delete(&self.dir, &id)?;
                    self.open_profiles(cx);
                    self.note = "已删除连接配置。".into();
                }
                Some(Confirm::Close(id, _)) => {
                    self.busy = true;
                    self.queue("close-session", json!({"session_id":id}));
                }
                None => {}
            },
            "mcp" => {
                self.dialog = Dialog::Mcp;
                self.desktop = self.endpoint.action("desktop-info", &json!({}))?;
                self.refresh_config(cx);
                self.chooser = false;
            }
            "client" => {
                if self.chooser {
                    self.pi = self.highlighted_pi;
                    self.refresh_config(cx);
                } else {
                    self.highlighted_pi = self.pi;
                }
                self.chooser = !self.chooser;
                w.focus(&self.chooser_focus);
            }
            "generic" | "pi" => {
                self.pi = id == "pi";
                w.focus(&self.chooser_focus);
                self.refresh_config(cx);
                self.chooser = false;
            }
            "copy-config" => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.config_text()));
                self.copied_config = true;
            }
            "skins" => self.dialog = Dialog::Skins,
            "takeover" | "release" => {
                self.busy = true;
                self.queue(
                    if id == "release" {
                        "release"
                    } else {
                        "takeover"
                    },
                    json!({"session_id":self.selected,"skin":self.theme.palette()["id"]}),
                );
            }
            "history" => {
                self.history = !self.history;
                self.history_text = Default::default();
            }
            "full" => {
                self.fullscreen.toggle(w);
            }
            "copy" => self.copy_terminal(true, cx),
            "clear" => {
                self.parser.clear_view();
                self.clear_selection();
            }
            "ctrlc" => self.input("\x03"),
            "escape" => self.input("\x1b"),
            "tab" => self.input("\t"),
            _ => {
                let (action, key) = id.split_once('/').context("未知操作")?;
                match action {
                    "skin" => {
                        let i: usize = key.parse()?;
                        self.theme = Theme(i);
                        self.parser.set_palette(self.theme.colors(), true);
                        if !self.selected.is_empty() {
                            self.queue(
                                "terminal-theme",
                                json!({"session_id":self.selected,"skin":skins()[i]["id"]}),
                            );
                        }
                        #[cfg(target_os = "macos")]
                        crate::mac_window::apply_theme(w, self.theme)?;
                        store::save(
                            &self.dir.join("appearance.json"),
                            &json!({"skin":skins()[i]["id"]}),
                        )?;
                        self.note = format!(
                            "已应用「{}」，下次打开自动沿用。",
                            str_at(&skins()[i], "name")
                        );
                    }
                    "profile" => {
                        self.highlighted_profile = key.parse()?;
                        self.commit_profile(cx);
                        w.focus(&self.chooser_focus);
                    }
                    "select" => self.select(key),
                    "close" => {
                        let s = self
                            .sessions
                            .iter()
                            .find(|s| s["session_id"] == key)
                            .context("会话已关闭")?;
                        self.confirm = Some(Confirm::Close(key.into(), str_at(s, "name").into()));
                    }
                    "reconnect" => {
                        if !self.busy && self.reconnecting.insert(key.into()) {
                            self.queue("reconnect", json!({"session_id":key}));
                        }
                    }
                    _ => {}
                }
            }
        }
        if self.confirm.is_some() && !confirming {
            self.confirm_origin = origin;
            w.focus(&self.confirm_buttons[0]);
        } else if confirming && self.confirm.is_none() {
            self.restore_origin(true, w);
        } else if before == Dialog::None && self.dialog != Dialog::None {
            self.dialog_scroll.set_offset(Point::default());
            // AppKit's web view leaves a mouse-clicked launcher unfocused.
            // Restoring it on Escape would make Return reopen the dialog.
            self.dialog_origin = if cfg!(target_os = "macos") && !self.keyboard_focus {
                None
            } else {
                origin
            };
            w.focus(&self.modal_dismiss);
        } else if before != Dialog::None && self.dialog == Dialog::None {
            self.restore_origin(false, w);
        } else if matches!(
            id,
            "takeover" | "release" | "ctrlc" | "escape" | "tab" | "clear" | "full"
        ) {
            self.focus_terminal();
            w.focus(&self.focus);
        }
        Ok(())
    }
    fn reveal_dialog_focus(&self, w: &Window) {
        if self.modal_dismiss.is_focused(w) {
            self.dialog_scroll.set_offset(Point::default());
            return;
        }
        // A tab stop may be below the fold in a short window.
        if let Some(focus) = w.focused_bounds() {
            let viewport = self.dialog_scroll.bounds();
            let margin = px(4.);
            let top = focus.top() - margin;
            let bottom = focus.bottom() + margin;
            let delta = if top < viewport.top() {
                viewport.top() - top
            } else if bottom > viewport.bottom() {
                viewport.bottom() - bottom
            } else {
                px(0.)
            };
            let mut offset = self.dialog_scroll.offset();
            offset.y += delta;
            self.dialog_scroll.set_offset(offset);
        }
    }
    fn restore_origin(&mut self, confirm: bool, w: &mut Window) {
        let origin = if confirm {
            self.confirm_origin.take()
        } else {
            self.dialog_origin.take()
        };
        #[cfg(target_os = "macos")]
        if !confirm && self.dialog == Dialog::None && origin.is_none() {
            w.blur();
            return;
        }
        let fallback = if self.dialog == Dialog::None {
            &self.page_focus
        } else {
            &self.modal_dismiss
        };
        w.focus(origin.as_ref().unwrap_or(fallback));
    }
    fn refresh_config(&self, cx: &mut Context<Self>) {
        let text = self.config_text();
        self.config_area
            .update(cx, |area, cx| area.show(text, self.theme, cx));
    }
    fn config_text(&self) -> String {
        serde_json::to_string_pretty(
            &self.desktop[if self.pi { "pi_config" } else { "mcp_config" }],
        )
        .unwrap_or_default()
    }
    #[cfg(target_os = "macos")]
    pub(crate) fn theme(&self) -> Theme {
        self.theme
    }

    fn workspace_interactive(&self) -> bool {
        self.dialog == Dialog::None && self.confirm.is_none() && self.clipboard_popup.is_none()
    }

    fn btn(
        &self,
        id: impl Into<String>,
        label: impl Into<SharedString>,
        kind: &str,
        enabled: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let id = id.into();
        let t = self.theme;
        let accent = match kind {
            "primary" => Some("green"),
            "release" => Some("amber"),
            _ => None,
        };
        let mut b = div()
            .id(SharedString::from(id.clone()))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .whitespace_nowrap()
            .text_size(px(11.))
            .px(px(9.))
            .h(px(28.))
            .rounded(px(5.));
        if kind != "bare" {
            b = b
                .border_1()
                .border_color(rgb(t.color(accent.unwrap_or("border"))));
        }
        if let Some(c) = accent {
            b = b
                .bg(rgb(t.color(c)))
                .text_color(rgb(t.color("on-accent")))
                .font(heading_font(FontWeight::SEMIBOLD));
        }
        if kind == "danger" {
            b = b.text_color(rgb(t.color("danger")));
        }
        let hint = match id.as_str() {
            "skins" => Some("切换界面与终端皮肤"),
            "quit" => Some("退出垒桥终端并关闭后台及所有 SSH 连接"),
            "takeover" => Some("接管会中断正在运行的 Agent 命令"),
            "release" => Some("请先退出 vim、top 等全屏程序，再交还 Agent"),
            "add" => Some("连接另一台主机"),
            "copy" => Some("复制选中的文本；未选中时复制可见终端内容"),
            "clear" => Some("只清空本页显示，不清除远端内容"),
            "full" => Some("切换全屏"),
            "ctrlc" => Some("发送 Ctrl-C 中断"),
            "history" => Some(if self.history {
                "折叠执行记录"
            } else {
                "展开执行记录"
            }),
            _ => None,
        };
        if let Some(hint) = hint {
            b = b.when(self.workspace_interactive(), |item| {
                item.tooltip(hover_tip(hint, t))
            });
        }
        if enabled {
            let (hover_fill, hover_border) = match accent {
                Some("amber") => ("amber", "amber"),
                Some(_) => ("accent-hover", "accent-hover"),
                None => ("hover", "border-strong"),
            };
            b = b
                .cursor_pointer()
                .when(!matches!(id.as_str(), "client" | "choose"), |b| {
                    b.hover(move |s| {
                        s.bg(rgb(t.color(hover_fill))).border_color(rgb(t.color(hover_border)))
                    })
                })
                .on_click(cx.listener(move |v, _, w, cx| {
                    if let Err(e) = v.act(&id, w, cx) {
                        v.note = e.to_string();
                    }
                    cx.notify();
                }));
        } else if let Some(accent) = accent {
            // CSS applies opacity to the finished button as one layer. Blending each
            // glyph over an already translucent fill would wash out the disabled text.
            let surface = t.color(if id == "connect" { "panel" } else { "bg" });
            let disabled = |key| {
                let fg = t.color(key);
                #[cfg(target_os = "macos")]
                if let Some(c) = self.disabled_colors.blend(surface, fg) {
                    return c;
                }
                rgb(surface).blend(Rgba { a: 0.4, ..rgb(fg) })
            };
            let fill = disabled(accent);
            b = b.bg(fill).border_color(fill).text_color(disabled("on-accent"));
        } else {
            b = b.opacity(0.4);
        }
        if enabled && self.dialog == Dialog::None && self.confirm.is_none() {
            b = b
                .tab_index(0)
                .when(self.keyboard_focus, |b| {
                    b.focus(|style| focus_outline(style, t))
                });
        }
        let label = label.into();
        b.when(!label.is_empty(), |b| b.child(label))
    }
    fn key(&mut self, e: &KeyDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let key = k.key.as_str();
        let windows_insert = cfg!(windows) && key == "insert";
        if !e.is_held {
            self.consumed_escape = false;
        }
        if self
            .fields
            .iter()
            .any(|f| f.read(cx).composing() && f.read(cx).focus_handle(cx).is_focused(w))
        {
            return;
        }
        if self.confirm.is_some() || self.dialog != Dialog::None {
            #[cfg(target_os = "macos")]
            if self.confirm.is_none() && !self.chooser && self.chooser_focus.is_focused(w)
                && matches!(key, "up" | "down") && k.modifiers == Modifiers::default()
                && self.show_native_picker(w, cx)
            {
                cx.stop_propagation();
                cx.notify();
                return;
            }
            if self.dialog == Dialog::Profiles
                && self.confirm.is_none()
                && self.chooser_focus.is_focused(w)
                && !k.modifiers.alt
                && !k.modifiers.control
                && !k.modifiers.platform
                && self.profile_menu_key(key, cx)
            {
                cx.stop_propagation();
                cx.notify();
                return;
            }
            if self.dialog == Dialog::Mcp
                && self.chooser_focus.is_focused(w)
                && matches!(key, "up" | "down" | "home" | "end" | "p")
            {
                self.highlighted_pi = matches!(key, "down" | "end" | "p");
                if !self.chooser {
                    self.pi = self.highlighted_pi;
                    self.refresh_config(cx);
                }
            } else if self.dialog == Dialog::Mcp && self.chooser && key == "escape" {
                self.chooser = false;
                w.focus(&self.chooser_focus);
            } else if key == "escape" {
                let confirming = self.confirm.take().is_some();
                if !confirming {
                    self.close_dialog();
                }
                self.restore_origin(confirming, w);
            } else if self.confirm.is_some() && key == "tab" {
                let next = usize::from(self.confirm_buttons[0].is_focused(w));
                w.focus(&self.confirm_buttons[next]);
            } else if key == "tab" {
                if self.dialog == Dialog::Profiles && self.chooser {
                    self.commit_profile(cx);
                }
                if self.dialog == Dialog::Mcp && self.chooser {
                    self.pi = self.highlighted_pi;
                    self.chooser = false;
                    self.refresh_config(cx);
                }
                if k.modifiers.shift {
                    w.focus_prev()
                } else {
                    w.focus_next()
                }
                self.reveal_dialog_focus(w);
            } else if self.dialog == Dialog::Profiles
                && key == "enter"
                && self.confirm.is_none()
                && self
                    .fields
                    .iter()
                    .any(|f| f.read(cx).focus_handle(cx).is_focused(w))
            {
                if self.save
                    && !self.busy
                    && let Err(e) = self.submit(false, w, cx)
                {
                    self.note = e.to_string();
                }
            } else {
                return;
            }
        } else if key == "escape"
            && k.modifiers == Modifiers::default()
            && self.composition.text.is_empty()
            && (self.fullscreen.active() || self.consumed_escape)
        {
            if self.fullscreen.active() {
                self.fullscreen.toggle(w);
            }
            self.consumed_escape = true;
        } else if self.history
            && self.history_focus.is_focused(w)
            && ((if cfg!(target_os = "macos") {
                k.modifiers == Modifiers::command()
            } else {
                k.modifiers == Modifiers::control()
            }) && matches!(key, "a" | "c")
                || windows_insert && k.modifiers == Modifiers::control())
        {
            self.menu_edit(
                if key == "a" {
                    crate::menu::EditCommand::SelectAll
                } else {
                    crate::menu::EditCommand::Copy
                },
                w,
                cx,
            );
        } else if !self.focus.is_focused(w) {
            if key != "tab" || k.modifiers.alt || k.modifiers.control || k.modifiers.platform {
                return;
            }
            if k.modifiers.shift {
                w.focus_prev();
            } else {
                w.focus_next();
            }
        } else if cfg!(target_os = "macos")
            && self.terminal_focused
            && k.modifiers.platform
            && !k.modifiers.alt
            && !k.modifiers.control
            && !k.modifiers.shift
            && key == "a"
        {
            self.select_all_terminal();
        } else if ((k.modifiers.platform || k.modifiers.control && k.modifiers.shift) && key == "c")
            || (windows_insert && k.modifiers == Modifiers::control())
        {
            self.copy_terminal(false, cx);
        } else if ((k.modifiers.platform || k.modifiers.control && k.modifiers.shift) && key == "v")
            || (windows_insert && k.modifiers == Modifiers::shift())
        {
            self.paste_terminal(cx);
        } else if k.modifiers.shift && matches!(key, "pageup" | "pagedown") {
            let page = usize::from(self.parser.screen().size().0.saturating_sub(1));
            let offset = self.parser.screen().scrollback();
            self.parser.set_scrollback(if key == "pageup" {
                offset.saturating_add(page)
            } else {
                offset.saturating_sub(page)
            });
        } else if self.manual() {
            if !self.composition.text.is_empty() {
                return;
            }
            let Some(data) = crate::keyboard::sequence(
                key,
                k.modifiers,
                self.parser.screen().application_cursor(),
            ) else {
                return;
            };
            self.input(&data);
        } else {
            return;
        }
        w.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }
}

impl View {
    fn label(&self, text: impl Into<SharedString>, color: &str, size: f32) -> Div {
        div()
            .text_size(px(size))
            .text_color(rgb(self.theme.color(color)))
            .child(text.into())
    }
    fn bar(&self, height: f32) -> Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(height))
            .border_b_1()
            .border_color(rgb(self.theme.color("border")))
    }
    fn header(&self, width: f32, cx: &Context<Self>) -> Div {
        let t = self.theme;
        let human_owner = self.state["owner"] == "human";
        let manual = human_owner && self.state["state"] == "manual";
        let connected = matches!(
            str_at(&self.state, "state"),
            "ready" | "busy" | "manual" | "synchronizing"
        );
        let mut h = self
            .bar(44.)
            .px(px(12.))
            .gap(px(if width <= 760. { 8. } else { 12. }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .flex_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .font(heading_font(FontWeight::SEMIBOLD))
                            .child("垒桥终端")
                            .when(width > 600., |b| {
                                b.child(self.label("/", "muted", 13.).mx(px(6.)))
                                    .child("BastionTerm")
                            }),
                    )
                    .child(
                        self.label(concat!("v", env!("CARGO_PKG_VERSION")), "muted", 10.)
                            .font_family(mono())
                            .id("app-version")
                            .when(self.workspace_interactive(), |item| {
                                item.tooltip(hover_tip(
                                    concat!("当前后台版本 ", env!("CARGO_PKG_VERSION")),
                                    t,
                                ))
                            }),
                    ),
            );
        if width > 500. {
            h = h.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    // In the compact header the text has zero width, but its flex gap remains.
                    .when(width <= 760., |live| live.pr(px(7.)))
                    .child(
                        div()
                            .size(px(if width <= 760. { 6. } else { 5. }))
                            .rounded_full()
                            .bg(rgb(t.color(if connected { "green" } else { "muted" }))),
                    )
                    .when(width > 760., |live| {
                        live.child(self.label(
                            if connected {
                                "SSH 已连接"
                            } else if self.state["state"] == "connecting" {
                                "SSH 登录中…"
                            } else {
                                "SSH 未连接"
                            },
                            "secondary",
                            11.,
                        ))
                    }),
            );
        }
        if width > 760. {
            h = h.child(self.label(
                if manual {
                    "你正在操作"
                } else {
                    "Agent 操作"
                },
                if manual { "amber" } else { "muted" },
                11.,
            ));
        }
        let button = |id, label, kind, enabled| {
            self.btn(id, label, kind, enabled, cx)
                .px(px(if id == "skins" { 8. } else { 10. }))
        };
        h.child(button("skins", "皮肤", "", true))
            .child(button("mcp", "MCP 设置", "", true))
            .child(button("profiles", "连接配置", "", !self.busy))
            .child(if human_owner {
                button("release", "交还 Agent", "release", self.manual())
            } else {
                button(
                    "takeover",
                    "接管终端",
                    "primary",
                    connected && !self.busy && !self.reconnecting.contains(&self.selected),
                )
            })
            .when(cfg!(windows), |bar| {
                bar.child(button("quit", "退出", "", true))
            })
    }
    fn tabs(&self, cx: &Context<Self>) -> Div {
        let t = self.theme;
        let mut list = div()
            .id("session-tabs")
            .flex()
            .min_w_0()
            .overflow_x_scroll();
        for s in &self.sessions {
            let id = str_at(s, "session_id");
            let active = id == self.selected;
            let reconnecting = self.reconnecting.contains(id);
            list = list.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h_full()
                    .max_w(px(280.))
                    .border_r_1()
                    .border_color(rgb(t.color("border")))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h_full()
                            .min_w_0()
                            .max_w_full()
                            .border_b_2()
                            .border_color(rgb(t.color(if active { "green" } else { "panel" })))
                            .when(active, |d| d.bg(rgb(t.color("hover"))))
                            .child(
                                self.btn(format!("select/{id}"), "", "bare", true, cx)
                                    .gap(px(8.))
                                    .min_w_0()
                                    .flex_shrink()
                                    .px(px(10.))
                                    .h_full()
                                    .rounded_none()
                                    .when(self.workspace_interactive(), |item| {
                                        item.tooltip(hover_tip(
                                            format!(
                                                "{}@{}:{}",
                                                str_at(&s["target"], "user"),
                                                str_at(&s["target"], "host"),
                                                s["target"]["port"]
                                            ),
                                            t,
                                        ))
                                    })
                                    .child(
                                        self.label(str_at(s, "name").to_owned(), "text", 11.)
                                            .min_w_0()
                                            .truncate(),
                                    )
                                    .child(
                                        self.label(
                                            if reconnecting {
                                                "重连中…"
                                            } else {
                                                state_label(s)
                                            },
                                            if s["state"] == "manual" {
                                                "amber"
                                            } else if matches!(str_at(s, "state"), "ready" | "busy")
                                            {
                                                "green"
                                            } else {
                                                "muted"
                                            },
                                            9.,
                                        )
                                        .flex_none(),
                                    ),
                            )
                            .child(
                                self.btn(
                                    format!("reconnect/{id}"),
                                    "↻",
                                    "bare",
                                    s["state"] == "disconnected" && !self.busy && !reconnecting,
                                    cx,
                                )
                                .px_0()
                                .w(px(26.))
                                .h_full()
                                .rounded_none()
                                .text_color(rgb(t.color(
                                    if s["state"] == "disconnected" && !reconnecting {
                                        "green"
                                    } else {
                                        "muted"
                                    },
                                )))
                                .when(self.workspace_interactive(), |item| {
                                    item.tooltip(hover_tip(
                                        if reconnecting {
                                            "正在重新连接…"
                                        } else if s["state"] == "disconnected" {
                                            "重新连接此 SSH 主机"
                                        } else {
                                            "仅在 SSH 断开后可重连"
                                        },
                                        t,
                                    ))
                                })
                                .text_size(px(16.)),
                            )
                            .child(
                                self.btn(
                                    format!("close/{id}"),
                                    "×",
                                    "bare",
                                    !self.busy && !reconnecting,
                                    cx,
                                )
                                .px(px(7.))
                                .h_full()
                                .rounded_none()
                                .text_color(rgb(t.color("muted")))
                                .when(self.workspace_interactive(), |item| {
                                    item.tooltip(hover_tip("关闭此 SSH 会话", t))
                                }),
                            ),
                    ),
            );
        }
        self.bar(34.).bg(rgb(t.color("panel"))).child(list).child(
            self.btn("add", "＋", "bare", !self.busy, cx)
                .w(px(34.))
                .px_0()
                .h_full()
                .rounded_none()
                .text_size(px(19.))
                .text_color(rgb(t.color("green"))),
        )
    }
    fn sidebar(&mut self, width: f32, w: &Window, cx: &Context<Self>) -> Div {
        let t = self.theme;
        let mut aside = div().flex().h_full().flex_none().child(
            div()
                .w(px(24.))
                .h_full()
                .border_l_1()
                .border_color(rgb(t.color("border")))
                .bg(rgb(t.color("panel")))
                .child(
                    self.btn(
                        "history",
                        if self.history { "▸" } else { "◂" },
                        "bare",
                        true,
                        cx,
                    )
                    .w(px(23.))
                    .h(px(32.))
                    .px_0()
                    .rounded_none()
                    .text_size(px(18.))
                    .text_color(rgb(t.color(if self.history_hovered && self.workspace_interactive() {
                        "green"
                    } else {
                        "secondary"
                    })))
                    .on_hover(cx.listener(|v, over, _, cx| {
                        v.history_hovered = *over;
                        cx.notify();
                    })),
                ),
        );
        if self.history {
            let text_width = px(width - 24.);
            if self
                .history_previews
                .as_ref()
                .is_none_or(|(cached, _)| *cached != text_width)
            {
                let previews = self
                    .jobs
                    .iter()
                    .map(|job| {
                        let full = str_at(job, "command")
                            .replace("\r\n", "\n")
                            .replace('\r', "\n");
                        let preview = history_preview(&full, text_width, w);
                        (SharedString::from(full), preview)
                    })
                    .collect();
                self.history_previews = Some((text_width, previews));
            }
            let (_, previews) = self.history_previews.as_ref().unwrap();
            self.history_text.begin_frame();
            let mut jobs = div()
                .id("history-jobs")
                .track_focus(&self.history_focus)
                .track_scroll(&self.history_text.scroll)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|v, e: &MouseDownEvent, w, cx| {
                        if !v.workspace_interactive() {
                            return;
                        }
                        if let Some(hit) = v.history_text.nearest(e.position) {
                            w.focus(&v.history_focus);
                            v.clear_selection();
                            v.history_text.select(hit, e.modifiers.shift, e.click_count);
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|v, _, _, _| v.history_text.dragging = false),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|v, _, _, _| v.history_text.dragging = false),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|v, e: &MouseDownEvent, w, cx| {
                        w.focus(&v.history_focus);
                        #[cfg(target_os = "macos")]
                        if let Some(hit) = v.history_text.nearest(e.position) {
                            v.history_text.context_word(hit)
                        }
                        v.open_clipboard(
                            &crate::menu::OpenClipboardMenu {
                                position: e.position,
                            },
                            w,
                            cx,
                        );
                        cx.stop_propagation();
                    }),
                )
                .flex_1()
                .min_h_0()
                .overflow_y_scroll();
            for (j, (full, preview)) in self.jobs.iter().zip(previews) {
                let code = j["exit_code"].as_i64();
                let status = match str_at(j, "status") {
                    "running" => "● 执行中".into(),
                    "disconnected" => "连接中断".into(),
                    _ => format!("EXIT {}", code.unwrap_or(-1)),
                };
                let stamp = j["started_at"]
                    .as_f64()
                    .filter(|time| *time != 0.)
                    .and_then(|time| chrono::DateTime::from_timestamp(time.floor() as i64, 0))
                    .map(|d| {
                        d.with_timezone(&chrono::Local)
                            .format("%H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_else(|| "—".into());
                let stamp: SharedString = stamp.into();
                let status: SharedString = status.into();
                jobs = jobs.child(
                    div()
                        .py(px(12.))
                        .border_b_1()
                        .border_color(rgb(t.color("border")))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .line_height(px(if j["status"] == "running" { 15. } else { 14. }))
                                .child(self.history_text.span(
                                    stamp.clone(),
                                    stamp,
                                    t.color("muted"),
                                    10.,
                                ))
                                .child(self.history_text.span(
                                    status.clone(),
                                    status,
                                    t.color(if code == Some(0) {
                                        "green"
                                    } else if j["status"] == "running" {
                                        "amber"
                                    } else {
                                        "danger"
                                    }),
                                    10.,
                                )),
                        )
                        .child(
                            self.history_text
                                .span(full.clone(), preview.clone(), t.color("secondary"), 11.)
                                .mt(px(7.))
                                .font_family(mono())
                                .line_height(px(17.6))
                                .max_h(px(3. * 17.6))
                                .overflow_hidden()
                                .id(SharedString::from(format!(
                                    "job-command/{}",
                                    str_at(j, "id")
                                )))
                                .when(self.workspace_interactive(), |item| {
                                    item.tooltip(hover_tip(str_at(j, "command").to_owned(), t))
                                }),
                        ),
                );
            }
            aside = aside.child(
                div()
                    .w(px(width))
                    .h_full()
                    .px(px(12.))
                    .flex()
                    .flex_col()
                    .child(
                        self.bar(40.)
                            .justify_between()
                            .child(
                                div()
                                    .font(heading_font(FontWeight::MEDIUM))
                                    .child("Agent 执行记录"),
                            )
                            .child(
                                self.label(self.jobs.len().to_string(), "muted", 10.)
                                    .font_family(mono())
                                    .border_1()
                                    .border_color(rgb(t.color("border")))
                                    .rounded(px(3.))
                                    .px(px(5.))
                                    .py(px(2.)),
                            ),
                    )
                    .child(jobs),
            );
        }
        aside
    }
    fn grid_position(&self, position: Point<Pixels>) -> (f32, f32) {
        let x = (f32::from(position.x) - 12.) / self.cell_width;
        let y = (f32::from(position.y) - if self.fullscreen.active() { 41. } else { 119. })
            / self.row_height;
        (x, y)
    }
    fn click_cell(&self, position: Point<Pixels>, selection_boundary: bool) -> Cell {
        let (x, y) = self.grid_position(position);
        let end = self.geometry.0 - u16::from(!selection_boundary);
        let x = if selection_boundary { x + 0.5 } else { x };
        (
            (y.ceil() - 1.).clamp(0., f32::from(self.geometry.1 - 1)) as u16,
            (x.ceil() - 1.).clamp(0., f32::from(end)) as u16,
        )
    }
    fn link_at(&self, position: Point<Pixels>) -> Option<display::LinkSpan> {
        let (x, y) = self.grid_position(position);
        if !(0. ..f32::from(self.geometry.0)).contains(&x)
            || !(0. ..f32::from(self.geometry.1)).contains(&y)
        {
            return None;
        }
        let screen = self.parser.screen();
        display::LinkSpan::at(screen, screen.viewport_start() + y as usize, x as u16)
    }

    fn release_link(&mut self, position: Point<Pixels>, w: &mut Window, cx: &mut Context<Self>) {
        let Some(pressed) = self.pressed_link.take() else {
            return;
        };
        if !self.link_at(position).is_some_and(|up| pressed.same(&up)) {
            return;
        }
        let Some(url) = pressed.target.url().map(str::to_owned) else {
            return;
        };
        let answer = w.prompt(
            PromptLevel::Warning,
            "在浏览器中打开此链接？",
            Some(&url),
            &[PromptButton::ok("打开链接"), PromptButton::cancel("取消")],
            cx,
        );
        cx.spawn(async move |_, cx| {
            if matches!(answer.await, Ok(0)) {
                let _ = cx.update(|app| app.open_url(&url));
            }
        })
        .detach();
    }

    fn buffer_cell(&self, point: Point<Pixels>) -> BufferCell {
        let (row, col) = self.click_cell(point, true);
        (
            self.parser.screen().viewport_start() + usize::from(row),
            col,
        )
    }

    fn report_mouse(
        &mut self,
        position: Point<Pixels>,
        mods: Modifiers,
        button: u8,
        release: bool,
        moving: bool,
    ) -> bool {
        use vt100::{MouseProtocolEncoding as Encoding, MouseProtocolMode as Mode};
        let screen = self.parser.screen();
        let mode = screen.mouse_protocol_mode();
        let encoding = screen.mouse_protocol_encoding();
        if !self.manual() || mode == Mode::None {
            return false;
        }
        let press = !moving && !release && button < 3;
        if press && mods.shift && !cfg!(target_os = "macos") {
            return false; // Shift starts a local selection on Windows.
        }
        if release {
            if self.mouse_buttons & (1 << button) == 0 {
                return false;
            }
            self.mouse_buttons &= !(1 << button);
        } else if press && mode != Mode::Press {
            self.mouse_buttons |= 1 << button;
        }
        if (mode == Mode::Press && !press)
            || (moving
                && (mode == Mode::PressRelease
                    || mode == Mode::ButtonMotion && button == 3
                    || button < 3 && self.mouse_buttons & (1 << button) == 0))
        {
            return true;
        }
        let (x, y) = if encoding == Encoding::SgrPixels {
            let width = self.cell_width * f32::from(self.geometry.0);
            let height = self.row_height * f32::from(self.geometry.1);
            let x = f32::from(position.x) - 12.;
            let y = f32::from(position.y) - if self.fullscreen.active() { 41. } else { 119. };
            (
                x.clamp(0., width - 1.) as u32,
                y.clamp(0., height - 1.) as u32,
            )
        } else {
            let (x, y) = self.grid_position(position);
            (
                x.clamp(0., f32::from(self.geometry.0 - 1)) as u32 + 1,
                y.clamp(0., f32::from(self.geometry.1 - 1)) as u32 + 1,
            )
        };
        let modifiers = if mode == Mode::Press {
            0
        } else {
            4 * u8::from(mods.shift) + 8 * u8::from(mods.alt) + 16 * u8::from(mods.control)
        };
        let code = button + 32 * u8::from(moving) + modifiers;
        let motion = moving.then_some((encoding, code, x, y));
        if moving && self.last_mouse_move == motion {
            return true;
        }
        self.last_mouse_move = motion;
        let bytes = match encoding {
            Encoding::Sgr | Encoding::SgrPixels => {
                format!("\x1b[<{code};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes()
            }
            Encoding::Default => {
                if x > 223 || y > 223 {
                    return true;
                }
                let code = if release { 3 + modifiers } else { code };
                vec![27, 91, 77, code + 32, (x + 32) as u8, (y + 32) as u8]
            }
        };
        use base64::Engine;
        self.queue("input-bytes",json!({"session_id":self.selected,"base64":base64::engine::general_purpose::STANDARD.encode(bytes)}));
        true
    }
    fn selection_cell(&self, point: Point<Pixels>, unit: Unit) -> BufferCell {
        let (row, mut col) = self.buffer_cell(point);
        if unit == Unit::Character
            && self
                .parser
                .screen()
                .buffer_cell(row, col)
                .is_some_and(|c| c.is_wide_continuation())
        {
            col += 1;
        }
        (row, col)
    }

    fn initial_range(&self, position: Point<Pixels>, unit: Unit) -> (BufferCell, BufferCell) {
        if unit == Unit::Word {
            if let Some(link) = self.link_at(position) {
                return ((link.row, link.columns.start), (link.row, link.columns.end));
            }
        }
        unit.range(self.parser.screen(), self.selection_cell(position, unit))
    }

    #[cfg(target_os = "macos")]
    fn select_context_word(&mut self, position: Point<Pixels>) {
        let cell = self.buffer_cell(position);
        if self
            .sorted_selection()
            .is_some_and(|(a, b)| a <= cell && cell < b)
        {
            return;
        }
        let (a, b) = self.initial_range(position, Unit::Word);
        if !self.parser.screen().buffer_text(a, b).trim().is_empty() {
            self.initial_selection = Some((a, b));
            self.column_selection = false;
        }
        self.selection = self.initial_selection;
    }

    fn extend_selection(&mut self, p: Point<Pixels>) {
        let Some((anchor, unit)) = self.drag else {
            return;
        };
        let mut current = self.selection_cell(p, unit);
        let direction = self.selection_scroll(p);
        if !self.column_selection {
            if direction < 0 {
                current.1 = 0;
            }
            if direction > 0 {
                current.1 = self.geometry.0;
            }
        }
        let (start, end) = self
            .initial_selection
            .filter(|_| unit != Unit::Character)
            .unwrap_or((anchor, anchor));
        // Releasing without dragging keeps the original word or hyperlink intact.
        let (a, b) = if current == anchor {
            (start, end)
        } else {
            unit.range(self.parser.screen(), current)
        };
        self.selection = Some(if current < anchor {
            (end.max(b), start.min(a))
        } else {
            (start.min(a), end.max(b))
        });
    }

    fn finish_selection(&mut self, e: &MouseUpEvent) {
        if self.drag.is_none() {
            return;
        }
        self.extend_selection(e.position);
        self.drag = None;
        let started = self.cursor_click_started.take();
        if !e.modifiers.alt
            || !self.manual()
            || self.parser.screen().scrollback() != 0
            || !started.is_some_and(|t| t.elapsed() < Duration::from_millis(500))
        {
            return;
        }
        let screen = self.parser.screen();
        if self.sorted_selection().is_some_and(|(a, b)| {
            selection::text(screen, a, b, self.column_selection)
                .encode_utf16()
                .nth(1)
                .is_some()
        }) {
            return;
        }
        let keys = crate::cursor_motion::arrows(screen, self.click_cell(e.position, false));
        self.input(&keys);
    }

    fn selection_scroll(&self, p: Point<Pixels>) -> isize {
        let y = f32::from(p.y) - if self.fullscreen.active() { 41. } else { 119. };
        let height = self.row_height * f32::from(self.geometry.1);
        let distance = if y < 0. { y } else { (y - height).max(0.) };
        if distance == 0. {
            return 0;
        }
        let speed = distance.signum() + (distance.clamp(-50., 50.) * 14. / 50. + 0.5).floor();
        speed as isize
    }

    fn screen(&mut self, w: &Window, cx: &Context<Self>) -> Stateful<Div> {
        if self.drag.is_some() {
            let point = w.mouse_position();
            let amount = self.selection_scroll(point);
            if amount != 0 {
                let screen = self.parser.screen();
                let offset = screen
                    .scrollback()
                    .saturating_add_signed(-amount)
                    .min(screen.history_rows());
                if offset != screen.scrollback() {
                    if self.drag_next_scroll <= Instant::now() {
                        self.parser.set_scrollback(offset);
                        self.drag_next_scroll = Instant::now() + Duration::from_millis(50);
                        self.extend_selection(point);
                    }
                    w.request_animation_frame();
                }
            }
        }
        self.sync_focus(w);
        let t = self.theme;
        let screen = self.parser.screen();
        let hover_link = if self.scrollbar.over_terminal
            && self.dialog == Dialog::None
            && self.clipboard_popup.is_none()
            && self.confirm.is_none()
        {
            self.link_at(w.mouse_position())
        } else {
            None
        };
        let rendered_link = hover_link.clone();
        let mut body = div()
            .id("terminal-body")
            .when(hover_link.is_some(), |d| d.cursor_pointer())
            .track_focus(
                &self
                    .focus
                    .clone()
                    .tab_stop(self.dialog == Dialog::None && self.confirm.is_none()),
            )
            .on_hover(cx.listener(|v, over, _, cx| {
                v.scrollbar.hover(*over);
                cx.notify();
            }))
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .pt(px(9.))
            .pl(px(12.))
            .pr(px(8.))
            .font_family(mono())
            .text_size(px(13.))
            .line_height(px(self.row_height))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, e: &MouseDownEvent, w, cx| {
                    v.focus_terminal();
                    w.focus(&v.focus);
                    v.sync_focus(w);
                    cx.notify();
                    v.pressed_link = v.link_at(e.position);
                    if v.report_mouse(e.position, e.modifiers, 0, false, false) {
                        return;
                    }
                    v.cursor_click_started = v.manual().then(Instant::now);
                    let unit = match e.click_count {
                        2 => Unit::Word,
                        3 => Unit::Line,
                        _ => Unit::Character,
                    };
                    let cell = v.selection_cell(e.position, unit);
                    let mouse_mode = v.manual()
                        && v.parser.screen().mouse_protocol_mode()
                            != vt100::MouseProtocolMode::None;
                    if e.modifiers.shift && !mouse_mode {
                        if let Some((anchor, _)) = v.selection {
                            v.drag = Some((anchor, Unit::Character));
                            v.extend_selection(e.position);
                        }
                    } else {
                        v.column_selection = unit == Unit::Character && e.modifiers.alt;
                        v.selection = Some(v.initial_range(e.position, unit));
                        v.initial_selection = v.selection.filter(|(a, b)| a != b);
                        v.drag = Some((cell, unit));
                    }
                    v.drag_next_scroll = Instant::now() + Duration::from_millis(50);
                    v.note.clear();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |v, e: &MouseMoveEvent, _, cx| {
                let current = v.link_at(e.position);
                if match (&rendered_link, current) {
                    (Some(old), Some(new)) => !old.same(&new),
                    (None, None) => false,
                    _ => true,
                } {
                    cx.notify();
                }
                let button = match e.pressed_button {
                    Some(MouseButton::Left) => 0,
                    Some(MouseButton::Middle) => 1,
                    Some(MouseButton::Right) => 2,
                    _ => 3,
                };
                if v.report_mouse(e.position, e.modifiers, button, false, true) {
                    return;
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|v, e: &MouseUpEvent, w, cx| {
                    v.report_mouse(e.position, e.modifiers, 0, true, false);
                    v.finish_selection(e);
                    v.release_link(e.position, w, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|v, e: &MouseUpEvent, _, _| {
                    v.report_mouse(e.position, e.modifiers, 0, true, false);
                    v.drag = None;
                    v.pressed_link = None;
                }),
            )
            .on_scroll_wheel(cx.listener(|v, e: &ScrollWheelEvent, _, cx| {
                let pixels = f32::from(e.delta.pixel_delta(px(v.row_height)).y);
                if pixels == 0. {
                    return;
                }
                let screen = v.parser.screen();
                let reports = !matches!(
                    screen.mouse_protocol_mode(),
                    vt100::MouseProtocolMode::None | vt100::MouseProtocolMode::Press
                ) && v.manual();
                let alternate = screen.alternate_screen();
                let mut amount = pixels / v.row_height;
                if reports || alternate {
                    if e.modifiers.shift {
                        return;
                    }
                    if e.delta.precise() && pixels.abs() < 50. {
                        amount *= 0.3;
                    }
                    if e.modifiers.alt || e.modifiers.control {
                        amount *= 5.;
                    }
                }
                v.wheel_remainder += amount;
                let lines = v.wheel_remainder.trunc() as isize;
                v.wheel_remainder = v.wheel_remainder.fract();
                if lines == 0 {
                    return;
                }
                if reports {
                    v.report_mouse(
                        e.position,
                        e.modifiers,
                        if lines > 0 { 64 } else { 65 },
                        false,
                        false,
                    );
                    return;
                }
                if alternate {
                    let key = if lines > 0 { "up" } else { "down" };
                    if let Some(data) = crate::keyboard::sequence(
                        key,
                        Modifiers::default(),
                        screen.application_cursor(),
                    ) {
                        v.input(&data);
                    }
                    return;
                }
                let rows = screen.scrollback().saturating_add_signed(lines);
                v.parser.set_scrollback(rows);
                cx.notify();
            }));
        for (button, code) in [(MouseButton::Middle, 1), (MouseButton::Right, 2)] {
            body = body
                .on_mouse_down(
                    button,
                    cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                        v.focus_terminal();
                        w.focus(&v.focus);
                        v.sync_focus(w);
                        if !v.report_mouse(e.position, e.modifiers, code, false, false)
                            && button == MouseButton::Right
                        {
                            #[cfg(target_os = "macos")]
                            v.select_context_word(e.position);
                            w.focus(&v.focus);
                            v.open_clipboard(
                                &crate::menu::OpenClipboardMenu {
                                    position: e.position,
                                },
                                w,
                                cx,
                            );
                            w.prevent_default();
                            cx.stop_propagation();
                        }
                    }),
                )
                .on_mouse_up_out(
                    button,
                    cx.listener(move |v, e: &MouseUpEvent, _, _| {
                        v.report_mouse(e.position, e.modifiers, code, true, false);
                    }),
                )
                .on_mouse_up(
                    button,
                    cx.listener(move |v, e: &MouseUpEvent, _, _| {
                        v.report_mouse(e.position, e.modifiers, code, true, false);
                    }),
                );
        }
        let focused = self.terminal_focused
            && self.focus.is_focused(w)
            && w.is_window_active()
            && self.dialog == Dialog::None
            && self.confirm.is_none();
        let frame = screen.frame_info();
        let cursor = frame.cursor.filter(|_| {
            !self.selected.is_empty() && (self.cursor_initialized || frame.cursor_initialized)
        });
        self.animate_cursor = cursor.is_some() && focused && frame.blink;
        let lit = !self.animate_cursor || self.blink;
        let shape = frame.shape;
        let block = cursor
            .filter(|_| focused && shape == vt100::CursorShape::Block)
            .map(|(r, c)| (r, c, lit));
        if !self.selected.is_empty() {
            for r in 0..screen.size().0 {
                body = body.child(
                    div()
                        .h(px(self.row_height))
                        .w_full()
                        .flex_none()
                        .bg(rgb(screen.frame_palette().background()))
                        .whitespace_nowrap()
                        .child(display::line(
                            screen.frame_row(r),
                            screen.frame_palette(),
                            r,
                            t,
                            self.sorted_selection().and_then(|(a, b)| {
                                selection::columns(
                                    a,
                                    b,
                                    frame.first_row + usize::from(r),
                                    screen.size().1,
                                    self.column_selection,
                                )
                            }),
                            self.cell_width,
                            block,
                            hover_link
                                .as_ref()
                                .filter(|h| h.row == frame.first_row + usize::from(r))
                                .map(|h| &h.columns),
                        )),
                );
            }
        }
        if let Some((row, col)) = cursor.filter(|_| block.is_none() && lit) {
            let cells = if screen
                .frame_row(row)
                .get(usize::from(col))
                .is_some_and(|cell| cell.is_wide())
            {
                2.
            } else {
                1.
            };
            let ink = rgb(screen.frame_palette().cursor());
            let marker = div()
                .absolute()
                .left(px(12. + self.cell_width * f32::from(col)))
                .top(px(9. + self.row_height * f32::from(row)))
                .h(px(self.row_height));
            let marker = if !focused {
                marker
                    .w(px(cells * self.cell_width))
                    .border_1()
                    .border_color(ink)
            } else if shape == vt100::CursorShape::Underline {
                marker
                    .w(px(cells * self.cell_width))
                    .border_b_1()
                    .border_color(ink)
            } else {
                marker.w(px(1.)).bg(ink)
            };
            body = body.child(marker);
        }
        let (r, c) = screen.cursor_position();
        let left = px(12. + self.cell_width * f32::from(c.min(self.geometry.0 - 1)));
        let top = px(9. + self.row_height * f32::from(r));
        if self.manual() && self.dialog == Dialog::None && self.confirm.is_none() {
            let view = cx.entity();
            let focus = self.focus.clone();
            body = body.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, w, cx| {
                        w.handle_input(&focus, ElementInputHandler::new(bounds, view), cx);
                    },
                )
                .absolute()
                .left(left)
                .top(top)
                .w(px(2.))
                .h(px(self.row_height)),
            );
            if !self.composition.text.is_empty() {
                body = body.child(
                    div()
                        .absolute()
                        .left(left)
                        .top(top)
                        .bg(rgb(t.color("panel")))
                        .child(self.composition.text.clone()),
                );
            }
        }
        if self.drag.is_some() || self.mouse_buttons != 0 {
            let view = cx.entity();
            body = body.child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let entity = view.clone();
                        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                            if phase.capture() && entity.read(cx).mouse_buttons != 0 {
                                if let Some(button) = match e.pressed_button {
                                    Some(MouseButton::Left) => Some(0),
                                    Some(MouseButton::Middle) => Some(1),
                                    Some(MouseButton::Right) => Some(2),
                                    _ => None,
                                } {
                                    entity.update(cx, |v, _| {
                                        v.report_mouse(
                                            e.position,
                                            e.modifiers,
                                            button,
                                            false,
                                            true,
                                        );
                                    });
                                }
                            }
                            if phase.capture() && entity.read(cx).drag.is_some() {
                                entity.update(cx, |v, cx| {
                                    if e.pressed_button == Some(MouseButton::Left) {
                                        v.extend_selection(e.position);
                                    } else {
                                        v.drag = None;
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                });
                            }
                        });
                        let entity = view.clone();
                        window.on_mouse_event(move |e: &MouseUpEvent, phase, w, cx| {
                            if phase.capture()
                                && e.button == MouseButton::Left
                                && entity.read(cx).drag.is_some()
                            {
                                entity.update(cx, |v, cx| {
                                    v.finish_selection(e);
                                    v.release_link(e.position, w, cx);
                                    cx.stop_propagation();
                                    cx.notify();
                                });
                            }
                        });
                    },
                )
                .absolute()
                .size_0(),
            );
        }
        let rows = screen.size().0;
        let history = screen.history_rows();
        if let Some(metrics) =
            crate::scrollbar::Metrics::new(rows, self.row_height, history, screen.scrollback())
        {
            let (opacity, fading) = self.scrollbar.opacity(history, screen.scrollback(), rows);
            if fading {
                w.request_animation_frame();
            }
            body = body.child(self.scroll_track(metrics, opacity, cx));
        } else {
            self.scrollbar.drag = None;
            self.scrollbar.over_thumb = false;
        }
        body
    }
    fn scroll_to_view_start(&mut self, start: usize) {
        let screen = self.parser.screen();
        let offset = screen.history_rows().saturating_sub(start);
        if offset != screen.scrollback() {
            self.parser.set_scrollback(offset);
        }
    }

    fn scroll_track(
        &self,
        metrics: crate::scrollbar::Metrics,
        opacity: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity();
        let mut ink = rgb(self.parser.screen().frame_palette().foreground());
        ink.a = opacity
            * if self.scrollbar.drag.is_some() {
                0.5
            } else if self.scrollbar.over_thumb {
                0.4
            } else {
                0.2
            };
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hit, window, _| {
                let thumb = Bounds::new(
                    point(bounds.left(), bounds.top() + px(metrics.top)),
                    size(bounds.size.width, px(metrics.thumb)),
                );
                window.paint_quad(fill(thumb, ink));
                let track = hit.clone();
                let entity = view.clone();
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || e.button != MouseButton::Left || !track.is_hovered(window)
                    {
                        return;
                    }
                    entity.update(cx, |v, cx| {
                        if v.dialog != Dialog::None || v.confirm.is_some() {
                            return;
                        }
                        let x = f32::from(e.position.x);
                        let y = f32::from(e.position.y);
                        let drag = metrics.grab(x, y, f32::from(bounds.top()));
                        v.scrollbar.drag = Some(drag);
                        v.drag = None;
                        v.scroll_to_view_start(drag.view_start(x, y));
                        cx.stop_propagation();
                        cx.notify();
                    });
                });
                let track = hit.clone();
                let entity = view.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() {
                        return;
                    }
                    let over = track.is_hovered(window) && thumb.contains(&e.position);
                    if let Some(drag) = entity.read(cx).scrollbar.drag {
                        entity.update(cx, |v, cx| {
                            if e.pressed_button == Some(MouseButton::Left) {
                                v.scroll_to_view_start(
                                    drag.view_start(
                                        f32::from(e.position.x),
                                        f32::from(e.position.y),
                                    ),
                                );
                            } else {
                                v.scrollbar.drag = None;
                            }
                            v.scrollbar.over_thumb = over;
                            cx.stop_propagation();
                            cx.notify();
                        });
                    } else if over != entity.read(cx).scrollbar.over_thumb {
                        entity.update(cx, |v, cx| {
                            v.scrollbar.over_thumb = over;
                            cx.notify();
                        });
                    }
                });
                let entity = view.clone();
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if phase.capture()
                        && e.button == MouseButton::Left
                        && entity.read(cx).scrollbar.drag.is_some()
                    {
                        entity.update(cx, |v, cx| {
                            v.scrollbar.drag = None;
                            cx.stop_propagation();
                            cx.notify();
                        });
                    }
                });
            },
        )
        .absolute()
        .right(px(8.))
        .top(px(9.))
        .w(px(14.))
        .h(px(metrics.height))
    }

    fn footer(&self, cx: &Context<Self>) -> Div {
        let t = self.theme;
        let manual = self.manual();
        let hint = if manual {
            "可直接输入 · 交还前请退出全屏程序"
        } else {
            match str_at(&self.state, "state") {
                "disconnected" => "SSH 已断开 · 点击标签 ↻ 重连",
                "busy" => "Agent 正在执行 · 实时输出",
                "synchronizing" => "正在同步终端…",
                _ => "观察模式 · 点击接管后可输入",
            }
        };
        let status = div()
            .flex()
            .items_center()
            .gap(px(7.))
            .min_w_0()
            .child(div().size(px(5.)).flex_none().rounded_full().bg(rgb(t.color(if manual {
                "amber"
            } else {
                "muted"
            }))))
            .child(self.label(hint, "muted", 10.).min_w_0().truncate());
        let mut actions = div().flex().items_center().flex_none().gap(px(5.));
        for (id, key) in [("ctrlc", "Ctrl C"), ("escape", "Esc"), ("tab", "Tab")] {
            actions = actions.child(
                self.btn(id, key, "", manual, cx)
                    .h(px(18.))
                    .px(px(6.))
                    .font_family(mono())
                    .text_size(px(10.)),
            );
        }
        let actions = actions.child(
            self.label(
                format!("{} × {}", self.geometry.0, self.geometry.1),
                "muted",
                10.,
            )
            .font_family(mono())
            .ml(px(6.))
            .min_w(px(63.))
            .text_right(),
        );
        div()
            .h(px(28.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .pl(px(12.))
            .pr(px(10.))
            .border_t_1()
            .border_color(rgb(t.color("border")))
            .child(status)
            .child(actions)
    }
    fn apply_grid(&mut self, fit: TerminalFit, geometry: (u16, u16)) {
        self.cell_width = TerminalFit::cell(fit.advance, geometry.0) as f32;
        self.row_height = TerminalFit::cell(fit.line_height, geometry.1) as f32;
        if geometry != self.geometry {
            self.geometry = geometry;
            self.parser.set_size(geometry.1, geometry.0);
            self.clear_selection();
            self.scrollbar.drag = None;
        }
    }
    fn sync_validation(&mut self, w: &Window, cx: &Context<Self>) {
        let Some(hint) = self.validation.as_mut() else { return };
        let current = self.fields.get(hint.field).map(|field| field.read(cx));
        if !current.is_some_and(|f| f.content == hint.submitted_value && f.focus_handle(cx).is_focused(w)) {
            self.validation = None;
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(bounds) = w.focused_bounds() {
            if self.dialog_scroll.bounds().intersects(&bounds) {
                let anchor = bounds.dilate(px(2.));
                if let Some(popover) = &hint.popover {
                    popover.reposition(w, anchor);
                } else {
                    hint.popover = crate::mac_window::ValidationPopover::show(w, anchor, &hint.message);
                }
            } else if hint.popover.is_some() {
                self.validation = None;
            }
        }
    }

    fn field_ui(&self, i: usize, name: &'static str, w: &Window, cx: &Context<Self>) -> Div {
        let focused = self.fields[i].read(cx).focus_handle(cx).is_focused(w);
        let input = div()
            .relative()
            .mt(px(7.))
            .rounded(px(5.))
            .border_1()
            .border_color(rgb(self.theme.color("border-strong")))
            .when(focused && !cfg!(windows), |input| input.shadow(outline(self.theme)))
            .bg(rgb(self.theme.color("input-bg")))
            .when(focused && cfg!(windows), |input| {
                input.child(div().absolute().inset(px(-5.)).rounded(px(9.)).border_2()
                    .border_color(rgb(self.theme.color("green"))))
            })
            .child(self.fields[i].clone());
        #[cfg(not(target_os = "macos"))]
        let input = if let Some(hint) = self.validation.as_ref().filter(|hint| hint.field == i && focused) {
            input.child(crate::form_validation::bubble(hint.message.clone(), self.theme.palette()["scheme"] == "light"))
        } else { input };
        div().min_w_0()
            .child(self.label(name, "secondary", 12.).line_height(profile_label_height()))
            .child(input)
    }
    fn profile_picker(&self, cx: &Context<Self>) -> Div {
        let t = self.theme;
        let name = if self.profile.is_null() {
            "新建连接"
        } else {
            str_at(&self.profile, "name")
        };
        let mut picker = div().relative().flex_1().min_w_0().child(
            self.btn("choose", "", "", !self.busy, cx)
                .track_focus(&self.chooser_focus)
                .focus(|style| focus_outline(style, t))
                .h(profile_picker_height())
                .w_full()
                .bg(rgb(t.color("input-bg")))
                .border_color(rgb(t.color("border-strong")))
                .text_size(px(12.)).justify_between()
                .child(div().flex_1().min_w_0().truncate().child(name.to_owned()))
                .child(picker_arrow(t)),
        );
        if self.chooser {
            let mut menu = div()
                .id("profiles-menu")
                .occlude()
                .w_full()
                .max_h(px(200.))
                .overflow_y_scroll()
                .track_scroll(&self.profile_scroll)
                .bg(rgb(t.color("input-bg")))
                .border_1()
                .border_color(rgb(t.color("border-strong")));
            for (i, name) in std::iter::once("新建连接")
                .chain(self.profiles.iter().map(|p| str_at(p, "name")))
                .enumerate()
            {
                menu = menu.child(
                    self.btn(format!("profile/{i}"), "", "bare", true, cx)
                        .child(div().min_w_0().truncate().child(name.to_owned()))
                        .w_full()
                        .h(px(26.))
                        .rounded_none()
                        .justify_start()
                        .when(i == self.highlighted_profile, |s| {
                            s.bg(rgb(t.color("hover")))
                        }),
                );
            }
            picker = picker.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(profile_picker_height())
                    .child(deferred(menu).with_priority(2)),
            );
        }
        picker
    }
    fn modal_control(&self, item: Stateful<Div>, enabled: bool) -> Stateful<Div> {
        // GPUI activates focused click handlers on key-up for Enter and Space.
        item.when(enabled && self.confirm.is_none(), |button| {
            button.tab_index(0).focus(|style| {
                style
                    .border_1()
                    .border_color(rgb(self.theme.color("green")))
            })
        })
    }
    fn modal_btn(
        &self,
        id: impl Into<String>,
        label: impl Into<SharedString>,
        kind: &str,
        enabled: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        self.modal_control(self.btn(id, label, kind, enabled, cx), enabled)
    }
    fn modal_checkbox(
        &self,
        id: &str,
        title: &'static str,
        checked: bool,
        enabled: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let t = self.theme;
        self.modal_btn(id.to_owned(), "", "bare", enabled, cx)
            .px_0().h(profile_label_height()).line_height(profile_label_height())
            .gap(px(7.))
            .text_size(px(12.))
            .text_color(rgb(t.color("secondary")))
            .child(
                div()
                    .size(px(if cfg!(windows) { 13. } else { 12. }))
                    .rounded(px(2.))
                    .border_1()
                    .border_color(rgb(t.color(if checked {
                        "green"
                    } else {
                        "border-strong"
                    })))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(checked, |b| {
                        b.bg(rgb(t.color("green")))
                            .text_color(rgb(t.color("on-accent")))
                            .child("✓")
                    }),
            )
            .child(title)
    }
    fn modal_backdrop(theme: Theme) -> impl IntoElement {
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                window.paint_window_backdrop(px(7.), rgba(theme.color("backdrop")));
            },
        )
        .absolute()
        .inset_0()
        .size_full()
    }
    fn dialog_box(
        &self,
        confirm: Option<&Confirm>,
        w: &Window,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let t = self.theme;
        let overlay = div()
            .id(if confirm.is_some() {
                "confirm-backdrop"
            } else {
                "dialog-backdrop"
            })
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .when(!cfg!(any(target_os = "macos", target_os = "windows")), |overlay| {
                overlay.bg(rgba(t.color("backdrop")))
            });
        let mut card = div()
            .id(if confirm.is_some() {
                "confirm-card"
            } else {
                "dialog-card"
            })
            .w(px(490.))
            .max_w(relative(0.90))
            .max_h(relative(0.92))
            .overflow_y_scroll()
            .when(confirm.is_none(), |card| {
                card.track_scroll(&self.dialog_scroll)
            })
            .px(px(30.))
            .py(px(26.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(t.color("border-strong")))
            .bg(rgb(t.color("panel")))
            .flex()
            .flex_col()
            .gap(px(10.));
        if let Some(confirm) = confirm {
            let (title, detail, act) = match confirm {
                Confirm::Close(_, n) => (
                    format!("关闭“{n}”？"),
                    "将关闭此 SSH 会话并移除标签，正在运行的前台任务会中断。已保存的连接配置和密码保留。",
                    "关闭会话",
                ),
                Confirm::Delete(_, n) => (
                    format!("删除“{n}”？"),
                    "将删除此连接配置及其已保存密码，当前 SSH 会话不受影响。",
                    "删除配置",
                ),
            };
            return overlay.child(
                card.w(px(420.)).gap_0()
                    .child(self.label(title, "text", 19.).flex_none().mt(px(9.)).font(heading_font(FontWeight::MEDIUM)).line_height(px(if cfg!(windows) { 25.33 } else { 26.8 })))
                    .child(self.label(detail, "muted", 12.).flex_none().mt(px(12.)).line_height(px(20.4)))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(10.))
                            .mt(px(22.)).flex_none()
                            .child(
                                self.btn("cancel", "取消", "", true, cx)
                                    .min_w(px(76.))
                                    .px(px(14.)).text_size(px(14.)).h(px(if cfg!(windows) {35.33} else {38.1}))
                                    .track_focus(&self.confirm_buttons[0])
                                    .focus(|s| s.border_color(rgb(t.color("text")))),
                            )
                            .child(
                                self.btn("confirm", act, "danger", true, cx)
                                    .min_w(px(76.))
                                    .px(px(14.)).text_size(px(14.)).h(px(if cfg!(windows) {35.33} else {38.1}))
                                    .border_color(rgb(t.color("danger")))
                                    .track_focus(&self.confirm_buttons[1])
                                    .focus(|s| s.border_color(rgb(t.color("text")))),
                            ),
                    ),
            );
        }
        let title = match self.dialog {
            Dialog::Profiles => "连接配置",
            Dialog::Mcp => "桌面与 MCP",
            Dialog::Skins => "选择皮肤",
            Dialog::None => "",
        };
        let profile_dialog = self.dialog == Dialog::Profiles;
        let heading_label = if profile_dialog {
            self.label("SSH CONNECTIONS", "muted", 9.)
                .font_family(mono())
        } else {
            self.label(title, "text", 24.).font(heading_font(FontWeight::MEDIUM))
                .mt(px(9.)).line_height(px(if cfg!(windows) { 32. } else { 34. }))
        };
        let heading = div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .child(heading_label)
            .child(
                self.btn("dismiss", "×", "bare", true, cx)
                    .track_focus(&self.modal_dismiss)
                    .w(px(30.))
                    .h(px(31.))
                    .px_0()
                    .text_color(rgb(t.color("muted")))
                    .text_size(px(24.))
                    .font_weight(FontWeight::NORMAL)
                    .focus(|style| focus_outline(style, t)),
            );
        card = if matches!(self.dialog, Dialog::Mcp | Dialog::Skins) {
            card.gap_0().child(heading.h(px(43.)))
        } else {
            card.gap_0().child(heading.h(px(if cfg!(windows) {32.} else {33.})))
        };
        match self.dialog {
            Dialog::Profiles => {
                let saved = !self.profile.is_null();
                card = card
                    .child(self.label(title, "text", 24.).mt(px(9.)).font(heading_font(FontWeight::MEDIUM)).line_height(px(if cfg!(windows) {32.} else {34.})))
                    .child(self.label("保存常用主机，下次直接选择连接。", "muted", 12.).mt(px(12.)).line_height(px(20.4)))
                    .child(self.label("已保存的连接", "secondary", 12.).mt(px(12.)).line_height(profile_label_height()))
                    .child(
                        div().flex_none().mt(px(7.))
                            .flex()
                            .gap(px(7.))
                            .child(self.profile_picker(cx))
                            .child(self.modal_btn("new", "新建", "", !self.busy, cx).h(profile_picker_height()).text_size(px(12.)).px(px(10.)))
                            .child(
                                self.modal_btn("delete", "删除", "danger", saved && !self.busy, cx)
                                    .h(profile_picker_height()).text_size(px(12.)).px(px(10.)),
                            ),
                    );
                card = card
                    .child(self.field_ui(0, "连接名称", w, cx).flex_none().mt(px(12.)))
                    .child(self.field_ui(1, "主机", w, cx).flex_none().mt(px(12.)))
                    .child(
                        div().flex_none().mt(px(12.))
                            .flex()
                            .gap(px(16.))
                            .child(div().w(px(90.)).child(self.field_ui(2, "端口", w, cx)))
                            .child(self.field_ui(3, "账号", w, cx).flex_1()),
                    )
                    .child(self.field_ui(4, "密码", w, cx).flex_none().mt(px(12.)))
                    .child(
                        div().flex_none().mt(px(12.)).py(px(5.))
                            .flex()
                            .gap(px(18.))
                            .child(self.modal_checkbox(
                                "save-toggle",
                                "保存连接配置",
                                self.save,
                                !self.busy,
                                cx,
                            ))
                            .child(self.modal_checkbox(
                                "remember",
                                "保存密码到本机配置文件",
                                self.remember,
                                self.save && !self.busy,
                                cx,
                            )),
                    )
                    .child(self.label(
                        if self.reusable_password(cx) {
                            "密码已保存在本机配置文件。留空沿用，不会回填到页面。"
                        } else {
                            "密码以明文保存在本机配置文件（权限 0600），不会回填到页面。"
                        },
                        "muted",
                        11.,
                    ).mt(px(9.)).line_height(px(18.7)))
                    .child(self.label(
                        "连接新主机会打开独立标签页；已有主机保持连接和控制权。",
                        "amber",
                        11.,
                    ).mt(px(9.)).line_height(px(18.7)))
                    .child(
                        div().mt(px(8.)).min_h(px(18.)).flex_none()
                            .when(!self.note.is_empty(), |row| {
                                row.child(self.label(self.note.clone(), "danger", 12.).line_height(px(20.4)))
                            }),
                    )
                    .child(
                        div().flex_none().mt(px(8.))
                            .flex()
                            .gap(px(10.))
                            .child(
                                self.modal_btn("save", "保存配置", "", self.save && !self.busy, cx)
                                    .text_size(px(14.)).h(px(if cfg!(windows) {39.33} else {42.1}))
                                    .flex_1(),
                            )
                            .child(
                                self.modal_btn(
                                    "connect",
                                    if self.busy {
                                        "连接中…"
                                    } else {
                                        "连接远端"
                                    },
                                    "primary",
                                    !self.busy,
                                    cx,
                                )
                                .text_size(px(14.)).h(px(if cfg!(windows) {39.33} else {42.1}))
                                .flex_1(),
                            ),
                    );
            }
            Dialog::Mcp => {
                let client_height = px(if cfg!(target_os = "macos") { 22. } else { 28. });
                let para = |text: String| {
                    self.label(text, "muted", 12.)
                        .line_height(px(20.4))
                        .mt(px(12.))
                        .flex_none()
                };
                let mut client = div().relative().child(
                    self.btn(
                        "client",
                        if self.pi { "pi agent" } else { "通用 MCP" },
                        "",
                        true,
                        cx,
                    )
                    .track_focus(&self.chooser_focus)
                    .focus(|style| focus_outline(style, t))
                    .h(client_height)
                    .w(px(89.33))
                    .px(px(5.))
                    .rounded_none()
                    .when(cfg!(target_os = "macos"), |b| {
                        b.w(px(86.)).pl(px(9.)).pr(px(6.)).rounded(px(5.))
                    })
                    .justify_between()
                    .bg(rgb(t.color("input-bg")))
                    .text_color(rgb(t.color("text")))
                    .text_size(px(12.))
                    .child(picker_arrow(t)),
                );
                if self.chooser {
                    let mut menu = div()
                        .id("client-menu")
                        .occlude()
                        .w(px(105.))
                        .py(px(2.))
                        .bg(rgb(t.color("input-bg")))
                        .border_1()
                        .border_color(rgb(t.color("border-strong")));
                    for (pi, id, name) in [(false, "generic", "通用 MCP"), (true, "pi", "pi agent")]
                    {
                        menu = menu.child(
                            self.btn(id, name, "bare", true, cx)
                                .justify_start()
                                .w_full()
                                .rounded_none()
                                .h(px(25.))
                                .text_size(px(12.))
                                .when(pi == self.highlighted_pi, |s| s.bg(rgb(t.color("hover")))),
                        );
                    }
                    client = client.child(
                        div()
                            .absolute()
                            .top(client_height)
                            .left_0()
                            .child(deferred(menu).with_priority(2)),
                    );
                }
                let directory = format!("数据目录：{}", self.dir.display());
                #[cfg(target_os = "macos")]
                let directory = wrap_directory(&directory, (w.viewport_size().width * 0.9).min(px(490.)) - px(62.), w);
                card = card
                    .child(para(if cfg!(windows) {
                        "点击窗口右上角的 × 会最小化。点击工具栏最右侧的「退出」会断开所有 SSH 连接并关闭后台。"
                    } else {
                        "关闭桌面窗口默认最小化；从“垒桥终端”菜单选择“退出垒桥终端与后台”会断开 SSH 并停止后台。"
                    }.into()))
                    .child(para(directory))
                    .child(div().mt(px(12.)).mb(px(12.)).flex_none().flex().items_center().gap(px(10.))
                        .text_size(px(12.)).text_color(rgb(t.color("secondary"))).child("客户端").child(client))
                    .child(self.config_area.clone())
                    .child(self.modal_btn("copy-config", "复制配置", "", true, cx).mt(px(13.)).h(px(if cfg!(windows) { 31.33 } else { 34. })).text_size(px(14.)))
                    .child(para(if self.copied_config { "已复制。合并配置后请重启 MCP 客户端。" } else {
                        "将 baoleiji-pty 条目合并到客户端配置，保留其他服务，然后重启客户端。"
                    }.into()).mb(px(12.)));
            }
            Dialog::Skins => {
                card = card.child(self.label("界面与终端一起换色，即选即用。", "muted", 12.)
                    .mt(px(12.)).line_height(px(20.4)));
                for row in (0..skins().len()).step_by(2) {
                    let mut pair = div().flex().flex_none().gap(px(10.)).mt(px(if row == 0 { 20. } else { 10. }));
                    for i in row..(row + 2).min(skins().len()) {
                        let skin = Theme(i);
                        let val = skin.palette();
                        let item = div()
                            .id(("skin", i))
                            .w(px(209.))
                            .rounded(px(7.))
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgb(if i == t.0 {
                                t.color("green")
                            } else {
                                skin.color("border")
                            }))
                            .when(i == t.0, |card| card.shadow(outline(t)))
                            .bg(rgb(skin.color("bg")))
                            .cursor_pointer()
                            .child(
                                div()
                                    .bg(rgb(skin.color("terminal-bg")))
                                    .p(px(12.))
                                    .font_family(mono())
                                    .text_size(px(10.))
                                    .text_color(rgb(skin.color("text")))
                                    .h(px(61.))
                                    .line_height(px(18.))
                                    .border_b_1()
                                    .border_color(rgb(skin.color("border")))
                                    .child(
                                        div()
                                            .flex()
                                            .child(
                                                div()
                                                    .text_color(rgb(skin.color("green")))
                                                    .child("demo ~ ❯"),
                                            )
                                            .child(" hostname"),
                                    )
                                    .child(div().flex().child("ai-server ").child(
                                        div().text_color(rgb(skin.color("amber"))).child("● ready"),
                                    )),
                            )
                            .child(
                                div()
                                    .px(px(12.))
                                    .h(px(37.))
                                    .items_center()
                                    .flex()
                                    .justify_between()
                                    .text_size(px(12.))
                                    .text_color(rgb(skin.color("text")))
                                    .child(str_at(val, "name").to_string())
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(rgb(skin.color("muted")))
                                            .child(str_at(val, "description").to_string()),
                                    ),
                            )
                            .on_click(cx.listener(move |v, _, w, cx| {
                                if let Err(e) = v.act(&format!("skin/{i}"), w, cx) {
                                    v.note = e.to_string();
                                }
                                cx.notify();
                            }));
                        pair = pair.child(self.modal_control(item, true));
                    }
                    card = card.child(pair);
                }
                card = card.child(
                    self.label(
                        if self.note.is_empty() {
                            "选择会保存在当前应用中。"
                        } else {
                            &self.note
                        }
                        .to_string(),
                        "muted",
                        12.,
                    ).mt(px(12.)).line_height(px(20.4)),
                );
            }
            Dialog::None => {}
        }
        overlay.child(card).when(
            self.chooser && matches!(self.dialog, Dialog::Mcp | Dialog::Profiles),
            |overlay| {
                overlay.child(
                    div().absolute().inset_0().child(
                        deferred(
                            div()
                                .id("chooser-dismiss")
                                .size_full()
                                .occlude()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|v, _, w, cx| {
                                        v.chooser = false;
                                        w.focus(&v.chooser_focus);
                                        cx.stop_propagation();
                                        cx.notify();
                                    }),
                                ),
                        )
                        .with_priority(1),
                    ),
                )
            },
        )
    }
}
// WebKit keeps the ASCII components of a slash-delimited path together. A
// long component that cannot occupy a line by itself may break at a grapheme.
#[cfg(target_os = "macos")]
fn wrap_directory(text: &str, width: Pixels, window: &Window) -> String {
    use icu_segmenter::LineSegmenter;
    use unicode_segmentation::UnicodeSegmentation;
    let style = TextStyle { font_family: "Avenir Next".into(), ..Default::default() };
    let segmenter = LineSegmenter::new_for_non_complex_scripts(Default::default());
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut rest = paragraph;
        loop {
            let shaped = window.text_system().shape_line(
                rest.to_owned().into(), px(12.), &[style.to_run(rest.len())], None,
            );
            let Some(limit) = shaped.index_for_x(width) else {
                lines.push(rest);
                break;
            };
            let legal_break = segmenter.segment_str(rest).take_while(|&i| i <= limit)
                .filter(|&i| i > 0 && !(rest[..i].ends_with('/')
                    && i >= 2 && rest.as_bytes()[i - 2].is_ascii_graphic()
                    && rest.as_bytes().get(i).is_some_and(u8::is_ascii_graphic)))
                .last();
            let cut = legal_break.unwrap_or_else(|| rest.grapheme_indices(true)
                .map(|(i, c)| i + c.len()).take_while(|&i| i <= limit).last()
                .unwrap_or_else(|| rest.graphemes(true).next().unwrap().len()));
            lines.push(&rest[..cut]);
            rest = &rest[cut..];
            if rest.is_empty() { break; }
        }
    }
    lines.join("\n")
}

// Measure with the same native font and wrapping as the visible label. This
// also handles hard line breaks where a single-line ellipsis cannot help.
fn history_preview(command: &str, width: Pixels, window: &Window) -> SharedString {
    use unicode_segmentation::UnicodeSegmentation;
    let normalized = command.replace("\r\n", "\n").replace('\r', "\n");
    let text = normalized.strip_suffix('\n').unwrap_or(&normalized);
    let style = TextStyle {
        font_family: mono().into(),
        ..Default::default()
    };
    let shape = |candidate: SharedString| {
        let run = style.to_run(candidate.len());
        window
            .text_system()
            .shape_text(candidate, px(11.), &[run], Some(width), None)
    };
    let full: SharedString = text.to_owned().into();
    let Ok(lines) = shape(full.clone()) else {
        return full;
    };
    let mut remaining = 3;
    let mut offset = 0;
    let mut visible_end = None;
    for (index, line) in lines.iter().enumerate() {
        if let Some(edge) = line.wrap_boundaries.get(remaining - 1) {
            visible_end = Some(offset + line.runs()[edge.run_ix].glyphs[edge.glyph_ix].index);
            break;
        }
        remaining -= 1 + line.wrap_boundaries.len();
        offset += line.len();
        if remaining == 0 {
            if index + 1 < lines.len() {
                visible_end = Some(offset);
            }
            break;
        }
        offset += 1; // newline between shaped paragraphs
    }
    let Some(end) = visible_end else { return full };
    for (index, _) in text.grapheme_indices(true).rev().filter(|(i, _)| *i <= end) {
        let preview: SharedString = format!("{}…", &text[..index]).into();
        if shape(preview.clone()).is_ok_and(|lines| {
            lines
                .iter()
                .map(|line| 1 + line.wrap_boundaries.len())
                .sum::<usize>()
                <= 3
        }) {
            return preview;
        }
    }
    "…".into()
}

fn heading_font(weight: FontWeight) -> Font {
    let mut face = font(if cfg!(windows) { mono() } else { "Avenir Next" });
    face.weight = weight;
    #[cfg(target_os = "macos")]
    {
        static FALLBACKS: std::sync::OnceLock<FontFallbacks> = std::sync::OnceLock::new();
        face.fallbacks = Some(FALLBACKS.get_or_init(|| {
            FontFallbacks::from_fonts(vec!["PingFang SC".to_string()])
        }).clone());
    }
    face
}

pub(crate) fn mono() -> &'static str {
    if cfg!(windows) {
        "JetBrains Mono"
    } else {
        "Menlo"
    }
}
impl Render for View {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        self.disabled_colors.update_display(w);
        self.sync_validation(w, cx);
        self.fullscreen.sync(w);
        let font_id = cx.text_system().resolve_font(&font(mono()));
        let advance = cx.text_system().advance(font_id, px(13.), 'W')
            .map_or(self.cell_width, |advance| f32::from(advance.width).max(1.));
        let scale = w.scale_factor();
        let measured = f32::from(cx.text_system().ascent(font_id, px(13.)))
            .abs()
            .round()
            + f32::from(cx.text_system().descent(font_id, px(13.)))
                .abs()
                .round();
        let line_height = ((measured * scale).ceil() * 1.3).floor().max(scale) / scale;
        let size = w.viewport_size();
        let width = f32::from(size.width);
        let height = f32::from(size.height);
        if self.history && self.history_text.scroll_drag() {
            w.request_animation_frame();
        }
        let sidebar = if width <= 760. { 230. } else { 280. };
        // 12px left padding, 8px right padding and xterm's 14px scrollbar.
        let avail_width = width
            - 34.
            - if self.fullscreen.active() {
                0.
            } else {
                24. + if self.history { sidebar } else { 0. }
            };
        let avail_height =
            height - 32. - 28. - 12. - if self.fullscreen.active() { 0. } else { 78. };
        let fit = TerminalFit {
            width: avail_width,
            height: avail_height,
            advance,
            line_height,
            history_open: self.history,
        };
        if self.last_fit != Some(fit) {
            let previous = self.last_fit.map_or((80, 24), |_| self.geometry);
            let mut geometry = fit.grid(previous);
            // Opening a tab or toggling history fits explicitly as well as through ResizeObserver.
            if self.last_fit.is_none_or(|last| last.history_open != fit.history_open) {
                geometry = fit.grid(geometry);
            }
            self.last_fit = Some(fit);
            self.apply_grid(fit, geometry);
        }
        let geometry = self.geometry;
        if !self.selected.is_empty()
            && self.state["state"] != "disconnected"
            && self.sent_size.as_ref() != Some(&(self.selected.clone(), geometry.0, geometry.1))
        {
            self.queue(
                "resize",
                json!({"session_id":self.selected,"cols":geometry.0,"rows":geometry.1}),
            );
            self.sent_size = Some((self.selected.clone(), geometry.0, geometry.1));
        }
        #[cfg(target_os = "macos")]
        if let Some(menu) = cx.try_global::<crate::menu::NativeMenu>() {
            menu.sync(|command| self.menu_allows(command, w, cx));
        }
        let t = self.theme;
        let mut root = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(t.color("bg")))
            .text_color(rgb(t.color("text")))
            .text_size(px(13.))
            .font_family(if cfg!(windows) { mono() } else { "Avenir Next" })
            .id("terminal-page")
            .tab_group()
            .track_focus(&self.page_focus)
            .capture_any_mouse_down(cx.listener(|v, _, _, cx| {
                if v.keyboard_focus {
                    v.keyboard_focus = false;
                    cx.notify();
                }
            }))
            .capture_key_down(cx.listener(|v, _: &KeyDownEvent, _, cx| {
                if !v.keyboard_focus {
                    v.keyboard_focus = true;
                    cx.notify();
                }
            }))
            .on_mouse_move(cx.listener(|v, e: &MouseMoveEvent, _, cx| {
                if v.history_text.dragging {
                    if e.pressed_button == Some(MouseButton::Left) {
                        v.history_text.drag(e.position)
                    } else {
                        v.history_text.dragging = false
                    }
                    cx.notify();
                }
            }))
            .on_action(cx.listener(Self::open_clipboard))
            .on_key_down(cx.listener(Self::key));
        let mut workspace = div()
            .relative()
            .min_w_0()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(t.color("terminal-bg")));
        if !self.fullscreen.active() {
            root = root.child(self.header(width, cx));
            workspace = workspace.child(self.tabs(cx));
        }
        let target = &self.state["target"];
        let (account, route) = if target.is_object() {
            (
                format!("SSH · {}", str_at(target, "user")),
                format!("{}:{}", str_at(target, "host"), target["port"]),
            )
        } else {
            ("SSH".into(), "尚未连接".into())
        };
        let narrow = width <= 760.;
        let title = div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(if narrow { 7. } else { 10. }))
            .font_family(mono())
            .child(
                self.label(account, "secondary", 11.)
                    .flex_none()
                    .truncate()
                    .when(narrow, |d| d.max_w(px(145.))),
            )
            .child(self.label("/", "muted", 11.).flex_none())
            .child(
                self.label(route.clone(), "secondary", 11.)
                    .min_w_0()
                    .truncate()
                    .id("terminal-route")
                    .when(target.is_object(), |d| {
                        d.when(self.workspace_interactive(), |item| {
                            item.tooltip(hover_tip(
                                format!("{}@{}", str_at(target, "user"), route),
                                t,
                            ))
                        })
                    }),
            );
        let mut actions =
            div()
                .flex()
                .items_center()
                .flex_none()
                .gap(px(if narrow { 0. } else { 4. }));
        for (id, label) in [("copy", "复制"), ("clear", "清屏"), ("full", "⛶")] {
            actions = actions.child(
                self.btn(id, label, "bare", true, cx)
                    .px(px(7.))
                    .h(px(22.))
                    .text_color(rgb(t.color("secondary"))),
            );
        }
        workspace = workspace
            .child(
                self.bar(32.)
                    .bg(rgb(t.color("panel")))
                    .px(px(12.))
                    .gap(px(12.))
                    .justify_between()
                    .child(title)
                    .child(actions),
            )
            .child(self.screen(w, cx))
            .child(self.footer(cx));
        if !self.note.is_empty() && self.dialog == Dialog::None {
            workspace = workspace.child(
                self.label(self.note.clone(), "amber", 12.)
                    .absolute()
                    .bottom(px(35.))
                    .left(px(12.))
                    .max_w(px((avail_width + 11.).max(0.)))
                    .bg(rgb(t.color("panel")))
                    .border_1()
                    .border_color(rgb(t.color("border-strong")))
                    .rounded(px(5.))
                    .py(px(9.))
                    .px(px(12.)),
            );
        }
        let mut panes = div().flex().min_h_0().flex_1().child(workspace);
        if !self.fullscreen.active() {
            panes = panes.child(self.sidebar(sidebar, w, cx));
        }
        root = root.child(panes);
        if self.dialog != Dialog::None {
            root = root
                .child(Self::modal_backdrop(t))
                .child(self.dialog_box(None, w, cx));
        }
        if let Some(confirm) = &self.confirm {
            root = root
                .child(Self::modal_backdrop(t))
                .child(self.dialog_box(Some(confirm), w, cx));
        }
        if let Some(menu) = &self.clipboard_popup {
            root = root.child(self.clipboard_overlay(menu, w, cx));
        }
        root
    }
}
impl EntityInputHandler for View {
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.composition.selection.clone(),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.composition.text.is_empty()).then(|| 0..self.composition.len16())
    }
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let (text, range) = self.composition.slice(range);
        *actual = Some(range);
        Some(text)
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composition.replace(range, text, None);
        let committed = self.composition.finish();
        self.input(&committed);
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composition.replace(range, text, selection);
        cx.notify();
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let committed = self.composition.finish();
        self.input(&committed);
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
