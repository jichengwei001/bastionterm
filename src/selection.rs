use vt100::Screen;

pub(crate) type Cell = (u16, u16);
pub(crate) type BufferCell = (usize, u16);

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Unit {
    Character,
    Word,
    Line,
}

impl Unit {
    pub(crate) fn range(self, screen: &Screen, (row, col): BufferCell) -> (BufferCell, BufferCell) {
        if self == Self::Character {
            return ((row, col), (row, col));
        }
        let (height, cols) = screen.size();
        let rows = screen.history_rows() + usize::from(height);
        if self == Self::Word && col >= cols {
            return ((row, col), (row, col));
        }
        if self == Self::Line {
            let mut start = row;
            let mut end = row;
            while start > 0 && screen.buffer_row_wrapped(start - 1) {
                start -= 1;
            }
            while end + 1 < rows && screen.buffer_row_wrapped(end) {
                end += 1;
            }
            return ((start, 0), (end, cols));
        }

        let is_space = |(r, c)| {
            screen.buffer_cell(r, c).is_some_and(|cell| {
                !cell.is_wide_continuation() && (!cell.has_contents() || cell.contents() == " ")
            })
        };
        let space = is_space((row, col));
        let extends = |pos: BufferCell| {
            if space {
                return is_space(pos);
            }
            screen.buffer_cell(pos.0, pos.1).is_some_and(|cell| {
                cell.is_wide_continuation()
                    || (cell.has_contents() && !" ()[]{}',\"`".contains(&cell.contents()))
            })
        };
        let mut start = (row, col);
        let mut end = start;
        loop {
            let previous = if start.1 > 0 {
                (start.0, start.1 - 1)
            } else if !space && start.0 > 0 && screen.buffer_row_wrapped(start.0 - 1) {
                (start.0 - 1, cols - 1)
            } else {
                break;
            };
            let crosses_wrap = previous.0 != start.0;
            let continues = if crosses_wrap {
                !is_space(previous) && !is_space(start)
            } else {
                extends(previous)
            };
            if !continues {
                break;
            }
            start = previous;
        }
        loop {
            let next = if end.1 + 1 < cols {
                (end.0, end.1 + 1)
            } else if !space && end.0 + 1 < rows && screen.buffer_row_wrapped(end.0) {
                (end.0 + 1, 0)
            } else {
                break;
            };
            let crosses_wrap = next.0 != end.0;
            let continues = if crosses_wrap {
                !is_space(next) && !is_space(end)
            } else {
                extends(next)
            };
            if !continues {
                break;
            }
            end = next;
        }
        (start, (end.0, end.1 + 1))
    }
}

pub(crate) fn columns(
    a: BufferCell,
    b: BufferCell,
    row: usize,
    cols: u16,
    rectangular: bool,
) -> Option<(u16, u16)> {
    if row < a.0 || row > b.0 {
        return None;
    }
    let (start, end) = if rectangular {
        (a.1.min(b.1), a.1.max(b.1))
    } else {
        (
            if row == a.0 { a.1 } else { 0 },
            if row == b.0 { b.1 } else { cols },
        )
    };
    (start < end).then_some((start, end))
}

pub(crate) fn text(screen: &Screen, a: BufferCell, b: BufferCell, rectangular: bool) -> String {
    let result = if rectangular {
        let left = a.1.min(b.1);
        let right = a.1.max(b.1);
        (a.0..=b.0)
            .map(|r| screen.buffer_text((r, left), (r, right)))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        screen.buffer_text(a, b)
    };
    let result = result.replace('\u{a0}', " ");
    if cfg!(windows) {
        result.replace('\n', "\r\n")
    } else {
        result
    }
}

