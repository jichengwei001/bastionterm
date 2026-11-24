// Bastion Bridge modification. Rewrap the primary grid and its scrollback together.
use crate::{cell::Cell, row::Row};
use std::collections::VecDeque;

pub(crate) struct Anchor {
    pub source_row: usize,
    pub source_col: u16,
    pub row: usize,
    pub col: u16,
}

impl Anchor {
    pub fn new(row: usize, col: u16) -> Self {
        Self {
            source_row: row,
            source_col: col,
            row,
            col,
        }
    }
}

pub(crate) fn rewrap(
    mut input: VecDeque<Row>,
    cols: u16,
    anchors: &mut [Anchor],
    cursor_row: usize,
) -> VecDeque<Row> {
    let mut output = VecDeque::with_capacity(input.len());
    let mut start = 0;
    while !input.is_empty() {
        let count = input
            .iter()
            .position(|r| !r.wrapped())
            .map_or(input.len(), |n| n + 1);
        // As in xterm, the program at the cursor redraws its own active input line.
        if (start..start + count).contains(&cursor_row)
            || (count == 1 && input[0].trimmed_len() <= usize::from(cols))
        {
            for a in anchors
                .iter_mut()
                .filter(|a| (start..start + count).contains(&a.source_row))
            {
                a.row = output.len() + a.source_row - start;
                a.col = a.source_col.min(cols - 1);
            }
            for mut row in input.drain(..count) {
                let wrapped = row.wrapped();
                row.resize(cols, Cell::default());
                row.wrap(wrapped);
                output.push_back(row);
            }
        } else {
            let mut line = Vec::with_capacity(cols.into());
            for row_index in start..start + count {
                let row = input.pop_front().unwrap();
                let length = if !row.wrapped() {
                    row.trimmed_len()
                } else {
                    // A double-width glyph may have left one unused column at the wrap.
                    let wide_next = input
                        .front()
                        .and_then(|r| r.get(0))
                        .is_some_and(Cell::is_wide);
                    let cells = row.cols();
                    let ends_in_blank = row.get(cells - 1).is_some_and(|cell| {
                        !cell.has_contents() && !cell.is_wide_continuation()
                    });
                    usize::from(cells) - usize::from(wide_next && ends_in_blank)
                };
                let mut row_anchors = anchors
                    .iter_mut()
                    .filter(|a| a.source_row == row_index)
                    .collect::<Vec<_>>();
                for (col, cell) in row.into_cells().into_iter().take(length).enumerate() {
                    if line.len() == usize::from(cols)
                        || (cell.is_wide() && line.len() + 1 == usize::from(cols))
                    {
                        output.push_back(Row::from_cells(std::mem::take(&mut line), cols, true));
                    }
                    for a in &mut row_anchors {
                        if usize::from(a.source_col) == col {
                            a.row = output.len();
                            a.col = line.len() as u16;
                        }
                    }
                    line.push(cell);
                }
                for a in row_anchors {
                    if usize::from(a.source_col) >= length {
                        a.row = output.len();
                        a.col = (line.len() + usize::from(a.source_col) - length)
                            .min(usize::from(cols) - 1) as u16;
                    }
                }
            }
            output.push_back(Row::from_cells(line, cols, false));
        }
        start += count;
    }
    output
}
