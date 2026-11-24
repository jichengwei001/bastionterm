use crate::term::BufWrite as _;

#[derive(Clone, Debug)]
pub struct Row {
    cells: Vec<crate::cell::Cell>,
    wrapped: bool,
}

impl Row {
    pub(crate) fn fill(&mut self, cell: &crate::Cell) {
        self.cells.fill(cell.clone());
        self.wrapped = false;
    }

    pub fn new(cols: u16) -> Self {
        Self::blank(cols, Default::default())
    }
    pub fn blank(cols: u16, attrs: crate::attrs::Attrs) -> Self {
        let mut blank = crate::Cell::default();
        blank.clear(attrs);
        Self {
            cells: vec![blank; usize::from(cols)],
            wrapped: false,
        }
    }

    pub(crate) fn cols(&self) -> u16 {
        self.cells
            .len()
            .try_into()
            // we limit the number of cols to a u16 (see Size)
            .unwrap()
    }

    pub fn clear(&mut self, attrs: crate::attrs::Attrs) {
        for cell in &mut self.cells {
            cell.clear(attrs);
        }
        self.wrapped = false;
    }

    pub(crate) fn as_slice(&self) -> &[crate::Cell] {
        &self.cells
    }

    fn cells(&self) -> impl Iterator<Item = &crate::cell::Cell> {
        self.cells.iter()
    }

    pub fn get(&self, col: u16) -> Option<&crate::cell::Cell> {
        self.cells.get(usize::from(col))
    }

    pub fn get_mut(&mut self, col: u16) -> Option<&mut crate::cell::Cell> {
        self.cells.get_mut(usize::from(col))
    }

    pub fn shift_columns(&mut self, start: u16, count: u16, right: bool, fill: &crate::Cell) {
        let start = usize::from(start);
        let len = self.cells.len();
        let n = usize::from(count).min(len - start);
        if start > 0 && self.cells[start - 1].is_wide() {
            self.cells[start - 1] = fill.clone();
        }
        if right {
            self.cells[start..].rotate_right(n);
            self.cells[start..start + n].fill(fill.clone());
            if self.cells[len - 1].is_wide() {
                self.cells[len - 1] = fill.clone();
            }
        } else {
            self.cells[start..].rotate_left(n);
            self.cells[len - n..].fill(fill.clone());
            if self.cells[start].is_wide_continuation() {
                self.cells[start] = fill.clone();
            }
        }
    }

    pub fn erase(&mut self, i: u16, attrs: crate::attrs::Attrs) {
        let wide = self.cells[usize::from(i)].is_wide();
        self.clear_wide(i, attrs);
        self.cells[usize::from(i)].clear(attrs);
        if i == self.cols() - if wide { 2 } else { 1 } {
            self.wrapped = false;
        }
    }

    pub fn resize(&mut self, len: u16, cell: crate::cell::Cell) {
        if len < self.cols() && len > 0 && self.cells[usize::from(len) - 1].is_wide() {
            self.cells[usize::from(len) - 1] = crate::cell::Cell::default();
        }
        self.cells.resize(usize::from(len), cell);
        self.wrapped = false;
    }

    pub(crate) fn trimmed_len(&self) -> usize {
        self.cells
            .iter()
            .rposition(|c| *c != crate::cell::Cell::default())
            .map_or(0, |i| i + 1)
    }

    pub(crate) fn into_cells(self) -> Vec<crate::cell::Cell> {
        self.cells
    }

    pub(crate) fn from_cells(mut cells: Vec<crate::cell::Cell>, cols: u16, wrapped: bool) -> Self {
        cells.resize(usize::from(cols), crate::cell::Cell::default());
        Self { cells, wrapped }
    }

    pub fn wrap(&mut self, wrap: bool) {
        self.wrapped = wrap;
    }

    pub fn wrapped(&self) -> bool {
        self.wrapped
    }

    fn clear_wide(&mut self, i: u16, attrs: crate::attrs::Attrs) {
        let col = usize::from(i);
        let other = if self.cells[col].is_wide() {
            col.checked_add(1)
        } else if self.cells[col].is_wide_continuation() {
            col.checked_sub(1)
        } else {
            None
        };
        if let Some(cell) = other.and_then(|j| self.cells.get_mut(j)) {
            cell.clear(attrs)
        }
    }

