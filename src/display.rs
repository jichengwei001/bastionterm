use crate::theme::Theme;
use gpui::{div, prelude::*, px, rgb, Div, FontWeight, HighlightStyle, StyledText};
use vt100::{Color, Underline};
fn color(c: Color, default: u32, palette: &vt100::Palette) -> u32 {
    match c {
        Color::Default => default,
        Color::Idx(i) => palette.indexed(i),
        Color::Rgb(r, g, b) => u32::from_be_bytes([0, r, g, b]),
    }
}
#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
    fg: u32,
    bg: gpui::Rgba,
    bold: bool,
    italic: bool,
    underline: Underline,
    overline: bool,
    decoration_color: Option<u32>,
    strike: bool,
    dim: bool,
}

impl CellStyle {
    fn from_cell(
        cell: &vt100::Cell,
        theme: Theme,
        palette: &vt100::Palette,
        selected: bool,
    ) -> Self {
        let mut foreground = cell.fgcolor();
        let mut background = cell.bgcolor();
        let mut default_fg = palette.foreground();
        let mut default_bg = palette.background();
        if cell.inverse() {
            std::mem::swap(&mut foreground, &mut background);
            std::mem::swap(&mut default_fg, &mut default_bg);
        }
        if let Color::Idx(n @ 0..=7) = foreground {
            if cell.bold() {
                foreground = Color::Idx(n + 8);
            }
        }
        let bg = if selected {
            rgb(palette.background()).blend(gpui::rgba(theme.ansi("selectionBackground")))
        } else {
            rgb(color(background, default_bg, palette))
        };
        let underline = cell.underline_style();
        let strike = cell.strike() && !(cell.overline() && underline != Underline::None);
        let decoration_color = match cell.underline_color() {
            Color::Default => None,
            Color::Idx(i @ 0..=7) if cell.bold() => {
                Some(color(Color::Idx(i + 8), default_fg, palette))
            }
            other => Some(color(other, default_fg, palette)),
        };
        Self {
            fg: color(foreground, default_fg, palette),
            bg,
            bold: cell.bold(),
            italic: cell.italic(),
            underline: if strike { Underline::None } else { underline },
            overline: cell.overline() && !strike,
            decoration_color,
            strike,
            dim: cell.dim(),
        }
    }
}

struct Run {
    start: u16,
    end: u16,
    text: String,
    style: CellStyle,
    fallback_glyph: bool,
}

