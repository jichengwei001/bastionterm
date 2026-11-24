// Adapted from GPUI 0.2.2 examples/input.rs, Apache-2.0.
use std::ops::Range;

use crate::field_history::{Edit, FieldHistory, FieldState};
use gpui::*;
use unicode_segmentation::*;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Undo,
        Redo,
        SelectHome,
        SelectEnd,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToStart,
    ]
);

pub struct TextInput {
    secret: bool,
    port: bool,
    name: bool,
    focus_handle: FocusHandle,
    pub content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    history: FieldHistory,
    composition_start: Option<FieldState>,
    blur: Option<Subscription>,
    pan: Pixels,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    drag_word: Option<Range<usize>>,
}

// HTML number inputs snap fractions in the direction of travel. Out-of-range
// values only move when the chosen direction takes them back into the interval.
fn stepped_port(value: &str, up: bool) -> Option<u16> {
    let Some(value) = value.parse::<f64>().ok().filter(|n| n.is_finite()) else {
        return Some(1);
    };
    let next = if up {
        value.floor() + 1.
    } else {
        value.ceil() - 1.
    };
    if (up && value >= 65535.) || (!up && value <= 1.) {
        return None;
    }
    Some(next.clamp(1., 65535.) as u16)
}

fn limit_utf16(text: &str, mut available: usize) -> &str {
    for (offset, ch) in text.char_indices() {
        if ch.len_utf16() > available {
            return &text[..offset];
        }
        available -= ch.len_utf16();
    }
    text
}

impl TextInput {
    fn step_port(&mut self, up: bool, cx: &mut Context<Self>) {
        if let Some(port) = stepped_port(&self.content, up) {
            self.history = FieldHistory::default();
            self.composition_start = None;
            self.content = port.to_string().into();
            self.marked_range = None;
            self.move_to(self.content.len(), cx);
        }
    }