pub(crate) fn trim(cell: BufferCell, removed: usize) -> BufferCell {
    cell.0
        .checked_sub(removed)
        .map_or((0, 0), |row| (row, cell.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_uses_retained_buffer_coordinates_when_viewport_scrolls() {
        let mut p = vt100::Parser::new(3, 10, 4);
        p.process(b"A row-1\r\nB row-2\r\nC row-3\r\nD row-4\r\nE row-5");
        let selection = ((1, 2), (3, 7));
        let newline = if cfg!(windows) { "\r\n" } else { "\n" };
        for offset in [0, 2, 1] {
            p.set_scrollback(offset);
            let screen = p.screen();
            assert_eq!(
                super::text(screen, selection.0, selection.1, true),
                ["row-2", "row-3", "row-4"].join(newline)
            );
            assert_eq!(
                super::text(screen, selection.0, selection.1, false),
                ["row-2", "C row-3", "D row-4"].join(newline)
            );
            assert_eq!(super::columns((1, 7), (3, 2), 2, 10, true), Some((2, 7)));
            assert_eq!(super::columns((1, 7), (3, 2), 2, 10, false), Some((0, 10)));
        }
        p.process(b"\r\nF row-6\r\nG row-7\r\nH row-8\r\nI row-9");
        let (epoch, trimmed) = p.screen().history_origin();
        assert_eq!(trimmed, 2);
        assert_eq!(super::trim(selection.0, trimmed), (0, 0));
        assert_eq!(super::trim(selection.1, trimmed), (1, 7));
        p.process(b"\x1b[?1049h\x1b[?1049l");
        assert_ne!(p.screen().history_origin().0, epoch);
    }

    #[test]
    fn copying_sparse_cells_respects_the_whole_rows_trimmed_length() {
        let mut p = vt100::Parser::new(4, 20, 4);
        p.process("A\x1b[6G界\x1b[12GZ\r\nB\x1b[8GC".as_bytes());
        let s = p.screen();
        assert_eq!(text(s, (0, 0), (0, 4), false), "A   ");
        assert_eq!(text(s, (0, 6), (0, 7), false), " ");
        assert_eq!(text(s, (0, 12), (0, 20), false), "");
        let newline = if cfg!(windows) { "\r\n" } else { "\n" };
        assert_eq!(
            text(s, (0, 1), (1, 5), true),
            ["    ", "    "].join(newline)
        );
    }

    #[test]
    fn native_cells_match_the_bundled_terminals_unicode_provider() {
        let mut p = vt100::Parser::new(4, 20, 4);
        let combining = format!("a{}", "\u{301}".repeat(12));
        p.process(combining.as_bytes());
        p.process("\r\n😀🪄中⭐👩‍💻".as_bytes());
        let s = p.screen();
        assert_eq!(s.cell(0, 0).unwrap().contents(), combining);
        assert_eq!(s.cursor_position(), (1, 7));
        assert!(s.cell(1, 2).unwrap().is_wide());
        assert!(s.cell(1, 3).unwrap().is_wide_continuation());
        assert_eq!(s.cell(1, 5).unwrap().contents(), "👩‍");
        assert_eq!(s.cell(1, 6).unwrap().contents(), "💻");
        // Cursor/SGR controls break a run of combining characters.
        p.process("A\x1b[0m\u{301}Z".as_bytes());
        assert_eq!(p.screen().cell(1, 7).unwrap().contents(), "A");
        assert_eq!(p.screen().cell(1, 8).unwrap().contents(), "\u{301}");
        let mut p = vt100::Parser::new(3, 12, 10);
        p.process("\u{301}".repeat(13).as_bytes());
        assert_eq!(p.screen().cursor_position(), (0, 13));
        // ICH and DCH clamp a pending-wrap cursor before editing the line.
        p.process(b"\x1b[@\x1b[P");
        assert_eq!(p.screen().cursor_position(), (0, 11));
        p.process("Q".as_bytes());
        assert_eq!(p.screen().cursor_position(), (0, 12));
    }

    #[test]
    fn word_and_line_spans_handle_wide_continuations_spaces_and_wrapping() {
        let mut parser = vt100::Parser::new(5, 10, 20);
        parser.process("a  bc界defghi z\r\nnext".as_bytes());
        let screen = parser.screen();
        let text = |unit: Unit, pos| {
            let ((r, c), (rr, cc)) = unit.range(screen, pos);
            screen.buffer_text((r, c), (rr, cc))
        };
        assert_eq!(text(Unit::Word, (0, 6)), "bc界defghi");
        assert_eq!(text(Unit::Word, (1, 1)), "bc界defghi");
        assert_eq!(text(Unit::Word, (0, 2)), "  ");
        assert_eq!(text(Unit::Line, (1, 3)), "a  bc界defghi z");
        assert_eq!(text(Unit::Word, (1, 10)), "");

        let mut parser = vt100::Parser::new(3, 6, 0);
        parser.process(" abcde(foo Z".as_bytes());
        let s = parser.screen();
        for pos in [(0, 2), (1, 0)] {
            let (a, b) = Unit::Word.range(s, pos);
            assert_eq!(s.buffer_text(a, b), "abcde(foo");
        }
    }
}