impl Run {
    fn element(self, width: f32) -> Div {
        let style = self.style;
        let mut ink = rgb(style.fg);
        if style.dim {
            ink.a = 0.5;
        }
        let len = self.text.len();
        let highlight = HighlightStyle {
            color: Some(ink.into()),
            font_style: style.italic.then_some(gpui::FontStyle::Italic),
            strikethrough: style.strike.then_some(gpui::StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
            font_weight: style.bold.then_some(FontWeight::BOLD),
            ..Default::default()
        };
        let mut element = div()
            .absolute()
            .top_0()
            .h_full()
            .left(px(f32::from(self.start) * width))
            .w(px(f32::from(self.end - self.start) * width))
            .bg(style.bg)
            .whitespace_nowrap();
        if self.fallback_glyph {
            element = element.flex().justify_center();
        }
        element = element.child(StyledText::new(self.text).with_highlights([(0..len, highlight)]));
        if style.underline != Underline::None || style.overline {
            let mut line_ink = rgb(style.decoration_color.unwrap_or(style.fg));
            line_ink.a = ink.a;
            element = element.child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, w, cx| {
                        let font = cx
                            .text_system()
                            .resolve_font(&gpui::font(crate::view::mono()));
                        let text = cx.text_system();
                        let height = px(13.);
                        let baseline =
                            bounds.top() + text.baseline_offset(font, height, bounds.size.height);
                        let top = baseline - text.ascent(font, height);
                        let bottom = (baseline + text.descent(font, height) * 0.618)
                            .min(bounds.bottom() - px(1.));
                        let line = |w: &mut gpui::Window, y, pattern, second| {
                            paint_decoration(
                                w,
                                gpui::point(bounds.left(), y),
                                bounds.size.width,
                                pattern,
                                second,
                                line_ink,
                            );
                        };
                        if style.overline {
                            let pattern = if style.underline == Underline::None {
                                Underline::Single
                            } else {
                                style.underline
                            };
                            line(w, top.max(bounds.top()), pattern, px(2.));
                        }
                        if style.underline != Underline::None {
                            let y = if style.underline == Underline::Wavy {
                                bottom - px(2.)
                            } else {
                                bottom
                            };
                            line(w, y, style.underline, px(-2.));
                        }
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            );
        }
        element
    }
}

fn paint_decoration(
    window: &mut gpui::Window,
    origin: gpui::Point<gpui::Pixels>,
    width: gpui::Pixels,
    pattern: Underline,
    second: gpui::Pixels,
    color: gpui::Rgba,
) {
    let style = gpui::UnderlineStyle {
        thickness: px(1.),
        color: Some(color.into()),
        wavy: pattern == Underline::Wavy,
    };
    if matches!(pattern, Underline::Dotted | Underline::Dashed) {
        let dash = if pattern == Underline::Dotted {
            px(1.)
        } else {
            px(3.)
        };
        let mut offset = px(0.);
        while offset < width {
            window.paint_underline(
                gpui::point(origin.x + offset, origin.y),
                dash.min(width - offset),
                &style,
            );
            offset += dash * 2.;
        }
    } else {
        window.paint_underline(origin, width, &style);
        if pattern == Underline::Double {
            window.paint_underline(gpui::point(origin.x, origin.y + second), width, &style);
        }
    }
}

#[derive(Clone)]
pub struct LinkSpan {
    pub row: usize,
    pub columns: std::ops::Range<u16>,
    pub target: std::sync::Arc<vt100::Hyperlink>,
}

impl LinkSpan {
    pub fn at(screen: &vt100::Screen, row: usize, col: u16) -> Option<Self> {
        let target = screen.buffer_cell(row, col)?.hyperlink()?;
        target.url()?;
        let same = |col| {
            screen
                .buffer_cell(row, col)
                .and_then(vt100::Cell::hyperlink)
                .is_some_and(|link| target.same_span(link))
        };
        let mut left = col;
        let mut right = col + 1;
        while left > 0 && same(left - 1) {
            left -= 1;
        }
        while right < screen.size().1 && same(right) {
            right += 1;
        }
        Some(Self {
            row,
            columns: left..right,
            target: target.clone(),
        })
    }

    pub fn same(&self, other: &Self) -> bool {
        self.row == other.row
            && self.columns == other.columns
            && self.target.same_span(&other.target)
    }
}

pub fn line(
    cells: &[vt100::Cell],
    palette: &vt100::Palette,
    row: u16,
    theme: Theme,
    selection: Option<(u16, u16)>,
    cell_width: f32,
    block_cursor: Option<(u16, u16, bool)>,
    hover_link: Option<&std::ops::Range<u16>>,
) -> Div {
    let mut line = div().relative().size_full().overflow_hidden();
    let mut run: Option<Run> = None;
    let cols = cells.len() as u16;
    for (i, cell) in cells.iter().enumerate() {
        let col = i as u16;
        if cell.is_wide_continuation() {
            continue;
        }
        let selected = selection.is_some_and(|(a, b)| a <= col && col < b);
        let cursor = block_cursor.filter(|&(r, c, _)| r == row && c == col);
        if cursor.is_none()
            && !cell.has_contents()
            && !selected
            && cell.bgcolor() == Color::Default
            && !cell.inverse()
            && cell.underline_style() == Underline::None
            && !cell.overline()
            && !cell.strike()
        {
            continue;
        }
        let mut style = CellStyle::from_cell(cell, theme, palette, selected);
        if hover_link.is_some_and(|columns| columns.contains(&col)) {
            style.underline = Underline::Single;
            style.decoration_color = None;
            style.overline = false;
            style.strike = false;
        }
        if let Some((_, _, lit)) = cursor {
            if lit {
                style.bg = rgb(palette.cursor());
                style.fg = theme.ansi("cursorAccent");
            } else {
                style.fg = palette.cursor();
            }
            style.dim = false;
        }
        let text = if cell.invisible() || !cell.has_contents() {
            if cell.is_wide() { "  " } else { " " }.to_owned()
        } else {
            cell.contents()
        };
        let fallback_glyph = !text.is_ascii();
        let end = (col + 1 + u16::from(cell.is_wide())).min(cols);
        if let Some(previous) = &mut run {
            if !previous.fallback_glyph
                && !fallback_glyph
                && previous.style == style
                && previous.end == col
            {
                previous.text.push_str(&text);
                previous.end = end;
                continue;
            }
        }
        if let Some(previous) = run.take() {
            line = line.child(previous.element(cell_width));
        }
        run = Some(Run {
            start: col,
            end,
            text,
            style,
            fallback_glyph,
        });
    }
    if let Some(previous) = run {
        line = line.child(previous.element(cell_width));
    }
    line
}

#[cfg(test)]
mod tests {
    #[test]
    fn colored_line_styles_round_trip_and_reset_independently() {
        use vt100::{Color, Parser, Underline};
        let mut parser = Parser::new(3, 30, 4);
        parser.process(b"\x1b[4:3;58:2::10:20:30;53mA\x1b[21mB\x1b[24mC\x1b[4:4;55;58;5;6mD\x1b[4:5;59mE\x1b[0mF");
        let mut replay = Parser::new(3, 30, 4);
        replay.process(&parser.screen().contents_formatted());
        for screen in [parser.screen(), replay.screen()] {
            let attrs = |col| {
                let c = screen.cell(0, col).unwrap();
                (c.underline_style(), c.underline_color(), c.overline())
            };
            let rgb = Color::Rgb(10, 20, 30);
            assert_eq!(attrs(0), (Underline::Wavy, rgb, true));
            assert_eq!(attrs(1), (Underline::Double, rgb, true));
            assert_eq!(attrs(2), (Underline::None, rgb, true));
            assert_eq!(attrs(3), (Underline::Dotted, Color::Idx(6), false));
            assert_eq!(attrs(4), (Underline::Dashed, Color::Default, false));
            assert_eq!(attrs(5), (Underline::None, Color::Default, false));
        }
    }