    /// Translate a viewport or selection range. Blank cells inside the printed
    /// portion of the whole row count even when the selection ends inside a gap.
    pub fn write_range(&self, out: &mut String, start: u16, end: u16) {
        let Some((last, cell)) = self
            .cells
            .iter()
            .enumerate()
            .rfind(|(_, c)| c.has_contents())
        else {
            return;
        };
        let end = usize::from(end)
            .min(last + usize::from(cell.width()))
            .min(self.cells.len());
        let mut index = usize::from(start);
        while index < end {
            let cell = &self.cells[index];
            if cell.has_contents() {
                cell.append_contents_to(out);
            } else {
                out.push(' ');
            }
            index += usize::from(cell.width().max(1));
        }
    }

    pub fn write_contents(&self, contents: &mut String, start: u16, width: u16, wrapping: bool) {
        let mut prev_was_wide = false;

        let mut prev_col = start;
        for (col, cell) in self
            .cells()
            .enumerate()
            .skip(usize::from(start))
            .take(usize::from(width))
        {
            if prev_was_wide {
                prev_was_wide = false;
                continue;
            }
            prev_was_wide = cell.is_wide();

            // we limit the number of cols to a u16 (see Size)
            let col: u16 = col.try_into().unwrap();
            if cell.has_contents() {
                for _ in 0..(col - prev_col) {
                    contents.push(' ');
                }
                prev_col += col - prev_col;

                contents.push_str(&cell.contents());
                prev_col += if cell.is_wide() { 2 } else { 1 };
            }
        }
        if prev_col == start && wrapping {
            contents.push('\n');
        }
    }

    pub fn write_contents_formatted(
        &self,
        contents: &mut Vec<u8>,
        start: u16,
        width: u16,
        row: u16,
        wrapping: bool,
        prev_pos: Option<crate::grid::Pos>,
        prev_attrs: Option<crate::attrs::Attrs>,
    ) -> (crate::grid::Pos, crate::attrs::Attrs) {
        let mut prev_was_wide = false;
        let default_cell = crate::cell::Cell::default();

        let mut prev_pos = prev_pos.unwrap_or_else(|| {
            if wrapping {
                crate::grid::Pos {
                    row: row - 1,
                    col: self.cols(),
                }
            } else {
                crate::grid::Pos { row, col: start }
            }
        });
        let mut prev_attrs = prev_attrs.unwrap_or_default();

        let first_cell = &self.cells[usize::from(start)];
        if wrapping && first_cell == &default_cell {
            let default_attrs = default_cell.attrs();
            if &prev_attrs != default_attrs {
                default_attrs.write_escape_code_diff(contents, &prev_attrs);
                prev_attrs = *default_attrs;
            }
            contents.push(b' ');
            crate::term::Backspace::default().write_buf(contents);
            crate::term::EraseChar::new(1).write_buf(contents);
            prev_pos = crate::grid::Pos { row, col: 0 };
        }

        let mut erase: Option<(u16, &crate::attrs::Attrs)> = None;
        for (col, cell) in self
            .cells()
            .enumerate()
            .skip(usize::from(start))
            .take(usize::from(width))
        {
            if prev_was_wide {
                prev_was_wide = false;
                continue;
            }
            prev_was_wide = cell.is_wide();

            // we limit the number of cols to a u16 (see Size)
            let col: u16 = col.try_into().unwrap();
            let pos = crate::grid::Pos { row, col };

            if let Some((prev_col, attrs)) = erase {
                if cell.has_contents() || cell.attrs() != attrs {
                    let new_pos = crate::grid::Pos { row, col: prev_col };
                    if wrapping && prev_pos.row + 1 == new_pos.row && prev_pos.col >= self.cols() {
                        if new_pos.col > 0 {
                            contents.extend(" ".repeat(usize::from(new_pos.col)).as_bytes());
                        } else {
                            contents.extend(b" ");
                            crate::term::Backspace::default().write_buf(contents);
                        }
                    } else {
                        crate::term::MoveFromTo::new(prev_pos, new_pos).write_buf(contents);
                    }
                    prev_pos = new_pos;
                    if &prev_attrs != attrs {
                        attrs.write_escape_code_diff(contents, &prev_attrs);
                        prev_attrs = *attrs;
                    }
                    crate::term::EraseChar::new(pos.col - prev_col).write_buf(contents);
                    erase = None;
                }
            }

            if cell != &default_cell {
                let attrs = cell.attrs();
                if cell.has_contents() {
                    if pos != prev_pos {
                        if !wrapping
                            || prev_pos.row + 1 != pos.row
                            || prev_pos.col < self.cols() - u16::from(cell.is_wide())
                            || pos.col != 0
                        {
                            crate::term::MoveFromTo::new(prev_pos, pos).write_buf(contents);
                        }
                        prev_pos = pos;
                    }

                    if &prev_attrs != attrs {
                        attrs.write_escape_code_diff(contents, &prev_attrs);
                        prev_attrs = *attrs;
                    }

                    prev_pos.col += if cell.is_wide() { 2 } else { 1 };
                    let cell_contents = cell.contents();
                    contents.extend(cell_contents.as_bytes());
                } else if erase.is_none() {
                    erase = Some((pos.col, attrs));
                }
            }
        }
        if let Some((prev_col, attrs)) = erase {
            let new_pos = crate::grid::Pos { row, col: prev_col };
            if wrapping && prev_pos.row + 1 == new_pos.row && prev_pos.col >= self.cols() {
                if new_pos.col > 0 {
                    contents.extend(" ".repeat(usize::from(new_pos.col)).as_bytes());
                } else {
                    contents.extend(b" ");
                    crate::term::Backspace::default().write_buf(contents);
                }
            } else {
                crate::term::MoveFromTo::new(prev_pos, new_pos).write_buf(contents);
            }
            prev_pos = new_pos;
            if &prev_attrs != attrs {
                attrs.write_escape_code_diff(contents, &prev_attrs);
                prev_attrs = *attrs;
            }
            crate::term::ClearRowForward::default().write_buf(contents);
        }

        (prev_pos, prev_attrs)
    }

