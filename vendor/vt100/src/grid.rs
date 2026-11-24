use crate::term::BufWrite as _;

#[derive(Clone, Debug)]
pub struct Grid {
    size: Size,
    pos: Pos,
    saved_pos: Pos,
    pub saved_attrs: crate::attrs::Attrs,
    pub saved_charset: u8,
    tab_stops: std::collections::BTreeSet<u16>,
    rows: Vec<crate::row::Row>,
    scroll_top: u16,
    scroll_bottom: u16,
    origin_mode: bool,
    scrollback: std::collections::VecDeque<crate::row::Row>,
    scrollback_len: usize,
    scrollback_offset: usize,
    trimmed_rows: usize,
}

impl Grid {
    pub fn new(size: Size, scrollback_len: usize) -> Self {
        Self {
            size,
            pos: Pos::default(),
            saved_pos: Pos::default(),
            saved_attrs: crate::attrs::Attrs::default(),
            saved_charset: b'B',
            tab_stops: (0..size.cols).step_by(8).collect(),
            rows: vec![],
            scroll_top: 0,
            scroll_bottom: size.rows - 1,
            origin_mode: false,
            scrollback: std::collections::VecDeque::new(),
            scrollback_len,
            scrollback_offset: 0,
            trimmed_rows: 0,
        }
    }

    pub fn allocate_rows(&mut self) {
        if self.rows.is_empty() {
            self.rows.extend(
                std::iter::repeat_with(|| crate::row::Row::new(self.size.cols))
                    .take(usize::from(self.size.rows)),
            );
        }
    }