    #[test]
    fn hyperlinks_survive_reflow_and_terminate_on_close_erase_and_reset() {
        use super::LinkSpan;
        let mut p = vt100::Parser::new(3, 12, 20);
        let url = format!(
            "https://example.test/{}?path={}",
            "a;b;".repeat(10),
            "x".repeat(1400)
        );
        let packet = format!("\x1b]8;id=doc;{url}\x1b\\A界\x1b[0mB\x1b]8;;\x07.");
        for bytes in packet.as_bytes().chunks(3) {
            p.process(bytes);
        }
        let span = LinkSpan::at(p.screen(), 0, 2).unwrap();
        assert_eq!(span.columns, 0..4);
        assert_eq!(span.target.url(), Some(url.as_str()));
        assert!(LinkSpan::at(p.screen(), 0, 4).is_none());
        p.process(b"\r\nnext\r\nnext\r\nlast");
        p.set_size(3, 16);
        assert_eq!(
            LinkSpan::at(p.screen(), 0, 1).unwrap().target.url(),
            Some(url.as_str())
        );
        p.process(b"\x1b[?1049h\x1b[H");
        assert!(LinkSpan::at(p.screen(), 0, 1).is_none());
        p.process(b"\x1b[?1049l");
        assert_eq!(
            LinkSpan::at(p.screen(), 0, 1).unwrap().target.url(),
            Some(url.as_str())
        );
        p.process(b"\x1bc\x1b]8;;https://example.test/\x07A\x1b]8;;file:///tmp/no\x07B");
        assert!(LinkSpan::at(p.screen(), 0, 0).is_some());
        assert!(LinkSpan::at(p.screen(), 0, 1).is_none());
        assert_eq!(
            p.screen().cell(0, 1).unwrap().underline_style(),
            vt100::Underline::Dashed
        );
        p.process(b"\r\x1b[2K");
        assert!(LinkSpan::at(p.screen(), 0, 0).is_none());
    }

    #[test]
    fn adjacent_hyperlinks_keep_their_identity_and_cancelled_osc_never_opens() {
        use super::LinkSpan;
        let mut p = vt100::Parser::new(2, 14, 4);
        for params in ["", "", "id=q", "id=q", "id=q"] {
            p.process(format!("\x1b]8;{params};http://example.test/\x07x").as_bytes());
        }
        assert_eq!(LinkSpan::at(p.screen(), 0, 0).unwrap().columns, 0..1);
        assert_eq!(LinkSpan::at(p.screen(), 0, 1).unwrap().columns, 1..2);
        assert_eq!(LinkSpan::at(p.screen(), 0, 2).unwrap().columns, 2..5);
        p.process(b"\x1b]8;;\x07\x1b]8;;https://example.test/unfinished\x18y");
        assert!(LinkSpan::at(p.screen(), 0, 5).is_none());
    }