    // while it's true that most of the logic in this is identical to
    // write_contents_formatted, i can't figure out how to break out the
    // common parts without making things noticeably slower.
    pub fn write_contents_diff(
        &self,
        contents: &mut Vec<u8>,
        prev: &Self,
        start: u16,
        width: u16,
        row: u16,
        wrapping: bool,
        prev_wrapping: bool,
        mut prev_pos: crate::grid::Pos,
        mut prev_attrs: crate::attrs::Attrs,
    ) -> (crate::grid::Pos, crate::attrs::Attrs) {
        let mut prev_was_wide = false;

        let first_cell = &self.cells[usize::from(start)];
        let prev_first_cell = &prev.cells[usize::from(start)];
        if wrapping
            && !prev_wrapping
            && first_cell == prev_first_cell
            && prev_pos.row + 1 == row
            && prev_pos.col >= self.cols() - u16::from(prev_first_cell.is_wide())
        {
            let first_cell_attrs = first_cell.attrs();
            if &prev_attrs != first_cell_attrs {
                first_cell_attrs.write_escape_code_diff(contents, &prev_attrs);
                prev_attrs = *first_cell_attrs;
            }
            let mut cell_contents = prev_first_cell.contents();
            let need_erase = if cell_contents.is_empty() {
                cell_contents = " ".to_string();
                true
            } else {
                false
            };
            contents.extend(cell_contents.as_bytes());
            crate::term::Backspace::default().write_buf(contents);
            if prev_first_cell.is_wide() {
                crate::term::Backspace::default().write_buf(contents);
            }
            if need_erase {
                crate::term::EraseChar::new(1).write_buf(contents);
            }
            prev_pos = crate::grid::Pos { row, col: 0 };
        }

        let mut erase: Option<(u16, &crate::attrs::Attrs)> = None;
        for (col, (cell, prev_cell)) in self
            .cells()
            .zip(prev.cells())
            .enumerate()
            .skip(usize::from(start))
            .take(usize::from(width))
        {
            if prev_was_wide {
                prev_was_wide = false;
                continue;
            }
            prev_was_wide = cell.is_wide();

            // we limit the number of cols to a u16 (see Size)
            let col: u16 = col.try_into().unwrap();
            let pos = crate::grid::Pos { row, col };

            if let Some((prev_col, attrs)) = erase {
                if cell.has_contents() || cell.attrs() != attrs {
                    let new_pos = crate::grid::Pos { row, col: prev_col };
                    if wrapping && prev_pos.row + 1 == new_pos.row && prev_pos.col >= self.cols() {
                        if new_pos.col > 0 {
                            contents.extend(" ".repeat(usize::from(new_pos.col)).as_bytes());
                        } else {
                            contents.extend(b" ");
                            crate::term::Backspace::default().write_buf(contents);
                        }
                    } else {
                        crate::term::MoveFromTo::new(prev_pos, new_pos).write_buf(contents);
                    }
                    prev_pos = new_pos;
                    if &prev_attrs != attrs {
                        attrs.write_escape_code_diff(contents, &prev_attrs);
                        prev_attrs = *attrs;
                    }
                    crate::term::EraseChar::new(pos.col - prev_col).write_buf(contents);
                    erase = None;
                }
            }

            if cell != prev_cell {
                let attrs = cell.attrs();
                if cell.has_contents() {
                    if pos != prev_pos {
                        if !wrapping
                            || prev_pos.row + 1 != pos.row
                            || prev_pos.col < self.cols() - u16::from(cell.is_wide())
                            || pos.col != 0
                        {
                            crate::term::MoveFromTo::new(prev_pos, pos).write_buf(contents);
                        }
                        prev_pos = pos;
                    }

                    if &prev_attrs != attrs {
                        attrs.write_escape_code_diff(contents, &prev_attrs);
                        prev_attrs = *attrs;
                    }

                    prev_pos.col += if cell.is_wide() { 2 } else { 1 };
                    contents.extend(cell.contents().as_bytes());
                } else if erase.is_none() {
                    erase = Some((pos.col, attrs));
                }
            }
        }
        if let Some((prev_col, attrs)) = erase {
            let new_pos = crate::grid::Pos { row, col: prev_col };
            if wrapping && prev_pos.row + 1 == new_pos.row && prev_pos.col >= self.cols() {
                if new_pos.col > 0 {
                    contents.extend(" ".repeat(usize::from(new_pos.col)).as_bytes());
                } else {
                    contents.extend(b" ");
                    crate::term::Backspace::default().write_buf(contents);
                }
            } else {
                crate::term::MoveFromTo::new(prev_pos, new_pos).write_buf(contents);
            }
            prev_pos = new_pos;
            if &prev_attrs != attrs {
                attrs.write_escape_code_diff(contents, &prev_attrs);
                prev_attrs = *attrs;
            }
            crate::term::ClearRowForward::default().write_buf(contents);
        }

        // if this row is going from wrapped to not wrapped, we need to erase
        // and redraw the last character to break wrapping. if this row is
        // wrapped, we need to redraw the last character without erasing it to
        // position the cursor after the end of the line correctly so that
        // drawing the next line can just start writing and be wrapped.
        if (!self.wrapped && prev.wrapped) || (!prev.wrapped && self.wrapped) {
            let end_pos = if self.cells[usize::from(self.cols() - 1)].is_wide_continuation() {
                crate::grid::Pos {
                    row,
                    col: self.cols() - 2,
                }
            } else {
                crate::grid::Pos {
                    row,
                    col: self.cols() - 1,
                }
            };
            crate::term::MoveFromTo::new(prev_pos, end_pos).write_buf(contents);
            prev_pos = end_pos;
            if !self.wrapped {
                crate::term::EraseChar::new(1).write_buf(contents);
            }
            let end_cell = &self.cells[usize::from(end_pos.col)];
            if end_cell.has_contents() {
                let attrs = end_cell.attrs();
                if &prev_attrs != attrs {
                    attrs.write_escape_code_diff(contents, &prev_attrs);
                    prev_attrs = *attrs;
                }
                contents.extend(end_cell.contents().as_bytes());
                prev_pos.col += if end_cell.is_wide() { 2 } else { 1 };
            }
        }

        (prev_pos, prev_attrs)
    }
}