    pub fn clear(&mut self) {
        self.tab_stops = (0..self.size.cols).step_by(8).collect();
        self.pos = Pos::default();
        self.saved_pos = Pos::default();
        for row in self.drawing_rows_mut() {
            row.clear(crate::attrs::Attrs::default());
        }
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows - 1;
        self.origin_mode = false;
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn set_size(&mut self, size: Size) {
        if self.scroll_bottom == self.size.rows - 1 {
            self.scroll_bottom = size.rows - 1;
        }
        if self.scrollback_len > 0 && !self.rows.is_empty() && size.cols >= 2 {
            self.resize_primary(size);
        } else {
            for row in &mut self.rows {
                row.resize(size.cols, crate::cell::Cell::default());
            }
            self.rows
                .resize_with(usize::from(size.rows), || crate::row::Row::new(size.cols));
            self.saved_pos.row = self.saved_pos.row.min(size.rows - 1);
            self.saved_pos.col = self.saved_pos.col.min(size.cols - 1);
        }
        if self.size != size {
            let start = self
                .tab_stops
                .range(..=size.cols)
                .next_back()
                .copied()
                .unwrap_or(0);
            self.tab_stops.extend((start..size.cols).step_by(8));
        }
        self.size = size;

        if self.scroll_bottom >= size.rows {
            self.scroll_bottom = size.rows - 1;
        }
        if self.scroll_bottom < self.scroll_top {
            self.scroll_top = 0;
        }

        self.row_clamp_top(false);
        self.row_clamp_bottom(false);
        self.col_clamp();
    }

    fn resize_primary(&mut self, size: Size) {
        use crate::reflow::{rewrap, Anchor};
        let base = self.scrollback.len();
        let cursor_row = base + usize::from(self.pos.row);
        let mut anchors = [
            Anchor::new(cursor_row, self.pos.col),
            Anchor::new(base + usize::from(self.saved_pos.row), self.saved_pos.col),
            Anchor::new(base.saturating_sub(self.scrollback_offset), 0),
            Anchor::new(base, 0),
        ];
        let mut lines = std::mem::take(&mut self.scrollback);
        lines.extend(std::mem::take(&mut self.rows));
        if size.cols != self.size.cols {
            lines = rewrap(lines, size.cols, &mut anchors, cursor_row);
        }

        let height = usize::from(size.rows);
        // Consume empty rows below the cursor first. Pull history back into the viewport
        // on growth, and keep the cursor visible on a shorter screen.
        while lines.len() > anchors[3].row + height
            && lines.len() > anchors[0].row + 1
            && lines.back().is_some_and(|r| r.trimmed_len() == 0)
        {
            lines.pop_back();
        }
        let new_base = lines.len().saturating_sub(height).min(anchors[0].row);
        lines.truncate(new_base + height);
        self.rows = lines.split_off(new_base).into();
        self.rows
            .resize_with(height, || crate::row::Row::new(size.cols));
        lines.drain(..new_base.saturating_sub(self.scrollback_len));
        self.scrollback = lines;
        if self.scrollback_offset > 0 {
            self.scrollback_offset = new_base
                .saturating_sub(anchors[2].row)
                .min(self.scrollback.len());
        }
        let visible = |a: &Anchor| Pos {
            row: a.row.saturating_sub(new_base).min(height - 1) as u16,
            col: a.col.min(size.cols - 1),
        };
        self.pos = visible(&anchors[0]);
        self.saved_pos = visible(&anchors[1]);
    }

    pub fn pos(&self) -> Pos {
        self.pos
    }

    pub fn set_pos(&mut self, mut pos: Pos) {
        if self.origin_mode {
            pos.row = pos.row.saturating_add(self.scroll_top);
        }
        self.pos = pos;
        self.row_clamp_top(self.origin_mode);
        self.row_clamp_bottom(self.origin_mode);
        self.col_clamp();
    }

    pub fn activate_from(&mut self, other: &Self) {
        self.pos = other.pos;
        self.origin_mode = other.origin_mode;
    }

    pub fn clear_view(&mut self) {
        if self.scrollback.is_empty() && self.pos.row == 0 {
            return;
        }
        self.rows.swap(0, usize::from(self.pos.row));
        self.rows.truncate(1);
        self.rows.resize_with(usize::from(self.size.rows), || {
            crate::row::Row::new(self.size.cols)
        });
        self.trimmed_rows = self.trimmed_rows.saturating_add(self.scrollback.len());
        self.scrollback.clear();
        self.scrollback_offset = 0;
        self.pos.row = 0;
    }

    // DECSTR resets margins and saved cursor data without moving the live cursor.
    pub fn soft_reset(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows - 1;
        self.origin_mode = false;
        self.saved_pos = Pos::default();
        self.saved_attrs = crate::attrs::Attrs::default();
        self.saved_charset = b'B';
    }

    pub fn save_cursor(&mut self) {
        self.saved_pos = self.pos;
    }

    pub fn restore_cursor(&mut self) {
        self.pos = self.saved_pos;
        self.row_clamp_top(self.origin_mode);
        self.row_clamp_bottom(self.origin_mode);
        self.col_clamp();
    }

    pub fn visible_rows(&self) -> impl Iterator<Item = &crate::row::Row> {
        let scrollback_len = self.scrollback.len();
        let rows_len = self.rows.len();
        self.scrollback
            .iter()
            .skip(scrollback_len - self.scrollback_offset)
            .chain(self.rows.iter())
            .take(rows_len)
    }

    pub fn drawing_rows(&self) -> impl Iterator<Item = &crate::row::Row> {
        self.rows.iter()
    }

    pub fn drawing_rows_mut(&mut self) -> impl Iterator<Item = &mut crate::row::Row> {
        self.rows.iter_mut()
    }

    pub fn visible_row(&self, row: u16) -> Option<&crate::row::Row> {
        self.visible_rows().nth(usize::from(row))
    }

    pub fn drawing_row(&self, row: u16) -> Option<&crate::row::Row> {
        self.drawing_rows().nth(usize::from(row))
    }

    pub fn drawing_row_mut(&mut self, row: u16) -> Option<&mut crate::row::Row> {
        self.drawing_rows_mut().nth(usize::from(row))
    }

    pub fn current_row_mut(&mut self) -> &mut crate::row::Row {
        self.drawing_row_mut(self.pos.row)
            // we assume self.pos.row is always valid
            .unwrap()
    }

    pub fn visible_cell(&self, pos: Pos) -> Option<&crate::cell::Cell> {
        self.visible_row(pos.row).and_then(|r| r.get(pos.col))
    }

    pub fn drawing_cell(&self, pos: Pos) -> Option<&crate::cell::Cell> {
        self.drawing_row(pos.row).and_then(|r| r.get(pos.col))
    }

    pub fn drawing_cell_mut(&mut self, pos: Pos) -> Option<&mut crate::cell::Cell> {
        self.drawing_row_mut(pos.row)
            .and_then(|r| r.get_mut(pos.col))
    }

    pub fn buffer_row(&self, row: usize) -> Option<&crate::row::Row> {
        if row < self.scrollback.len() {
            self.scrollback.get(row)
        } else {
            self.rows.get(row - self.scrollback.len())
        }
    }

    pub fn trimmed_rows(&self) -> usize {
        self.trimmed_rows
    }

    pub fn history_rows(&self) -> usize {
        self.scrollback.len()
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback_len
    }

    pub fn scrollback(&self) -> usize {
        self.scrollback_offset
    }

    pub fn set_scrollback(&mut self, rows: usize) {
        self.scrollback_offset = rows.min(self.scrollback.len());
    }

    pub fn write_contents(&self, contents: &mut String) {
        let mut wrapping = false;
        for row in self.visible_rows() {
            row.write_contents(contents, 0, self.size.cols, wrapping);
            if !row.wrapped() {
                contents.push('\n');
            }
            wrapping = row.wrapped();
        }

        while contents.ends_with('\n') {
            contents.truncate(contents.len() - 1);
        }
    }

    pub fn write_contents_formatted(&self, contents: &mut Vec<u8>) -> crate::attrs::Attrs {
        crate::term::ClearAttrs::default().write_buf(contents);
        crate::term::ClearScreen::default().write_buf(contents);

        let mut prev_attrs = crate::attrs::Attrs::default();
        let mut prev_pos = Pos::default();
        let mut wrapping = false;
        for (i, row) in self.visible_rows().enumerate() {
            // we limit the number of cols to a u16 (see Size), so
            // visible_rows() can never return more rows than will fit
            let i = i.try_into().unwrap();
            let (new_pos, new_attrs) = row.write_contents_formatted(
                contents,
                0,
                self.size.cols,
                i,
                wrapping,
                Some(prev_pos),
                Some(prev_attrs),
            );
            prev_pos = new_pos;
            prev_attrs = new_attrs;
            wrapping = row.wrapped();
        }

        self.write_cursor_position_formatted(contents, Some(prev_pos), Some(prev_attrs));

        prev_attrs
    }

    pub fn write_contents_diff(
        &self,
        contents: &mut Vec<u8>,
        prev: &Self,
        mut prev_attrs: crate::attrs::Attrs,
    ) -> crate::attrs::Attrs {
        let mut prev_pos = prev.pos;
        let mut wrapping = false;
        let mut prev_wrapping = false;
        for (i, (row, prev_row)) in self.visible_rows().zip(prev.visible_rows()).enumerate() {
            // we limit the number of cols to a u16 (see Size), so
            // visible_rows() can never return more rows than will fit
            let i = i.try_into().unwrap();
            let (new_pos, new_attrs) = row.write_contents_diff(
                contents,
                prev_row,
                0,
                self.size.cols,
                i,
                wrapping,
                prev_wrapping,
                prev_pos,
                prev_attrs,
            );
            prev_pos = new_pos;
            prev_attrs = new_attrs;
            wrapping = row.wrapped();
            prev_wrapping = prev_row.wrapped();
        }

        self.write_cursor_position_formatted(contents, Some(prev_pos), Some(prev_attrs));

        prev_attrs
    }

    pub fn write_cursor_position_formatted(
        &self,
        contents: &mut Vec<u8>,
        prev_pos: Option<Pos>,
        prev_attrs: Option<crate::attrs::Attrs>,
    ) {
        let prev_attrs = prev_attrs.unwrap_or_default();
        // writing a character to the last column of a row doesn't wrap the
        // cursor immediately - it waits until the next character is actually
        // drawn. it is only possible for the cursor to have this kind of
        // position after drawing a character though, so if we end in this
        // position, we need to redraw the character at the end of the row.
        if prev_pos != Some(self.pos) && self.pos.col >= self.size.cols {
            let mut pos = Pos {
                row: self.pos.row,
                col: self.size.cols - 1,
            };
            if self
                .drawing_cell(pos)
                // we assume self.pos.row is always valid, and self.size.cols
                // - 1 is always a valid column
                .unwrap()
                .is_wide_continuation()
            {
                pos.col = self.size.cols - 2;
            }
            let cell =
                // we assume self.pos.row is always valid, and self.size.cols
                // - 2 must be a valid column because self.size.cols - 1 is
                // always valid and we just checked that the cell at
                // self.size.cols - 1 is a wide continuation character, which
                // means that the first half of the wide character must be
                // before it
                self.drawing_cell(pos).unwrap();
            if cell.has_contents() {
                if let Some(prev_pos) = prev_pos {
                    crate::term::MoveFromTo::new(prev_pos, pos).write_buf(contents);
                } else {
                    crate::term::MoveTo::new(pos).write_buf(contents);
                }
                cell.attrs().write_escape_code_diff(contents, &prev_attrs);
                contents.extend(cell.contents().as_bytes());
                prev_attrs.write_escape_code_diff(contents, cell.attrs());
            } else {
                // if the cell doesn't have contents, we can't have gotten
                // here by drawing a character in the last column. this means
                // that as far as i'm aware, we have to have reached here from
                // a newline when we were already after the end of an earlier
                // row. in the case where we are already after the end of an
                // earlier row, we can just write a few newlines, otherwise we
                // also need to do the same as above to get ourselves to after
                // the end of a row.
                let mut found = false;
                for i in (0..self.pos.row).rev() {
                    pos.row = i;
                    pos.col = self.size.cols - 1;
                    if self
                        .drawing_cell(pos)
                        // i is always less than self.pos.row, which we assume
                        // to be always valid, so it must also be valid.
                        // self.size.cols - 1 is always a valid col.
                        .unwrap()
                        .is_wide_continuation()
                    {
                        pos.col = self.size.cols - 2;
                    }
                    let cell = self
                        .drawing_cell(pos)
                        // i is always less than self.pos.row, which we assume
                        // to be always valid, so it must also be valid.
                        // self.size.cols - 2 is valid because self.size.cols
                        // - 1 is always valid, and col gets set to
                        // self.size.cols - 2 when the cell at self.size.cols
                        // - 1 is a wide continuation character, meaning that
                        // the first half of the wide character must be before
                        // it
                        .unwrap();
                    if cell.has_contents() {
                        if let Some(prev_pos) = prev_pos {
                            if prev_pos.row != i || prev_pos.col < self.size.cols {
                                crate::term::MoveFromTo::new(prev_pos, pos).write_buf(contents);
                                cell.attrs().write_escape_code_diff(contents, &prev_attrs);
                                contents.extend(cell.contents().as_bytes());
                                prev_attrs.write_escape_code_diff(contents, cell.attrs());
                            }
                        } else {
                            crate::term::MoveTo::new(pos).write_buf(contents);
                            cell.attrs().write_escape_code_diff(contents, &prev_attrs);
                            contents.extend(cell.contents().as_bytes());
                            prev_attrs.write_escape_code_diff(contents, cell.attrs());
                        }
                        contents.extend("\n".repeat(usize::from(self.pos.row - i)).as_bytes());
                        found = true;
                        break;
                    }
                }

                // this can happen if you get the cursor off the end of a row,
                // and then do something to clear the end of the current row
                // without moving the cursor (IL, DL, ED, EL, etc). we know
                // there can't be something in the last column because we
                // would have caught that above, so it should be safe to
                // overwrite it.
                if !found {
                    pos = Pos {
                        row: self.pos.row,
                        col: self.size.cols - 1,
                    };
                    if let Some(prev_pos) = prev_pos {
                        crate::term::MoveFromTo::new(prev_pos, pos).write_buf(contents);
                    } else {
                        crate::term::MoveTo::new(pos).write_buf(contents);
                    }
                    contents.push(b' ');
                    // we know that the cell has no contents, but it still may
                    // have drawing attributes (background color, etc)
                    let end_cell = self
                        .drawing_cell(pos)
                        // we assume self.pos.row is always valid, and
                        // self.size.cols - 1 is always a valid column
                        .unwrap();
                    end_cell
                        .attrs()
                        .write_escape_code_diff(contents, &prev_attrs);
                    crate::term::SaveCursor::default().write_buf(contents);
                    crate::term::Backspace::default().write_buf(contents);
                    crate::term::EraseChar::new(1).write_buf(contents);
                    crate::term::RestoreCursor::default().write_buf(contents);
                    prev_attrs.write_escape_code_diff(contents, end_cell.attrs());
                }
            }
        } else if let Some(prev_pos) = prev_pos {
            crate::term::MoveFromTo::new(prev_pos, self.pos).write_buf(contents);
        } else {
            crate::term::MoveTo::new(self.pos).write_buf(contents);
        }
    }

    pub fn clear_scrollback(&mut self) {
        self.trimmed_rows = self.trimmed_rows.saturating_add(self.scrollback.len());
        self.scrollback.clear();
        self.scrollback_offset = 0;
    }

    pub fn margins(&self) -> (u16, u16) {
        (self.scroll_top, self.scroll_bottom)
    }

    pub fn selective_erase(&mut self, mode: u16, display: bool, attrs: crate::attrs::Attrs) {
        let pos = self.pos;
        let cols = self.size.cols;
        let (first, last) = match (display, mode) {
            (false, 0..=2) => (pos.row, pos.row),
            (true, 0) => (pos.row, self.size.rows - 1),
            (true, 1) => (0, pos.row),
            (true, 2) => (0, self.size.rows - 1),
            (true, 3) => {
                self.clear_scrollback();
                return;
            }
            _ => return,
        };
        for r in first..=last {
            let start = if r == pos.row && mode == 0 {
                pos.col
            } else {
                0
            };
            let end = if r == pos.row && mode == 1 {
                pos.col.saturating_add(1).min(cols)
            } else {
                cols
            };
            let row = self.drawing_row_mut(r).unwrap();
            for col in start..end {
                if !row.get(col).unwrap().attrs().protected() {
                    row.erase(col, attrs);
                }
            }
            if end == cols {
                row.wrap(false);
            }
        }
    }

    pub fn erase_all(&mut self, attrs: crate::attrs::Attrs) {
        for row in self.drawing_rows_mut() {
            row.clear(attrs);
        }
    }

    pub fn erase_all_forward(&mut self, attrs: crate::attrs::Attrs) {
        let pos = self.pos;
        for row in self.drawing_rows_mut().skip(usize::from(pos.row) + 1) {
            row.clear(attrs);
        }

        self.erase_row_forward(attrs);
    }

    pub fn erase_all_backward(&mut self, attrs: crate::attrs::Attrs) {
        let pos = self.pos;
        for row in self.drawing_rows_mut().take(usize::from(pos.row)) {
            row.clear(attrs);
        }

        self.erase_row_backward(attrs);
    }

    pub fn erase_row(&mut self, attrs: crate::attrs::Attrs) {
        self.current_row_mut().clear(attrs);
    }

    pub fn erase_row_forward(&mut self, attrs: crate::attrs::Attrs) {
        let size = self.size;
        let pos = self.pos;
        let row = self.current_row_mut();
        for col in pos.col..size.cols {
            row.erase(col, attrs);
        }
    }

    pub fn erase_row_backward(&mut self, attrs: crate::attrs::Attrs) {
        let size = self.size;
        let pos = self.pos;
        let row = self.current_row_mut();
        for col in 0..=pos.col.min(size.cols - 1) {
            row.erase(col, attrs);
        }
    }

    pub fn alignment_pattern(&mut self, attrs: crate::attrs::Attrs) {
        self.set_pos(Pos::default());
        let row = usize::from(self.pos.row);
        let previous = if row == 0 {
            self.scrollback.back_mut()
        } else {
            self.rows.get_mut(row - 1)
        };
        if let Some(line) = previous {
            line.wrap(false)
        }
        let mut cell = crate::Cell::default();
        cell.set('E', attrs);
        for line in &mut self.rows[row..] {
            line.fill(&cell)
        }
    }

    pub fn shift_columns(&mut self, col: u16, count: u16, right: bool, attrs: crate::attrs::Attrs) {
        if !self.in_scroll_region() {
            return;
        }
        let col = col % self.size.cols;
        let mut blank = crate::Cell::default();
        blank.clear(attrs);
        for i in self.scroll_top..=self.scroll_bottom {
            // Our wrap flag belongs to the preceding physical row.
            let previous = if i > 0 {
                self.rows.get_mut(usize::from(i - 1))
            } else {
                self.scrollback.back_mut()
            };
            if let Some(row) = previous {
                row.wrap(false)
            }
            self.rows[usize::from(i)].shift_columns(col, count, right, &blank);
        }
    }

    pub fn shift_row(&mut self, count: u16, right: bool, attrs: crate::attrs::Attrs) {
        self.col_clamp();
        let col = self.pos.col;
        let mut fill = crate::Cell::default();
        fill.clear(attrs);
        self.current_row_mut()
            .shift_columns(col, count, right, &fill);
    }

    pub fn erase_cells(&mut self, count: u16, attrs: crate::attrs::Attrs) {
        self.col_clamp();
        let size = self.size;
        let pos = self.pos;
        let row = self.current_row_mut();
        for col in pos.col..((pos.col.saturating_add(count)).min(size.cols)) {
            row.erase(col, attrs);
        }
    }

    pub fn edit_lines(&mut self, count: u16, insert: bool, attrs: crate::attrs::Attrs) {
        self.col_clamp();
        if !self.in_scroll_region() {
            return;
        }
        let range = usize::from(self.pos.row)..=usize::from(self.scroll_bottom);
        let rows = &mut self.rows[range];
        let n = usize::from(count).min(rows.len());
        let blank = crate::row::Row::blank(self.size.cols, attrs);
        if insert {
            rows.rotate_right(n);
            rows[..n].fill(blank);
        } else {
            rows.rotate_left(n);
            let start = rows.len() - n;
            rows[start..].fill(blank);
        }
        self.pos.col = 0;
    }

    pub fn scroll_up(&mut self, count: u16, attrs: crate::attrs::Attrs, history: bool) {
        let blank = crate::row::Row::blank(self.size.cols, attrs);
        for _ in 0..count.min(self.scroll_bottom - self.scroll_top + 1) {
            let removed = self.rows.remove(usize::from(self.scroll_top));
            self.rows
                .insert(usize::from(self.scroll_bottom), blank.clone());
            if history && self.scrollback_len > 0 && self.scroll_top == 0 {
                self.scrollback.push_back(removed);
                if self.scrollback.len() > self.scrollback_len {
                    self.scrollback.pop_front();
                    self.trimmed_rows = self.trimmed_rows.saturating_add(1);
                }
                if self.scrollback_offset > 0 {
                    self.scrollback_offset =
                        (self.scrollback_offset + 1).min(self.scrollback.len());
                }
            }
        }
    }
    pub fn scroll_down(&mut self, count: u16, attrs: crate::attrs::Attrs) {
        let rows = &mut self.rows[usize::from(self.scroll_top)..=usize::from(self.scroll_bottom)];
        let n = usize::from(count).min(rows.len());
        rows.rotate_right(n);
        rows[..n].fill(crate::row::Row::blank(self.size.cols, attrs));
    }
    pub fn set_scroll_region(&mut self, top: u16, bottom: u16) {
        let bottom = bottom.min(self.size().rows - 1);
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        } else {
            self.scroll_top = 0;
            self.scroll_bottom = self.size().rows - 1;
        }
        self.set_pos(Pos::default());
    }

