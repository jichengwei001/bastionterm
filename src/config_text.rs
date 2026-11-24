//! Native read-only, vertically resizable text area for the MCP configuration.
use crate::theme::Theme;
use gpui::{prelude::*, *};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub(crate) struct ConfigText {
    value: SharedString,
    focus: FocusHandle,
    scroll: ScrollHandle,
    layout: Option<TextLayout>,
    anchor: usize,
    caret: usize,
    height: Pixels,
    selecting: bool,
    resize: Option<(Pixels, Pixels)>,
    theme: Theme,
}

impl ConfigText {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            value: "".into(),
            focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            layout: None,
            anchor: 0,
            caret: 0,
            height: px(190.),
            selecting: false,
            resize: None,
            theme: Theme(0),
        }
    }

    pub(crate) fn show(&mut self, value: String, theme: Theme, cx: &mut Context<Self>) {
        if self.value.as_ref() != value {
            self.value = value.into();
            self.anchor = 0;
            self.caret = 0;
            self.scroll.set_offset(Point::default());
            self.layout = None;
        }
        self.selecting = false;
        self.resize = None;
        self.theme = theme;
        cx.notify();
    }

    pub(crate) fn focused(&self, w: &Window) -> bool {
        self.focus.is_focused(w)
    }

    pub(crate) fn can_copy(&self) -> bool {
        !self.selection().is_empty()
    }

    pub(crate) fn copy(&self, cx: &mut Context<Self>) {
        if self.can_copy() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.value[self.selection()].to_owned(),
            ));
        }
    }

    pub(crate) fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.value.len();
        self.reveal();
    }

    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    fn hit(&self, point: Point<Pixels>) -> usize {
        self.layout.as_ref().map_or(0, |layout| {
            layout
                .index_for_position(point)
                .unwrap_or_else(|edge| edge)
                .min(self.value.len())
        })
    }

    fn reveal(&self) {
        let Some(pos) = self
            .layout
            .as_ref()
            .and_then(|l| l.position_for_index(self.caret))
        else {
            return;
        };
        let bounds = self.scroll.bounds();
        let mut offset = self.scroll.offset();
        if pos.y < bounds.top() + px(8.) {
            offset.y += bounds.top() + px(8.) - pos.y;
        } else if pos.y + px(20.) > bounds.bottom() {
            offset.y -= pos.y + px(20.) - bounds.bottom();
        }
        offset.y = offset.y.clamp(-self.scroll.max_offset().height, px(0.));
        self.scroll.set_offset(offset);
    }

    fn key(&mut self, event: &KeyDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let key = k.key.as_str();
        let command = if cfg!(target_os = "macos") {
            k.modifiers.platform
        } else {
            k.modifiers.control
        };
        let words = if cfg!(target_os = "macos") {
            k.modifiers.alt
        } else {
            k.modifiers.control
        };
        let selection = self.selection();
        let pos = self.caret;
        let shift = k.modifiers.shift;
        match key {
            "c" if command => self.copy(cx),
            "insert" if cfg!(windows) && k.modifiers == Modifiers::control() => self.copy(cx),
            "a" if command => self.select_all(),
            "left" | "right" | "up" | "down" | "home" | "end" | "pageup" | "pagedown" => {
                let next = match key {
                    "home" | "up" if command => 0,
                    "end" | "down" if command => self.value.len(),
                    "left" | "right" if !shift && !selection.is_empty() && !words && !command => {
                        if key == "left" {
                            selection.start
                        } else {
                            selection.end
                        }
                    }
                    "left" if words => self.value[..pos]
                        .unicode_word_indices()
                        .next_back()
                        .map_or(0, |(i, _)| i),
                    "right" if words => self.value[pos..]
                        .unicode_word_indices()
                        .next()
                        .map_or(self.value.len(), |(i, s)| pos + i + s.len()),
                    "home" | "left" if key == "home" || command => {
                        self.value[..pos].rfind('\n').map_or(0, |i| i + 1)
                    }
                    "end" | "right" if key == "end" || command => self.value[pos..]
                        .find('\n')
                        .map_or(self.value.len(), |i| pos + i),
                    "left" => self.value[..pos]
                        .grapheme_indices(true)
                        .next_back()
                        .map_or(0, |(i, _)| i),
                    "right" => self.value[pos..]
                        .graphemes(true)
                        .next()
                        .map_or(pos, |s| pos + s.len()),
                    _ => {
                        let Some(layout) = &self.layout else { return };
                        let Some(mut point) = layout.position_for_index(pos) else {
                            return;
                        };
                        let line = layout.line_height();
                        let step = if matches!(key, "pageup" | "pagedown") {
                            (self.scroll.bounds().size.height / line).floor().max(1.)
                        } else {
                            1.
                        };
                        point.y += line
                            * (0.5
                                + if matches!(key, "up" | "pageup") {
                                    -step
                                } else {
                                    step
                                });
                        self.hit(point)
                    }
                };
                self.caret = next;
                if !shift {
                    self.anchor = next;
                }
                self.reveal();
            }
            _ => return,
        }
        w.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        w.focus(&self.focus);
        let i = self.hit(event.position);
        self.selecting = true;
        if event.modifiers.shift {
            self.caret = i;
        } else if event.click_count == 2 {
            let range = self
                .value
                .split_word_bound_indices()
                .find(|(start, word)| *start + word.len() > i)
                .map_or(i..i, |(start, word)| start..start + word.len());
            self.anchor = range.start;
            self.caret = range.end;
        } else if event.click_count >= 3 {
            self.anchor = self.value[..i].rfind('\n').map_or(0, |p| p + 1);
            self.caret = self.value[i..]
                .find('\n')
                .map_or(self.value.len(), |p| i + p + 1);
        } else {
            self.anchor = i;
            self.caret = i;
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn drag_to(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        if e.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some((start_y, start_height)) = self.resize {
            self.height = (start_height + e.position.y - start_y).max(px(24.));
        } else if self.selecting {
            self.caret = self.hit(e.position);
            self.reveal();
        } else {
            return;
        }
        cx.notify();
    }
}

impl Render for ConfigText {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let selection = self.selection();
        let text = StyledText::new(self.value.clone()).with_highlights(
            (!selection.is_empty()).then(|| {
                (
                    selection,
                    HighlightStyle {
                        background_color: Some(rgba(0x4c91ee80).into()),
                        ..Default::default()
                    },
                )
            }),
        );
        self.layout = Some(text.layout().clone());
        let view = cx.entity();
        let grip = canvas(
            |_, _, _| (),
            move |b, _, w, _| {
                let mut path = PathBuilder::stroke(px(1.));
                let strokes = if cfg!(target_os = "macos") { [7., 11.] } else { [4., 7.] };
                for inset in strokes {
                    path.move_to(point(b.right() - px(inset), b.bottom() - px(2.)));
                    path.line_to(point(b.right() - px(2.), b.bottom() - px(inset)));
                }
                if let Ok(path) = path.build() {
                    w.paint_path(path, rgb(t.color("muted")));
                }
                let v = view.clone();
                w.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if phase.capture() && (v.read(cx).selecting || v.read(cx).resize.is_some()) {
                        v.update(cx, |v, cx| v.drag_to(e, cx));
                    }
                });
                let v = view.clone();
                w.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if phase.capture() && e.button == MouseButton::Left {
                        v.update(cx, |v, _| {
                            v.resize = None;
                            v.selecting = false;
                        });
                    }
                });
            },
        )
        .size_full();
        div()
            .id("config-text")
            .relative()
            .flex_none()
            .w_full()
            .h(self.height)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .border_1()
            .border_color(rgb(t.color("border")))
            .bg(rgb(t.color("input-bg")))
            .focus(|s| s.border_color(rgb(t.color("text"))))
            .child(
                div()
                    .id("config-text-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|area, e: &MouseDownEvent, w, cx| {
                            w.focus(&area.focus);
                            let i = area.hit(e.position);
                            if !area.selection().contains(&i) {
                                let range = if cfg!(target_os = "macos") {
                                    crate::field_words::at(&area.value, i, false)
                                } else {
                                    i..i
                                };
                                area.anchor = range.start;
                                area.caret = range.end;
                            }
                            w.dispatch_action(
                                Box::new(crate::menu::OpenClipboardMenu {
                                    position: e.position,
                                }),
                                cx,
                            );
                            w.prevent_default();
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .cursor(CursorStyle::IBeam)
                    .p(px(10.))
                    .font_family(super::view::mono())
                    .text_size(px(11.))
                    .line_height(px(17.6))
                    .child(text),
            )
            .child(
                div()
                    .absolute()
                    .right_0()
                    .bottom_0()
                    .size(px(13.))
                    .cursor(CursorStyle::ResizeUpDown)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e: &MouseDownEvent, w, cx| {
                            w.focus(&this.focus);
                            this.selecting = false;
                            this.resize = Some((e.position.y, this.height));
                            cx.stop_propagation();
                        }),
                    )
                    .child(grip),
            )
    }
}
