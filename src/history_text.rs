//! Text selection in the execution log, in the same reading order as the web view.
use gpui::{
    Div, HighlightStyle, ParentElement, Pixels, Point, ScrollHandle, SharedString, Styled,
    StyledText, TextLayout, px, rgb, rgba,
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Position {
    pub part: usize,
    pub byte: usize,
}

struct Part {
    full: SharedString,
    shown: SharedString,
    layout: TextLayout,
}

#[derive(Default)]
pub(crate) struct HistoryText {
    parts: Vec<Part>,
    anchor: Position,
    head: Position,
    pub dragging: bool,
    pointer: Option<Point<Pixels>>,
    pub scroll: ScrollHandle,
}

impl HistoryText {
    pub fn clear_selection(&mut self) {
        self.anchor = Position::default();
        self.head = self.anchor;
        self.dragging = false;
        self.pointer = None;
    }
    pub fn begin_frame(&mut self) {
        self.parts.clear();
    }

    pub fn span(&mut self, full: SharedString, shown: SharedString, color: u32, size: f32) -> Div {
        let range = self.range(self.parts.len(), full.len());
        let prefix = shown.strip_suffix('…').filter(|_| full != shown);
        let display_byte = |byte: usize| {
            if prefix.is_some_and(|p| byte > p.len()) {
                shown.len()
            } else {
                byte.min(shown.len())
            }
        };
        let start = display_byte(range.start);
        let end = display_byte(range.end);
        let text = StyledText::new(shown.clone()).with_highlights((start < end).then(|| {
            (
                start..end,
                HighlightStyle {
                    background_color: Some(rgba(0x4c91ee80).into()),
                    ..Default::default()
                },
            )
        }));
        self.parts.push(Part {
            full,
            shown,
            layout: text.layout().clone(),
        });
        gpui::div()
            .text_size(px(size))
            .text_color(rgb(color))
            .cursor_text()
            .child(text)
    }

    pub fn nearest(&self, point: Point<Pixels>) -> Option<Position> {
        let (i, part) = self.parts.iter().enumerate().min_by_key(|(_, part)| {
            let b = part.layout.bounds();
            let dx = (b.left() - point.x).max(point.x - b.right()).max(px(0.));
            let dy = (b.top() - point.y).max(point.y - b.bottom()).max(px(0.));
            (dy, dx)
        })?;
        let byte = part.layout.index_for_position(point).unwrap_or_else(|b| b);
        Some(self.hit(i, &part.shown, byte))
    }

    pub fn drag(&mut self, mut p: Point<Pixels>) {
        self.pointer = Some(p);
        let b = self.scroll.bounds();
        if b.size.height > px(1.) {
            p.y = p.y.clamp(b.top(), b.bottom() - px(1.));
        }
        if let Some(hit) = self.nearest(p) {
            self.head = hit
        }
    }

    pub fn scroll_drag(&mut self) -> bool {
        let Some(p) = self.pointer.filter(|_| self.dragging) else {
            return false;
        };
        let bounds = self.scroll.bounds();
        let top = p.y - bounds.top() - px(8.);
        let bottom = p.y - bounds.bottom() + px(8.);
        let delta = if top < px(0.) {
            top.max(px(-24.))
        } else {
            bottom.clamp(px(0.), px(24.))
        };
        let old = self.scroll.offset();
        let mut next = old;
        next.y = (old.y - delta).clamp(-self.scroll.max_offset().height, px(0.));
        self.drag(p);
        if old == next {
            return false;
        }
        self.scroll.set_offset(next);
        true
    }

    #[cfg(target_os = "macos")]
    pub fn context_word(&mut self, p: Position) {
        if self.can_copy() && self.anchor.min(self.head) <= p && p <= self.anchor.max(self.head) {
            return;
        }
        self.select(p, false, 2);
        self.dragging = false;
    }
    pub fn hit(&self, part: usize, shown: &str, byte: usize) -> Position {
        let full = &self.parts[part].full;
        let clipped = shown.strip_suffix('…').filter(|_| full.as_ref() != shown);
        let offset = if clipped.is_some_and(|prefix| byte > prefix.len()) {
            full.len()
        } else {
            byte.min(full.len())
        };
        Position {
            part,
            byte: full.floor_char_boundary(offset),
        }
    }

    fn range(&self, part: usize, len: usize) -> Range<usize> {
        let lo = self.anchor.min(self.head);
        let hi = self.anchor.max(self.head);
        if part < lo.part || part > hi.part {
            return 0..0;
        }
        let start = if part == lo.part { lo.byte.min(len) } else { 0 };
        let end = if part == hi.part {
            hi.byte.min(len)
        } else {
            len
        };
        start..end
    }

    pub fn select(&mut self, p: Position, extend: bool, clicks: usize) {
        self.dragging = true;
        self.head = p;
        if extend {
            return;
        }
        self.anchor = p;
        self.pointer = None;
        let value = &self.parts[p.part].full;
        if clicks >= 3 {
            self.anchor.byte = value[..p.byte].rfind('\n').map_or(0, |i| i + 1);
            self.head.byte = value[p.byte..]
                .find('\n')
                .map_or(value.len(), |i| p.byte + i + 1);
        } else if clicks == 2 {
            if let Some((i, word)) = value
                .split_word_bound_indices()
                .find(|(i, w)| i + w.len() > p.byte)
            {
                self.anchor.byte = i;
                self.head.byte = i + word.len();
            }
        }
    }

    #[cfg(test)]
    fn extend(&mut self, p: Position) {
        self.head = p;
    }

    pub fn select_all(&mut self) {
        self.anchor = Position::default();
        self.head = self.parts.last().map_or(self.anchor, |s| Position {
            part: self.parts.len() - 1,
            byte: s.full.len(),
        });
    }

    pub fn can_copy(&self) -> bool {
        self.anchor != self.head && self.anchor.part.max(self.head.part) < self.parts.len()
    }

    pub fn selected_text(&self) -> String {
        if !self.can_copy() {
            return String::new();
        }
        let rows = self.anchor.part.min(self.head.part)..=self.anchor.part.max(self.head.part);
        rows.map(|p| &self.parts[p].full[self.range(p, self.parts[p].full.len())])
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reversed_selection_preserves_full_clamped_commands_and_unicode() {
        let mut log = HistoryText::default();
        let command = "第一行\necho two\necho three\necho four";
        for (full, shown) in [
            ("EXIT 0", "EXIT 0"),
            (command, "第一行…"),
            ("13:41", "13:41"),
        ] {
            let _ = log.span(full.to_owned().into(), shown.to_owned().into(), 0, 11.);
        }
        log.select(Position { part: 2, byte: 2 }, false, 1);
        log.extend(Position { part: 0, byte: 2 });
        assert_eq!(log.selected_text(), format!("IT 0\n{command}\n13"));
        log.select(log.hit(1, "第一行…", "第一行…".len()), false, 1);
        log.extend(log.hit(1, "第一行…", 3));
        assert_eq!(log.selected_text(), &command[3..]);
    }
}
