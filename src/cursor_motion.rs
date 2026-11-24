//! Arrow-key input for an Alt-click, following the bundled xterm.js MoveToCell behavior.
use crate::selection::Cell;
use vt100::Screen;

pub(crate) fn arrows(screen: &Screen, target: Cell) -> String {
    let (row, col) = screen.cursor_position();
    let (target_row, target_col) = target;
    let cols = screen.size().1;
    let mut output = String::new();
    let mut push = |direction, count| {
        let prefix = if screen.application_cursor() {
            "\x1bO"
        } else {
            "\x1b["
        };
        for _ in 0..count {
            output.push_str(prefix);
            output.push(direction);
        }
    };

    if !screen.alternate_screen() {
        // Readline and similar shell editors move horizontally even across visual rows.
        let index = |(r, c): Cell| usize::from(r) * usize::from(cols) + usize::from(c);
        let start = index((row, col));
        let end = index(target);
        push(if end < start { 'D' } else { 'C' }, start.abs_diff(end));
        return output;
    }

    let line_start = |mut r: u16| {
        while r > 0 && screen.row_wrapped(r - 1) {
            r -= 1;
        }
        r
    };
    let logical_row = |r: u16| (0..r).filter(|&i| !screen.row_wrapped(i)).count();
    let start_line = logical_row(row);
    let end_line = logical_row(target_row);
    let mut horizontal_row = row;
    if start_line != end_line {
        push(
            'D',
            span_length(screen, (row, col), (line_start(row), col), false),
        );
        push(
            if end_line < start_line { 'A' } else { 'B' },
            start_line.abs_diff(end_line),
        );
        horizontal_row = line_start(target_row);
    }

    // xterm uses the clicked column in its final direction lookup. This matters
    // when navigation enters or leaves a soft-wrapped line in the alternate buffer.
    let direction_row = if logical_row(target_col) != end_line {
        line_start(target_row)
    } else {
        row
    };
    let forward = target > (direction_row, col);
    push(
        if forward { 'C' } else { 'D' },
        span_length(screen, (horizontal_row, col), target, forward),
    );
    output
}

// xterm counts UTF-16 units of the physical row fragments crossed, including
// unoccupied cells. A backwards step flushes the row when it crosses column 0.
fn span_length(screen: &Screen, start: Cell, target: Cell, forward: bool) -> usize {
    let (rows, cols) = screen.size();
    let (mut row, mut col) = start;
    let mut total = 0;
    loop {
        let reached = row == target.0
            && if forward {
                col <= target.1
            } else {
                col >= target.1
            };
        let (mut left, right) = if reached {
            (col, target.1)
        } else if forward {
            (col, cols)
        } else {
            (0, col.saturating_add(1))
        };
        while left < right {
            if let Some(cell) = screen.cell(row, left) {
                total += cell.contents().encode_utf16().count().max(1);
                left += if cell.is_wide() { 2 } else { 1 };
            } else {
                total += 1;
                left += 1;
            }
        }
        if reached {
            break;
        }
        if forward {
            row += 1;
            if row >= rows {
                break;
            }
            col = 0;
        } else if row > 0 {
            row -= 1;
            col = cols - 1;
        } else {
            break;
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_and_alternate_buffer_use_their_own_arrow_sequences() {
        let mut parser = vt100::Parser::new(8, 12, 100);
        parser.process(b"\x1b[2;6H");
        assert_eq!(arrows(parser.screen(), (0, 7)), "\x1b[D".repeat(10));
        parser.process("\x1b[?1049h\x1b[?1h\x1b[Hab界é😀\x1b[H".as_bytes());
        assert_eq!(arrows(parser.screen(), (0, 6)), "\x1bOC".repeat(7));
        assert_eq!(arrows(parser.screen(), (2, 1)), "\x1bOB\x1bOB\x1bOC");
    }
}