    #[test]
    fn cursor_modes_reset_and_round_trip_independently_of_cursor_position() {
        let mut terminal = vt100::Parser::new(3, 8, 20);
        use vt100::CursorShape::{Bar, Block, Underline};
        for (seq, shape, blink) in [
            ("\x1b[?12l", Bar, false),
            ("\x1b[1 q", Block, true),
            ("\x1b[4 q", Underline, false),
            ("\x1b[?12h", Underline, false),
            ("\x1b[0 q", Bar, true),
            ("\x1b[6 q", Bar, false),
            ("\x1b[!p", Bar, true),
        ] {
            terminal.process(seq.as_bytes());
            let screen = terminal.screen();
            assert_eq!(
                (screen.cursor_shape(), screen.cursor_blinks()),
                (shape, blink)
            );
            let mut copy = vt100::Parser::new(3, 8, 20);
            copy.process(&screen.cursor_state_formatted());
            assert_eq!(
                (copy.screen().cursor_shape(), copy.screen().cursor_blinks()),
                (shape, blink)
            );
        }
        terminal.process(b"12345678");
        assert_eq!(terminal.screen().cursor_position().1, 8);
        assert_eq!(terminal.screen().visible_cursor(), Some((0, 7)));
        terminal.process("\r\n界\x1b[2;2H".as_bytes());
        assert_eq!(terminal.screen().visible_cursor(), None);
        terminal.process(b"\x1b[?25l\x1b[1;1H");
        assert_eq!(terminal.screen().visible_cursor(), None);
        terminal.process(b"\x1b[!p");
        assert_eq!(terminal.screen().visible_cursor(), Some((0, 0)));
    }

    #[test]
    fn horizontal_edits_respect_margins_and_repeat_combined_characters() {
        let mut p = vt100::Parser::new(4, 6, 20);
        p.process(b"top\r\n123456\r\nabcdef\r\nbottom\x1b[2;3r\x1b[2;3H\x1b[2'}");
        assert_eq!(p.screen().contents(), "top\n12  34\nab  cd\nbottom");
        p.process(b"\x1b[2'~\x1b[1 @\x1b[2 A");
        assert_eq!(p.screen().contents(), "top\n  234\n  bcd\nbottom");
        p.process(b"\x1b[4;1H\x1b[3 @");
        assert_eq!(p.screen().contents(), "top\n  234\n  bcd\nbottom");
        p.process("\x1bc界\u{301}\x1b[2b".as_bytes());
        assert_eq!(p.screen().contents(), "界\u{301}界\u{301}界\u{301}");
        p.process(b"\x1b[2b");
        assert_eq!(p.screen().cursor_position(), (0, 6));
    }

    #[test]
    fn colored_edits_and_scrolling_respect_region_and_history() {
        let mut p = vt100::Parser::new(4, 6, 12);
        p.process(b"one\r\ntwo\r\nthree\r\nfour\x1b[31;44m\x1b[2;3r\x1b[2;2H\x1b[L");
        assert_eq!(p.screen().contents(), "one\n\ntwo\nfour");
        assert_eq!(p.screen().cursor_position(), (1, 0));
        let blank = p.screen().cell(1, 4).unwrap();
        assert_eq!(blank.bgcolor(), vt100::Color::Idx(4));
        assert_eq!(blank.fgcolor(), vt100::Color::Default);
        p.process(b"\x1b[M\x1b[4;2H\x1b[L");
        assert_eq!(p.screen().contents(), "one\ntwo\n\nfour");
        assert_eq!(p.screen().cursor_position(), (3, 1));
        p.process(b"\x1b[r\x1b[S");
        assert_eq!(p.screen().history_rows(), 0);
        p.process(b"\x1b[4;1H\n");
        assert_eq!(p.screen().history_rows(), 1);
        p.process("abcde界".as_bytes());
        let padding = p.screen().cell(2, 5).unwrap();
        assert!(!padding.has_contents());
        assert_eq!(padding.bgcolor(), vt100::Color::Idx(4));
        p.process(b"\x1b[1G\x1b[X");
        assert_eq!(
            p.screen().cell(3, 1).unwrap().fgcolor(),
            vt100::Color::Default
        );
    }