    fn in_scroll_region(&self) -> bool {
        self.pos.row >= self.scroll_top && self.pos.row <= self.scroll_bottom
    }

    pub fn origin_mode(&self) -> bool {
        self.origin_mode
    }

    pub fn backspace(&mut self, reverse: bool) {
        if !reverse {
            self.col_clamp();
        }
        if self.pos.col > 0 {
            self.col_dec(1);
        } else if reverse && self.pos.row > self.scroll_top && self.pos.row <= self.scroll_bottom {
            let previous = self.pos.row - 1;
            if self.rows[usize::from(previous)].wrapped() {
                self.rows[usize::from(previous)].wrap(false);
                self.pos.row = previous;
                self.pos.col = self.size.cols - 1;
                if self
                    .drawing_cell(self.pos)
                    .is_some_and(|c| !c.has_contents() && !c.is_wide_continuation())
                {
                    self.col_dec(1);
                }
            }
        }
    }

    pub fn set_origin_mode(&mut self, mode: bool) {
        self.origin_mode = mode;
        self.set_pos(Pos { row: 0, col: 0 });
    }

    /// Match xterm's relative cursor movement, including its origin-mode offset.
    pub fn move_cursor(&mut self, dx: i32, dy: i32) {
        self.col_clamp();
        let y = i32::from(self.pos.row);
        let row = if dy < 0 && self.pos.row >= self.scroll_top {
            (y + dy).max(i32::from(self.scroll_top))
        } else if dy > 0 && self.pos.row <= self.scroll_bottom {
            (y + dy).min(i32::from(self.scroll_bottom))
        } else {
            y + dy
        };
        self.set_pos(Pos {
            row: row.clamp(0, i32::from(u16::MAX)) as u16,
            col: (i32::from(self.pos.col) + dx).clamp(0, i32::from(u16::MAX)) as u16,
        });
    }
    pub fn index(&mut self, attrs: crate::attrs::Attrs) -> bool {
        if self.pos.row == self.scroll_bottom {
            self.scroll_up(1, attrs, true);
            true
        } else {
            self.pos.row = (self.pos.row + 1).min(self.size.rows - 1);
            false
        }
    }
    pub fn reverse_index(&mut self, attrs: crate::attrs::Attrs) {
        self.col_clamp();
        if self.pos.row == self.scroll_top {
            self.scroll_down(1, attrs)
        } else {
            self.pos.row = self.pos.row.saturating_sub(1)
        }
    }
    pub fn row_set(&mut self, i: u16) {
        self.pos.row = i;
        self.row_clamp();
    }