    fn inserted_text<'a>(&self, range: &Range<usize>, text: &'a str) -> &'a str {
        if self.name {
            let retained = self.content[..range.start].encode_utf16().count()
                + self.content[range.end..].encode_utf16().count();
            return limit_utf16(text, 120usize.saturating_sub(retained));
        }
        text
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn state(&self) -> FieldState {
        FieldState {
            text: self.content.clone(),
            selection: self.selected_range.clone(),
            reversed: self.selection_reversed && !self.selected_range.is_empty(),
        }
    }

    fn restore(&mut self, state: FieldState, cx: &mut Context<Self>) {
        self.content = state.text;
        self.selected_range = state.selection;
        self.selection_reversed = state.reversed;
        self.marked_range = None;
        cx.notify();
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_composition();
        if let Some(state) = self.history.undo() {
            self.restore(state, cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_composition();
        if let Some(state) = self.history.redo() {
            self.restore(state, cx);
        }
    }

    fn finish_composition(&mut self) {
        self.marked_range = None;
        if let Some(before) = self.composition_start.take() {
            self.history.record(before, self.state(), Edit::Separate);
        }
    }

    fn replace(&mut self, range: Range<usize>, text: &str, kind: Edit, cx: &mut Context<Self>) {
        if !self.accepts(text) {
            return;
        }
        let before = self.composition_start.take();
        let kind = if before.is_some() {
            Edit::Separate
        } else {
            kind
        };
        let before = before.unwrap_or_else(|| self.state());
        let end = self.splice(&range, text);
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.history.record(before, self.state(), kind);
        cx.notify();
    }

    fn accepts(&self, text: &str) -> bool {
        !self.port
            || text
                .bytes()
                .all(|b| b.is_ascii_digit() || b".-+eE".contains(&b))
    }

    fn splice(&mut self, range: &Range<usize>, text: &str) -> usize {
        let text = self.inserted_text(range, text);
        let end = range.start + text.len();
        self.content =
            (self.content[..range.start].to_owned() + text + &self.content[range.end..]).into();
        end
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let mut range = self.selected_range.clone();
        if range.is_empty() {
            range.start = self.previous_boundary(range.start);
        }
        self.replace(range, "", Edit::Backspace, cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let mut range = self.selected_range.clone();
        if range.is_empty() {
            range.end = self.next_boundary(range.end);
        }
        self.replace(range, "", Edit::Delete, cx);
    }

    fn word_offset(&self, right: bool) -> usize {
        crate::field_words::step(&self.content, self.cursor_offset(), right, self.secret)
    }

    fn delete_until(&mut self, target: usize, cx: &mut Context<Self>) {
        let mut range = self.selected_range.clone();
        if range.is_empty() {
            range = target.min(range.start)..target.max(range.end);
        }
        self.replace(range, "", Edit::Separate, cx);
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        _window.focus(&self.focus_handle);
        self.is_selecting = true;

        self.drag_word = None;
        let offset = self.index_for_mouse_position(event.position, event.click_count > 1);
        if event.click_count >= 3 {
            self.select_all(&SelectAll, _window, cx);
        } else if event.click_count == 2 {
            let word = crate::field_words::at(&self.content, offset, self.secret);
            self.move_to(word.start, cx);
            self.select_to(word.end, cx);
            self.drag_word = Some(word);
        } else if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn context_menu(&mut self, e: &MouseDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        w.focus(&self.focus_handle);
        self.finish_composition();
        self.is_selecting = false;
        self.drag_word = None;
        let offset = self.index_for_mouse_position(e.position, true);
        if !self.selected_range.contains(&offset) {
            if cfg!(target_os = "macos") {
                let word = crate::field_words::at(
                    &self.content,
                    if offset == self.content.len() {
                        self.previous_boundary(offset)
                    } else {
                        offset
                    },
                    self.secret,
                );
                self.move_to(word.start, cx);
                self.select_to(word.end, cx);
            } else {
                self.move_to(offset, cx);
            }
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
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
        self.drag_word = None;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.is_selecting {
            return;
        }
        let offset = self.index_for_mouse_position(event.position, self.drag_word.is_some());
        if let Some(anchor) = self.drag_word.clone() {
            let target = crate::field_words::at(&self.content, offset, self.secret);
            if target.start < anchor.start {
                self.move_to(anchor.end, cx);
                self.select_to(target.start, cx);
            } else {
                self.move_to(anchor.start, cx);
                self.select_to(target.end, cx);
            }
        } else {
            self.select_to(offset, cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace(
                self.selected_range.clone(),
                &text.replace(['\r', '\n'], ""),
                Edit::Separate,
                cx,
            );
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.secret && !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.secret && !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace(self.selected_range.clone(), "", Edit::Separate, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.history.end_group();
        self.selection_reversed = false;
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>, under_pointer: bool) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        let x = position.x - bounds.left() + self.pan;
        if under_pointer {
            line.index_for_x(x)
                .map_or(self.content.len(), |i| self.source_index(i))
        } else {
            self.source_index(line.closest_index_for_x(x))
        }
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.history.end_group();
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    pub fn new(field: &str, value: String, placeholder: &str, cx: &mut Context<Self>) -> Self {
        let end = value.len();
        Self {
            secret: field == "password",
            port: field == "port",
            name: field == "name",
            focus_handle: cx.focus_handle().tab_stop(true),
            content: value.into(),
            placeholder: placeholder.to_owned().into(),
            selected_range: end..end,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            pan: px(0.),
            last_bounds: None,
            is_selecting: false,
            drag_word: None,
            history: Default::default(),
            composition_start: None,
            blur: None,
        }
    }
    pub fn load_value(&mut self, text: String, reset_history: bool, cx: &mut Context<Self>) {
        if !reset_history && self.content.as_ref() == text {
            return;
        }
        self.history = Default::default();
        self.composition_start = None;
        self.marked_range = None;
        self.pan = px(0.);
        self.drag_word = None;
        self.is_selecting = false;
        self.content = text.into();
        self.move_to(self.content.len(), cx);
    }

    pub fn clear_secret(&mut self, cx: &mut Context<Self>) {
        debug_assert!(self.secret);
        self.load_value(String::new(), true, cx);
    }
    pub fn set_placeholder(&mut self, value: &'static str, cx: &mut Context<Self>) {
        if self.placeholder.as_ref() != value {
            self.placeholder = value.into();
            cx.notify();
        }
    }
    pub fn can_edit(&self, command: crate::menu::EditCommand) -> bool {
        use crate::menu::EditCommand::*;
        match command {
            Undo => self.composing() || self.history.can_undo(),
            Redo => self.history.can_redo(),
            Copy | Cut => !self.secret && !self.selected_range.is_empty(),
            Paste => true,
            SelectAll => !self.content.is_empty(),
        }
    }
    pub fn composing(&self) -> bool {
        self.marked_range.is_some()
    }
    fn display_index(&self, i: usize) -> usize {
        if self.secret {
            self.content[..i].chars().count() * "•".len()
        } else {
            i
        }
    }
    fn source_index(&self, i: usize) -> usize {
        if self.secret {
            self.content
                .char_indices()
                .nth(i / "•".len())
                .map(|(n, _)| n)
                .unwrap_or(self.content.len())
        } else {
            self.content.floor_char_boundary(i.min(self.content.len()))
        }
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.finish_composition();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.replace(range, new_text, Edit::Typing, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        if !self.accepts(new_text) {
            return;
        }
        if self.composition_start.is_none() {
            self.composition_start = Some(self.state());
        }
        let end = self.splice(&range, new_text);
        self.marked_range = (end > range.start).then_some(range.start..end);
        let new_text = &self.content[range.start..end];
        let relative_byte = |n: usize| {
            let mut units = 0;
            for (i, c) in new_text.char_indices() {
                if units >= n {
                    return i;
                }
                units += c.len_utf16();
            }
            new_text.len()
        };
        self.selected_range = new_selected_range_utf16
            .map(|r| range.start + relative_byte(r.start)..range.start + relative_byte(r.end))
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        self.selection_reversed = false;
        if self.marked_range.is_none() {
            self.finish_composition();
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let range = self.display_index(range.start)..self.display_index(range.end);
        Some(Bounds::from_corners(
            point(
                bounds.left() - self.pan + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() - self.pan + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;

        let utf8_index = last_layout.closest_index_for_x(line_point.x + self.pan);
        Some(self.offset_to_utf16(self.source_index(utf8_index)))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    pan: Pixels,
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.display_index(input.selected_range.start)
            ..input.display_index(input.selected_range.end);
        let cursor = input.display_index(input.cursor_offset());
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), style.color.opacity(0.6))
        } else {
            (
                if input.secret {
                    "•".repeat(content.chars().count()).into()
                } else {
                    content
                },
                style.color,
            )
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            let marked_range =
                input.display_index(marked_range.start)..input.display_index(marked_range.end);
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        let pan = input
            .pan
            .min(cursor_pos)
            .max(cursor_pos - bounds.size.width + px(2.))
            .max(px(0.));
        let left = bounds.left() - pan;
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(left + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    style.color,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(left + line.x_for_index(selected_range.start), bounds.top()),
                        point(left + line.x_for_index(selected_range.end), bounds.bottom()),
                    ),
                    rgba(0x3311ff30),
                )),
                None,
            )
        };
        PrepaintState {
            pan,
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let line = prepaint.line.take().unwrap();
        line.paint(
            point(bounds.origin.x - prepaint.pan, bounds.origin.y),
            window.line_height(),
            window,
            cx,
        )
        .unwrap();

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.pan = prepaint.pan;
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.blur.is_none() {
            self.blur = Some(cx.on_blur(&self.focus_handle, window, |field, _, cx| {
                field.finish_composition();
                field.history.end_group();
                cx.notify();
            }));
        }
        let mut input = div()
            .group("profile-input")
            .items_center()
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(|f, _: &WordLeft, _, cx| f.move_to(f.word_offset(false), cx)))
            .on_action(cx.listener(|f, _: &WordRight, _, cx| f.move_to(f.word_offset(true), cx)))
            .on_action(
                cx.listener(|f, _: &SelectWordLeft, _, cx| f.select_to(f.word_offset(false), cx)),
            )
            .on_action(
                cx.listener(|f, _: &SelectWordRight, _, cx| f.select_to(f.word_offset(true), cx)),
            )
            .on_action(
                cx.listener(|f, _: &DeleteWordLeft, _, cx| {
                    f.delete_until(f.word_offset(false), cx)
                }),
            )
            .on_action(
                cx.listener(|f, _: &DeleteWordRight, _, cx| {
                    f.delete_until(f.word_offset(true), cx)
                }),
            )
            .on_action(cx.listener(|f, _: &DeleteToStart, _, cx| f.delete_until(0, cx)))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(|field, _: &SelectHome, _, cx| field.select_to(0, cx)))
            .on_action(
                cx.listener(|field, _: &SelectEnd, _, cx| field.select_to(field.content.len(), cx)),
            )
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::context_menu))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .overflow_hidden()
            .px(px(10.))
            .py(px(10.))
            .line_height(px(if cfg!(windows) { 15.33 } else { 16. }))
            .text_size(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(TextElement { input: cx.entity() }),
            );
        if self.port {
            let arrow_height = px(if cfg!(windows) { 15.33 } else { 16. }) / 2.;
            let mut arrows = div().flex_none().w(px(14.)).flex().flex_col();
            if !self.focus_handle.is_focused(window) {
                arrows = arrows
                    .opacity(0.)
                    .group_hover("profile-input", |s| s.opacity(1.));
            }
            for (label, up) in [("▴", true), ("▾", false)] {
                arrows = arrows.child(
                    div()
                        .text_center()
                        .h(arrow_height)
                        .line_height(arrow_height)
                        .text_size(px(10.))
                        .cursor_default()
                        .child(label)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |field, _, w, cx| {
                                cx.stop_propagation();
                                w.focus(&field.focus_handle);
                                field.step_port(up, cx);
                            }),
                        ),
                );
            }
            input = input
                .on_key_down(cx.listener(|field, e: &KeyDownEvent, _, cx| {
                    if !field.composing()
                        && !e.keystroke.modifiers.modified()
                        && matches!(e.keystroke.key.as_str(), "up" | "down")
                    {
                        field.step_port(e.keystroke.key == "up", cx);
                        cx.stop_propagation();
                    }
                }))
                .child(arrows);
        }
        input
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

pub fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new("shift-home", SelectHome, Some("TextInput")),
        KeyBinding::new("shift-end", SelectEnd, Some("TextInput")),
    ]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("alt-left", WordLeft, Some("TextInput")),
        KeyBinding::new("alt-right", WordRight, Some("TextInput")),
        KeyBinding::new("alt-shift-left", SelectWordLeft, Some("TextInput")),
        KeyBinding::new("alt-shift-right", SelectWordRight, Some("TextInput")),
        KeyBinding::new("alt-backspace", DeleteWordLeft, Some("TextInput")),
        KeyBinding::new("alt-delete", DeleteWordRight, Some("TextInput")),
        KeyBinding::new("cmd-backspace", DeleteToStart, Some("TextInput")),
        KeyBinding::new("cmd-left", Home, Some("TextInput")),
        KeyBinding::new("cmd-right", End, Some("TextInput")),
        KeyBinding::new("cmd-shift-left", SelectHome, Some("TextInput")),
        KeyBinding::new("cmd-shift-right", SelectEnd, Some("TextInput")),
        KeyBinding::new("ctrl-a", Home, Some("TextInput")),
        KeyBinding::new("ctrl-e", End, Some("TextInput")),
        KeyBinding::new("cmd-a", SelectAll, Some("TextInput")),
        KeyBinding::new("cmd-v", Paste, Some("TextInput")),
        KeyBinding::new("cmd-c", Copy, Some("TextInput")),
        KeyBinding::new("cmd-x", Cut, Some("TextInput")),
        KeyBinding::new("cmd-z", Undo, Some("TextInput")),
        KeyBinding::new("cmd-shift-z", Redo, Some("TextInput")),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("TextInput")),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-left", WordLeft, Some("TextInput")),
        KeyBinding::new("ctrl-right", WordRight, Some("TextInput")),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, Some("TextInput")),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, Some("TextInput")),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, Some("TextInput")),
        KeyBinding::new("ctrl-delete", DeleteWordRight, Some("TextInput")),
        KeyBinding::new("ctrl-a", SelectAll, Some("TextInput")),
        KeyBinding::new("ctrl-c", Copy, Some("TextInput")),
        KeyBinding::new("ctrl-x", Cut, Some("TextInput")),
        KeyBinding::new("ctrl-v", Paste, Some("TextInput")),
        KeyBinding::new("ctrl-z", Undo, Some("TextInput")),
        KeyBinding::new("ctrl-y", Redo, Some("TextInput")),
        KeyBinding::new("ctrl-shift-z", Redo, Some("TextInput")),
        KeyBinding::new("shift-delete", Cut, Some("TextInput")),
        KeyBinding::new("ctrl-insert", Copy, Some("TextInput")),
        KeyBinding::new("shift-insert", Paste, Some("TextInput")),
    ]);
}

#[cfg(test)]
mod tests {
    use super::{limit_utf16, stepped_port};
    #[test]
    fn port_steps_follow_bounded_number_input() {
        for (text, up, down) in [
            ("", Some(1), Some(1)),
            ("0", Some(1), None),
            ("1", Some(2), None),
            ("65535", None, Some(65534)),
            ("65536", None, Some(65535)),
            ("22.4", Some(23), Some(22)),
            ("2e3", Some(2001), Some(1999)),
        ] {
            assert_eq!(
                (stepped_port(text, true), stepped_port(text, false)),
                (up, down)
            );
        }
    }

    #[test]
    fn name_limit_preserves_complete_utf16_characters() {
        assert_eq!(limit_utf16("中🧪A", 2), "中");
        assert_eq!(limit_utf16("中🧪A", 3), "中🧪");
    }
}