    #[test]
    fn alignment_test_uses_current_attributes_and_origin_without_scrolling_history() {
        let mut p = vt100::Parser::new(4, 6, 12);
        p.process(b"top\r\nmid\r\nlow\r\nend\x1b[2;3r\x1b[?6h\x1b[34;41m\x1b#8");
        assert_eq!(p.screen().contents(), "top\nEEEEEE\nEEEEEE\nEEEEEE");
        assert_eq!(p.screen().cursor_position(), (1, 0));
        assert_eq!(
            p.screen().cell(3, 5).unwrap().bgcolor(),
            vt100::Color::Idx(1)
        );
        assert_eq!(p.screen().history_rows(), 0);
    }

    #[test]
    fn osc_colors_follow_stream_order_and_frozen_frame() {
        use crate::theme::Theme;
        let mut p = Theme(0).viewport(2, 10);
        let query = b"\x1b]4;17;?\x07\x1b]11;?\x1b\\";
        let original = p.process_with_replies(query);
        p.process(b"\x1b[?2026h\x1b]4;17;rgb:f/a/1;1");
        assert_eq!(
            p.process_with_replies(b"7;?\x07"),
            b"\x1b]4;17;rgb:ffff/aaaa/1111\x1b\\"
        );
        assert_eq!(p.screen().frame_palette().indexed(17), 0x00005f);
        p.process(b"\x1b[?2026l\x1b]11;#a3c\x07");
        assert_eq!(p.screen().frame_palette().indexed(17), 0xffaa11);
        assert_eq!(p.screen().frame_palette().background(), 0xa030c0);
        p.process(query);
        assert!(p.process_with_replies(b"").is_empty());
        p.process(b"\x1bc");
        assert_eq!(
            p.process_with_replies(query),
            b"\x1b]4;17;rgb:ffff/aaaa/1111\x1b\\\x1b]11;rgb:a0a0/3030/c0c0\x1b\\"
        );
        p.process(b"\x1b]104;17\x07\x1b]111\x07");
        assert_eq!(p.process_with_replies(query), original);
        p.process(b"\x1b]11;#abcdef\x07");
        p.set_palette(Theme(3).colors(), false);
        assert_eq!(p.screen().frame_palette().background(), 0xabcdef);
        p.set_palette(Theme(3).colors(), true);
        assert_eq!(
            p.screen().frame_palette().background(),
            Theme(3).ansi("background")
        );
    }

    #[test]
    fn mode_reports_follow_current_buffer_and_persisted_options() {
        let mut parser = vt100::Parser::new(3, 5, 0);
        let ask = b"\x1b[?6$p\x1b[4$p\x1b[20$p\x1b[?66$p\x1b[?2004$p\x1b[?9999$p";
        parser.process(b"\x1b[?6;66;2004h\x1b[4;20h\x1b[?1049h");
        assert_eq!(
            parser.process_with_replies(ask),
            b"\x1b[?6;1$y\x1b[4;1$y\x1b[20;1$y\x1b[?66;1$y\x1b[?2004;1$y\x1b[?9999;0$y"
        );
        parser.process(b"\x1b[?6l\x1b[?1049l");
        assert_eq!(parser.process_with_replies(b"\x1b[?6$p"), b"\x1b[?6;2$y");
        parser.process(b"\x1bc");
        assert_eq!(
            parser.process_with_replies(ask),
            b"\x1b[?6;2$y\x1b[4;2$y\x1b[20;1$y\x1b[?66;2$y\x1b[?2004;2$y\x1b[?9999;0$y"
        );
    }