    pub fn col_inc(&mut self, count: u16) {
        self.pos.col = self.pos.col.saturating_add(count);
    }

    pub fn col_dec(&mut self, count: u16) {
        self.pos.col = self.pos.col.saturating_sub(count);
    }

    pub fn set_tab_stop(&mut self) {
        self.tab_stops.insert(self.pos.col);
    }

    pub fn clear_tab_stops(&mut self, mode: u16) {
        match mode {
            0 => {
                self.tab_stops.remove(&self.pos.col);
            }
            3 => self.tab_stops.clear(),
            _ => {}
        }
    }

    pub fn move_tabs(&mut self, count: u16, forward: bool) {
        let col = self.pos.col;
        if col >= self.size.cols {
            return;
        }
        let steps = usize::from(count.max(1) - 1);
        self.pos.col = if forward {
            self.tab_stops
                .range(col + 1..self.size.cols)
                .nth(steps)
                .copied()
                .unwrap_or(self.size.cols - 1)
        } else {
            self.tab_stops
                .range(..col)
                .rev()
                .nth(steps)
                .copied()
                .unwrap_or(0)
        };
    }

    pub fn col_set(&mut self, i: u16) {
        self.pos.col = i;
        self.col_clamp();
    }

    pub fn col_wrap(&mut self, width: u16, wrap: bool, attrs: crate::attrs::Attrs) {
        if self.pos.col > self.size.cols - width {
            let mut prev_pos = self.pos;
            self.pos.col = 0;
            if self.index(attrs) {
                let Some(previous) = prev_pos.row.checked_sub(1) else {
                    return;
                };
                prev_pos.row = previous;
            }
            let new_pos = self.pos;
            self.drawing_row_mut(prev_pos.row)
                // we assume self.pos.row is always valid, and so prev_pos.row
                // must be valid because it is always less than or equal to
                // self.pos.row
                .unwrap()
                .wrap(wrap && prev_pos.row + 1 == new_pos.row);
        }
    }

    fn row_clamp_top(&mut self, limit_to_scroll_region: bool) -> u16 {
        if limit_to_scroll_region && self.pos.row < self.scroll_top {
            let rows = self.scroll_top - self.pos.row;
            self.pos.row = self.scroll_top;
            rows
        } else {
            0
        }
    }

    fn row_clamp_bottom(&mut self, limit_to_scroll_region: bool) -> u16 {
        let bottom = if limit_to_scroll_region {
            self.scroll_bottom
        } else {
            self.size.rows - 1
        };
        if self.pos.row > bottom {
            let rows = self.pos.row - bottom;
            self.pos.row = bottom;
            rows
        } else {
            0
        }
    }

    fn row_clamp(&mut self) {
        if self.pos.row > self.size.rows - 1 {
            self.pos.row = self.size.rows - 1;
        }
    }

    fn col_clamp(&mut self) {
        if self.pos.col > self.size.cols - 1 {
            self.pos.col = self.size.cols - 1;
        }
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct Size {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct Pos {
    pub row: u16,
    pub col: u16,
}