    #[test]
    fn synchronized_frame_holds_pixels_but_services_live_queries() {
        let mut p = vt100::Parser::new_viewport(3, 5, 20);
        p.process(b"old\r\x1b[?2026");
        let old = p.screen().frame_row(0).to_vec();
        assert_eq!(
            p.process_with_replies(b"hnew\x1b[?2026$p\x1b[6n"),
            b"\x1b[?2026;1$y\x1b[1;4R"
        );
        assert_eq!(p.screen().contents(), "new");
        assert_eq!(p.screen().frame_row(0), old);
        p.process(b"\x1b[?2026h");
        assert_eq!(p.screen().frame_row(0), old);
        assert_eq!(
            p.process_with_replies(b"\x1b[?2026l\x1b[?2026$p"),
            b"\x1b[?2026;2$y"
        );
        assert_ne!(p.screen().frame_row(0), old);
        p.process(b"\x1bcabcdeX\r\x1b[?45h\x08");
        assert_eq!(p.screen().cursor_position(), (0, 4));
        assert!(!p.screen().buffer_row_wrapped(0));
        p.process(b"\x1b[2;1H\x08");
        assert_eq!(p.screen().cursor_position(), (1, 0));
    }

    #[test]
    fn focus_and_mouse_modes_replay_without_answering_historical_queries() {
        let mut p = vt100::Parser::new(8, 40, 32);
        let set = b"\x1b[?1003;1016;1004h";
        let ask = b"\x1b[?1003$p\x1b[?1004$p\x1b[?1006$p\x1b[?1016$p\x1b[?1005$p";
        let active = b"\x1b[?1003;1$y\x1b[?1004;1$y\x1b[?1006;2$y\x1b[?1016;1$y\x1b[?1005;4$y";
        p.process(set);
        assert_eq!(p.process_with_replies(ask), active);
        let initial = p.screen().focus_request();
        p.process(b"\x1b[?1004h");
        assert_ne!(p.screen().focus_request(), initial);
        let mut restored = vt100::Parser::new(8, 40, 32);
        restored.process(&p.screen().input_mode_formatted());
        restored.process(ask);
        assert!(restored.process_with_replies(b"").is_empty());
        assert_eq!(restored.process_with_replies(ask), active);
        let old = p.screen().clone();
        p.process(b"\x1b[?1000;1006;1004l");
        restored.process(&p.screen().input_mode_diff(&old));
        let off = b"\x1b[?1003;2$y\x1b[?1004;2$y\x1b[?1006;2$y\x1b[?1016;2$y\x1b[?1005;4$y";
        assert_eq!(restored.process_with_replies(ask), off);
        p.process(set);
        p.process(b"\x1bc");
        assert_eq!(p.process_with_replies(ask), off);
    }

    #[test]
    fn status_strings_reply_only_to_live_complete_queries_and_respect_protected_cells() {
        let mut p = vt100::Parser::new(3, 12, 10);
        p.process("A\x1b[1\"q界B\x1b[0\"qC\x1b[?2K".as_bytes());
        assert_eq!(p.screen().contents(), " 界B");
        let mut copy = vt100::Parser::new(3, 12, 10);
        copy.process(&p.screen().contents_formatted());
        copy.process(b"\x1b[?2J");
        assert_eq!(copy.screen().contents(), " 界B");
        p.process(b"\r\n1\r\n2\r\n3\r\n4");
        p.set_scrollback(2);
        assert_eq!(p.screen().scrollback(), 2);
        p.process(b"\x1b[3J");
        assert_eq!(p.screen().scrollback(), 0);
        assert_eq!(p.screen().contents(), "2\n3\n4");
        assert!(p.process_with_replies(b"\x1bP$q ").is_empty());
        assert_eq!(p.process_with_replies(b"q\x1b\\"), b"\x1bP1$r5 q\x1b\\");
        assert!(p.process_with_replies(b"\x1bP$qm\x18").is_empty());
        assert!(p.process_with_replies(b"\x1bP$q").is_empty());
        p.process(b"m");
        assert!(p.process_with_replies(b"\x1b\\").is_empty());
        p.process(b"\x1bP$q");
        assert!(p.process_with_replies(b"r\x1b\\").is_empty());
    }

    #[test]
    fn line_drawing_and_tab_stops_survive_chunking_and_buffer_switches() {
        let mut parser = vt100::Parser::new(6, 24, 100);
        let input = "\x1b)0\x0elqqqqk\r\nx\x0f中文\x0ex\r\nmqqqqj\x0f\r\n\x1b[3g\x1b[6G\x1bH\r\tA\x1b[?47h\r\tZ\x1b[?47l\r\tB";
        for byte in input.as_bytes() {
            parser.process(std::slice::from_ref(byte));
        }
        assert_eq!(
            parser.screen().rows(0, 24).take(4).collect::<Vec<_>>(),
            ["┌────┐", "│中文│", "└────┘", "     B"]
        );
        assert_eq!(parser.screen().cursor_position(), (3, 6));
    }

    #[test]
    fn sgr_intensity_conceal_and_strike_round_trip_without_losing_text() {
        let mut parser = vt100::Parser::new(3, 30, 30);
        parser.process("\x1b[1;2mA\x1b[22;2;8;9m界\x1b[28;29mC\x1b[22mD".as_bytes());
        let mut replay = vt100::Parser::new(3, 30, 30);
        replay.process(&parser.screen().contents_formatted());
        for screen in [parser.screen(), replay.screen()] {
            assert_eq!(screen.contents(), "A界CD");
            let flags = |col| {
                let cell = screen.cell(0, col).unwrap();
                (cell.bold(), cell.dim(), cell.invisible(), cell.strike())
            };
            assert_eq!(flags(0), (true, true, false, false));
            assert_eq!(flags(1), (false, true, true, true));
            assert!(screen.cell(0, 2).unwrap().is_wide_continuation());
            assert_eq!(flags(3), (false, true, false, false));
            assert_eq!(flags(4), (false, false, false, false));
        }
    }

    // Expected rows and cursor positions are from the 2.0.7 web terminal (xterm 6).
    #[test]
    fn resize_reflows_wide_and_styled_history_without_losing_the_prompt() {
        let mut p = vt100::Parser::new(4, 8, 100);
        p.process("1234567\x1b[32m界ABCD\x1b[0m\r\nOK".as_bytes());
        for (h, w, expected, cursor) in [
            (4, 5, vec!["12345", "67界A", "BCD", "OK"], (3, 2)),
            (4, 12, vec!["1234567界ABC", "D", "OK", ""], (2, 2)),
            (2, 12, vec!["D", "OK"], (1, 2)),
            (6, 12, vec!["1234567界ABC", "D", "OK", "", "", ""], (2, 2)),
        ] {
            p.set_size(h, w);
            assert_eq!(p.screen().rows(0, w).collect::<Vec<_>>(), expected);
            assert_eq!(p.screen().cursor_position(), cursor);
        }
        assert_eq!(
            p.screen().cell(0, 7).unwrap().fgcolor(),
            vt100::Color::Idx(2)
        );
        assert!(p.screen().cell(0, 8).unwrap().is_wide_continuation());
    }

    #[test]
    fn resize_keeps_scrolled_view_indexable_and_restores_the_primary_buffer() {
        let mut p = vt100::Parser::new(3, 12, 100);
        p.process(b"one\r\ntwo\r\n123456789AB\r\nlast\r\n$");
        p.set_size(3, 5);
        assert_eq!(
            p.screen().rows(0, 5).collect::<Vec<_>>(),
            ["B", "last", "$"]
        );
        p.set_scrollback(usize::MAX);
        assert_eq!(
            p.screen().rows(0, 5).collect::<Vec<_>>(),
            ["one", "two", "12345"]
        );
        p.set_size(3, 16);
        assert_eq!(
            p.screen().rows(0, 16).collect::<Vec<_>>(),
            ["one", "two", "123456789AB"]
        );
        assert!(p.screen().cell(2, 15).is_some());
        p.set_scrollback(0);
        p.process(b"\x1b[?1049h\x1b[2J\x1b[Halternate");
        p.set_size(3, 8);
        assert_eq!(p.screen().rows(0, 8).next().unwrap(), "alternat");
        p.process(b"\x1b[?1049l");
        p.set_size(3, 12);
        assert_eq!(
            p.screen().rows(0, 12).collect::<Vec<_>>(),
            ["123456789AB", "last", "$"]
        );
    }

    #[test]
    fn clear_keeps_colored_prompt_and_input_mode() {
        let mut parser = vt100::Parser::new(20, 80, 100);
        parser.process(b"old\r\n\x1b[?2004h\x1b[32mhost$ \x1b[0m");
        parser.clear_view();
        assert_eq!(parser.screen().contents().trim_end(), "host$");
        assert_eq!(parser.screen().cursor_position(), (0, 6));
        assert!(parser.screen().bracketed_paste());
        assert_eq!(
            parser.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Idx(2)
        );
    }
}
