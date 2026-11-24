use crate::term::BufWrite as _;

const MODE_APPLICATION_KEYPAD: u8 = 0b0000_0001;
const MODE_APPLICATION_CURSOR: u8 = 0b0000_0010;
const MODE_HIDE_CURSOR: u8 = 0b0000_0100;
const MODE_ALTERNATE_SCREEN: u8 = 0b0000_1000;
const MODE_NO_WRAP: u8 = 0b0010_0000;
const MODE_INSERT: u8 = 0b0100_0000;
const MODE_NEWLINE: u8 = 0b1000_0000;
const MODE_BRACKETED_PASTE: u8 = 0b0001_0000;

/// The xterm mouse handling mode currently in use.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MouseProtocolMode {
    /// Mouse handling is disabled.
    None,

    /// Mouse button events should be reported on button press. Also known as
    /// X10 mouse mode.
    Press,

    /// Mouse button events should be reported on button press and release.
    /// Also known as VT200 mouse mode.
    PressRelease,

    // Highlight,
    /// Mouse button events should be reported on button press and release, as
    /// well as when the mouse moves between cells while a button is held
    /// down.
    ButtonMotion,

    /// Mouse button events should be reported on button press and release,
    /// and mouse motion events should be reported when the mouse moves
    /// between cells regardless of whether a button is held down or not.
    AnyMotion,
    // DecLocator,
}

impl Default for MouseProtocolMode {
    fn default() -> Self {
        Self::None
    }
}

/// The encoding to use for the enabled `MouseProtocolMode`.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MouseProtocolEncoding {
    /// Default single-printable-byte encoding.
    Default,

    /// SGR-like encoding.
    Sgr,
    /// SGR with zero-based logical pixel coordinates (DECSET 1016).
    SgrPixels,
}

impl Default for MouseProtocolEncoding {
    fn default() -> Self {
        Self::Default
    }
}

/// Shape selected by DECSCUSR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

/// Cursor and buffer coordinates of the frame being displayed.
#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    pub cursor: Option<(u16, u16)>,
    pub cursor_initialized: bool,
    pub blink: bool,
    pub shape: CursorShape,
    pub first_row: usize,
}

#[derive(Clone, Debug)]
struct FrozenFrame {
    palette: crate::Palette,
    rows: Vec<crate::row::Row>,
    info: FrameInfo,
}

/// Represents the overall terminal state.
#[derive(Clone, Debug)]
pub struct Screen {
    grid: crate::grid::Grid,
    alternate_grid: crate::grid::Grid,

    attrs: crate::attrs::Attrs,
    charsets: crate::charset::Charsets,

    title: String,
    icon_name: String,

    pub(crate) palette: crate::Palette,
    modes: u8,
    reverse_wrap: bool,
    sync_started: Option<std::time::Instant>,
    pub(crate) capture_frames: bool,
    frozen: Option<FrozenFrame>,
    cursor_shape: Option<CursorShape>,
    cursor_blink: Option<bool>,
    default_cursor_blink: bool,
    cursor_initialized: bool,
    preceding_printable: bool,
    hyperlink: Option<std::sync::Arc<crate::Hyperlink>>,
    buffer_epoch: usize,
    focus_tracking: bool,
    focus_requests: u64,
    mouse_protocol_mode: MouseProtocolMode,
    mouse_protocol_encoding: MouseProtocolEncoding,

    audible_bell_count: usize,
    visual_bell_count: usize,

    errors: usize,
    pub(crate) replies: Option<Vec<u8>>,
    pub(crate) status_query: Option<Vec<u8>>,
}

impl Screen {
    pub(crate) fn new(size: crate::grid::Size, scrollback_len: usize) -> Self {
        let mut grid = crate::grid::Grid::new(size, scrollback_len);
        grid.allocate_rows();
        Self {
            grid,
            alternate_grid: crate::grid::Grid::new(size, 0),

            attrs: crate::attrs::Attrs::default(),
            charsets: crate::charset::Charsets::default(),

            title: String::default(),
            icon_name: String::default(),

            palette: Default::default(),
            modes: 0,
            reverse_wrap: false,
            sync_started: None,
            capture_frames: false,
            frozen: None,
            cursor_shape: None,
            cursor_blink: None,
            default_cursor_blink: true,
            cursor_initialized: false,
            preceding_printable: false,
            hyperlink: None,
            buffer_epoch: 0,
            focus_tracking: false,
            focus_requests: 0,
            mouse_protocol_mode: MouseProtocolMode::default(),
            mouse_protocol_encoding: MouseProtocolEncoding::default(),

            audible_bell_count: 0,
            visual_bell_count: 0,

            errors: 0,
            replies: None,
            status_query: None,
        }
    }

    pub(crate) fn set_size(&mut self, rows: u16, cols: u16) {
        self.frozen = None;
        self.grid.set_size(crate::grid::Size { rows, cols });
        self.alternate_grid
            .set_size(crate::grid::Size { rows, cols });
    }

    /// Returns the current size of the terminal.
    ///
    /// The return value will be (rows, cols).
    #[must_use]
    pub fn size(&self) -> (u16, u16) {
        let size = self.grid().size();
        (size.rows, size.cols)
    }

    /// Returns the current position in the scrollback.
    ///
    /// This position indicates the offset from the top of the screen, and is
    /// `0` when the normal screen is in view.
    #[must_use]
    pub fn scrollback(&self) -> usize {
        self.grid().scrollback()
    }

    /// Number of retained rows preceding the active screen.
    pub fn history_rows(&self) -> usize {
        self.grid().history_rows()
    }

    /// Identifies buffer replacement and rows permanently evicted from its front.
    pub fn history_origin(&self) -> (usize, usize) {
        (self.buffer_epoch, self.grid().trimmed_rows())
    }

    /// First visible row counted from the oldest retained row.
    pub fn viewport_start(&self) -> usize {
        self.history_rows() - self.scrollback()
    }

    /// Cell in the combined history and live screen.
    pub fn buffer_cell(&self, row: usize, col: u16) -> Option<&crate::cell::Cell> {
        self.grid().buffer_row(row)?.get(col)
    }

    /// True when the next physical row continues this line.
    pub fn buffer_row_wrapped(&self, row: usize) -> bool {
        self.grid()
            .buffer_row(row)
            .is_some_and(crate::row::Row::wrapped)
    }

    /// Selected text in retained buffer coordinates. Soft wraps do not introduce a newline.
    pub fn buffer_text(&self, start: (usize, u16), end: (usize, u16)) -> String {
        let mut text = String::new();
        let cols = self.size().1;
        if start >= end {
            return text;
        }
        for r in start.0..=end.0 {
            let Some(row) = self.grid().buffer_row(r) else {
                break;
            };
            let left = if r == start.0 { start.1.min(cols) } else { 0 };
            let right = if r == end.0 { end.1.min(cols) } else { cols };
            row.write_range(&mut text, left, right);
            if r < end.0 && !row.wrapped() {
                text.push('\n');
            }
        }
        text
    }

    pub(crate) fn set_scrollback(&mut self, rows: usize) {
        self.grid_mut().set_scrollback(rows);
    }

    pub(crate) fn clear_view(&mut self) {
        self.grid_mut().clear_view();
    }

    /// Returns the text contents of the terminal.
    ///
    /// This will not include any formatting information, and will be in plain
    /// text format.
    #[must_use]
    pub fn contents(&self) -> String {
        let mut contents = String::new();
        self.write_contents(&mut contents);
        contents
    }

    fn write_contents(&self, contents: &mut String) {
        self.grid().write_contents(contents);
    }

    /// Returns the text contents of the terminal by row, restricted to the
    /// given subset of columns.
    ///
    /// This will not include any formatting information, and will be in plain
    /// text format.
    ///
    /// Newlines will not be included.
    pub fn rows(&self, start: u16, width: u16) -> impl Iterator<Item = String> + '_ {
        self.grid().visible_rows().map(move |row| {
            let mut contents = String::new();
            row.write_range(&mut contents, start, start.saturating_add(width));
            contents
        })
    }

    /// Returns the text contents of the terminal logically between two cells.
    /// This will include the remainder of the starting row after `start_col`,
    /// followed by the entire contents of the rows between `start_row` and
    /// `end_row`, followed by the beginning of the `end_row` up until
    /// `end_col`. This is useful for things like determining the contents of
    /// a clipboard selection.
    #[must_use]
    pub fn contents_between(
        &self,
        start_row: u16,
        start_col: u16,
        end_row: u16,
        end_col: u16,
    ) -> String {
        match start_row.cmp(&end_row) {
            std::cmp::Ordering::Less => {
                let (_, cols) = self.size();
                let mut contents = String::new();
                for (i, row) in self
                    .grid()
                    .visible_rows()
                    .enumerate()
                    .skip(usize::from(start_row))
                    .take(usize::from(end_row) - usize::from(start_row) + 1)
                {
                    if i == usize::from(start_row) {
                        row.write_contents(&mut contents, start_col, cols - start_col, false);
                        if !row.wrapped() {
                            contents.push('\n');
                        }
                    } else if i == usize::from(end_row) {
                        row.write_contents(&mut contents, 0, end_col, false);
                    } else {
                        row.write_contents(&mut contents, 0, cols, false);
                        if !row.wrapped() {
                            contents.push('\n');
                        }
                    }
                }
                contents
            }
            std::cmp::Ordering::Equal => {
                if start_col < end_col {
                    self.rows(start_col, end_col - start_col)
                        .nth(usize::from(start_row))
                        .unwrap_or_default()
                } else {
                    String::new()
                }
            }
            std::cmp::Ordering::Greater => String::new(),
        }
    }

    /// Return escape codes sufficient to reproduce the entire contents of the
    /// current terminal state. This is a convenience wrapper around
    /// `contents_formatted`, `input_mode_formatted`, and `title_formatted`.
    #[must_use]
    pub fn state_formatted(&self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_contents_formatted(&mut contents);
        self.write_input_mode_formatted(&mut contents);
        self.write_title_formatted(&mut contents);
        contents
    }

    /// Return escape codes sufficient to turn the terminal state of the
    /// screen `prev` into the current terminal state. This is a convenience
    /// wrapper around `contents_diff`, `input_mode_diff`, `title_diff`, and
    /// `bells_diff`.
    #[must_use]
    pub fn state_diff(&self, prev: &Self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_contents_diff(&mut contents, prev);
        self.write_input_mode_diff(&mut contents, prev);
        self.write_title_diff(&mut contents, prev);
        self.write_bells_diff(&mut contents, prev);
        contents
    }

    /// Returns the formatted visible contents of the terminal.
    ///
    /// Formatting information will be included inline as terminal escape
    /// codes. The result will be suitable for feeding directly to a raw
    /// terminal parser, and will result in the same visual output.
    #[must_use]
    pub fn contents_formatted(&self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_contents_formatted(&mut contents);
        contents
    }

    fn write_contents_formatted(&self, contents: &mut Vec<u8>) {
        self.write_cursor_style(contents, None);
        crate::term::HideCursor::new(self.hide_cursor()).write_buf(contents);
        let prev_attrs = self.grid().write_contents_formatted(contents);
        self.attrs.write_escape_code_diff(contents, &prev_attrs);
    }

    /// Returns the formatted visible contents of the terminal by row,
    /// restricted to the given subset of columns.
    ///
    /// Formatting information will be included inline as terminal escape
    /// codes. The result will be suitable for feeding directly to a raw
    /// terminal parser, and will result in the same visual output.
    ///
    /// You are responsible for positioning the cursor before printing each
    /// row, and the final cursor position after displaying each row is
    /// unspecified.
    // the unwraps in this method shouldn't be reachable
    #[allow(clippy::missing_panics_doc)]
    pub fn rows_formatted(&self, start: u16, width: u16) -> impl Iterator<Item = Vec<u8>> + '_ {
        let mut wrapping = false;
        self.grid().visible_rows().enumerate().map(move |(i, row)| {
            // number of rows in a grid is stored in a u16 (see Size), so
            // visible_rows can never return enough rows to overflow here
            let i = i.try_into().unwrap();
            let mut contents = vec![];
            row.write_contents_formatted(&mut contents, start, width, i, wrapping, None, None);
            if start == 0 && width == self.grid.size().cols {
                wrapping = row.wrapped();
            }
            contents
        })
    }

    /// Returns a terminal byte stream sufficient to turn the visible contents
    /// of the screen described by `prev` into the visible contents of the
    /// screen described by `self`.
    ///
    /// The result of rendering `prev.contents_formatted()` followed by
    /// `self.contents_diff(prev)` should be equivalent to the result of
    /// rendering `self.contents_formatted()`. This is primarily useful when
    /// you already have a terminal parser whose state is described by `prev`,
    /// since the diff will likely require less memory and cause less
    /// flickering than redrawing the entire screen contents.
    #[must_use]
    pub fn contents_diff(&self, prev: &Self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_contents_diff(&mut contents, prev);
        contents
    }

    fn write_contents_diff(&self, contents: &mut Vec<u8>, prev: &Self) {
        self.write_cursor_style(contents, Some(prev));
        if self.hide_cursor() != prev.hide_cursor() {
            crate::term::HideCursor::new(self.hide_cursor()).write_buf(contents);
        }
        let prev_attrs = self
            .grid()
            .write_contents_diff(contents, prev.grid(), prev.attrs);
        self.attrs.write_escape_code_diff(contents, &prev_attrs);
    }

    /// Returns a sequence of terminal byte streams sufficient to turn the
    /// visible contents of the subset of each row from `prev` (as described
    /// by `start` and `width`) into the visible contents of the corresponding
    /// row subset in `self`.
    ///
    /// You are responsible for positioning the cursor before printing each
    /// row, and the final cursor position after displaying each row is
    /// unspecified.
    // the unwraps in this method shouldn't be reachable
    #[allow(clippy::missing_panics_doc)]
    pub fn rows_diff<'a>(
        &'a self,
        prev: &'a Self,
        start: u16,
        width: u16,
    ) -> impl Iterator<Item = Vec<u8>> + 'a {
        self.grid()
            .visible_rows()
            .zip(prev.grid().visible_rows())
            .enumerate()
            .map(move |(i, (row, prev_row))| {
                // number of rows in a grid is stored in a u16 (see Size), so
                // visible_rows can never return enough rows to overflow here
                let i = i.try_into().unwrap();
                let mut contents = vec![];
                row.write_contents_diff(
                    &mut contents,
                    prev_row,
                    start,
                    width,
                    i,
                    false,
                    false,
                    crate::grid::Pos { row: i, col: start },
                    crate::attrs::Attrs::default(),
                );
                contents
            })
    }

    /// Returns terminal escape sequences sufficient to set the current
    /// terminal's input modes.
    ///
    /// Supported modes are:
    /// * application keypad
    /// * application cursor
    /// * bracketed paste
    /// * xterm mouse support
    #[must_use]
    pub fn input_mode_formatted(&self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_input_mode_formatted(&mut contents);
        contents
    }

    fn write_input_mode_formatted(&self, contents: &mut Vec<u8>) {
        crate::term::ApplicationKeypad::new(self.mode(MODE_APPLICATION_KEYPAD)).write_buf(contents);
        crate::term::ApplicationCursor::new(self.mode(MODE_APPLICATION_CURSOR)).write_buf(contents);
        crate::term::BracketedPaste::new(self.mode(MODE_BRACKETED_PASTE)).write_buf(contents);
        if self.focus_tracking {
            contents.extend_from_slice(b"\x1b[?1004h");
        }
        crate::term::MouseProtocolMode::new(self.mouse_protocol_mode, MouseProtocolMode::None)
            .write_buf(contents);
        crate::term::MouseProtocolEncoding::new(
            self.mouse_protocol_encoding,
            MouseProtocolEncoding::Default,
        )
        .write_buf(contents);
    }

    /// Returns terminal escape sequences sufficient to change the previous
    /// terminal's input modes to the input modes enabled in the current
    /// terminal.
    #[must_use]
    pub fn input_mode_diff(&self, prev: &Self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_input_mode_diff(&mut contents, prev);
        contents
    }

    fn write_input_mode_diff(&self, contents: &mut Vec<u8>, prev: &Self) {
        if self.mode(MODE_APPLICATION_KEYPAD) != prev.mode(MODE_APPLICATION_KEYPAD) {
            crate::term::ApplicationKeypad::new(self.mode(MODE_APPLICATION_KEYPAD))
                .write_buf(contents);
        }
        if self.mode(MODE_APPLICATION_CURSOR) != prev.mode(MODE_APPLICATION_CURSOR) {
            crate::term::ApplicationCursor::new(self.mode(MODE_APPLICATION_CURSOR))
                .write_buf(contents);
        }
        if self.mode(MODE_BRACKETED_PASTE) != prev.mode(MODE_BRACKETED_PASTE) {
            crate::term::BracketedPaste::new(self.mode(MODE_BRACKETED_PASTE)).write_buf(contents);
        }
        if self.focus_tracking != prev.focus_tracking {
            contents.extend_from_slice(if self.focus_tracking {
                b"\x1b[?1004h"
            } else {
                b"\x1b[?1004l"
            });
        }
        crate::term::MouseProtocolMode::new(self.mouse_protocol_mode, prev.mouse_protocol_mode)
            .write_buf(contents);
        crate::term::MouseProtocolEncoding::new(
            self.mouse_protocol_encoding,
            prev.mouse_protocol_encoding,
        )
        .write_buf(contents);
    }

    /// Returns terminal escape sequences sufficient to set the current
    /// terminal's window title.
    #[must_use]
    pub fn title_formatted(&self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_title_formatted(&mut contents);
        contents
    }

    fn write_title_formatted(&self, contents: &mut Vec<u8>) {
        crate::term::ChangeTitle::new(&self.icon_name, &self.title, "", "").write_buf(contents);
    }

    /// Returns terminal escape sequences sufficient to change the previous
    /// terminal's window title to the window title set in the current
    /// terminal.
    #[must_use]
    pub fn title_diff(&self, prev: &Self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_title_diff(&mut contents, prev);
        contents
    }

    fn write_title_diff(&self, contents: &mut Vec<u8>, prev: &Self) {
        crate::term::ChangeTitle::new(&self.icon_name, &self.title, &prev.icon_name, &prev.title)
            .write_buf(contents);
    }

    /// Returns terminal escape sequences sufficient to cause audible and
    /// visual bells to occur if they have been received since the terminal
    /// described by `prev`.
    #[must_use]
    pub fn bells_diff(&self, prev: &Self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_bells_diff(&mut contents, prev);
        contents
    }

    fn write_bells_diff(&self, contents: &mut Vec<u8>, prev: &Self) {
        if self.audible_bell_count != prev.audible_bell_count {
            crate::term::AudibleBell::default().write_buf(contents);
        }
        if self.visual_bell_count != prev.visual_bell_count {
            crate::term::VisualBell::default().write_buf(contents);
        }
    }

    /// Returns terminal escape sequences sufficient to set the current
    /// terminal's drawing attributes.
    ///
    /// Supported drawing attributes are:
    /// * fgcolor
    /// * bgcolor
    /// * bold
    /// * italic
    /// * underline
    /// * inverse
    ///
    /// This is not typically necessary, since `contents_formatted` will leave
    /// the current active drawing attributes in the correct state, but this
    /// can be useful in the case of drawing additional things on top of a
    /// terminal output, since you will need to restore the terminal state
    /// without the terminal contents necessarily being the same.
    #[must_use]
    pub fn attributes_formatted(&self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_attributes_formatted(&mut contents);
        contents
    }

    fn write_attributes_formatted(&self, contents: &mut Vec<u8>) {
        crate::term::ClearAttrs::default().write_buf(contents);
        self.attrs
            .write_escape_code_diff(contents, &crate::attrs::Attrs::default());
    }

    /// Returns the current cursor position of the terminal.
    ///
    /// The return value will be (row, col).
    #[must_use]
    pub fn cursor_position(&self) -> (u16, u16) {
        let pos = self.grid().pos();
        (pos.row, pos.col)
    }

    /// Returns terminal escape sequences sufficient to set the current
    /// cursor state of the terminal.
    ///
    /// This is not typically necessary, since `contents_formatted` will leave
    /// the cursor in the correct state, but this can be useful in the case of
    /// drawing additional things on top of a terminal output, since you will
    /// need to restore the terminal state without the terminal contents
    /// necessarily being the same.
    ///
    /// Note that the bytes returned by this function may alter the active
    /// drawing attributes, because it may require redrawing existing cells in
    /// order to position the cursor correctly (for instance, in the case
    /// where the cursor is past the end of a row). Therefore, you should
    /// ensure to reset the active drawing attributes if necessary after
    /// processing this data, for instance by using `attributes_formatted`.
    #[must_use]
    pub fn cursor_state_formatted(&self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_cursor_state_formatted(&mut contents);
        contents
    }

    fn write_cursor_state_formatted(&self, contents: &mut Vec<u8>) {
        self.write_cursor_style(contents, None);
        crate::term::HideCursor::new(self.hide_cursor()).write_buf(contents);
        self.grid()
            .write_cursor_position_formatted(contents, None, None);

        // we don't just call write_attributes_formatted here, because that
        // would still be confusing - consider the case where the user sets
        // their own unrelated drawing attributes (on a different parser
        // instance) and then calls cursor_state_formatted. just documenting
        // it and letting the user handle it on their own is more
        // straightforward.
    }

    /// Returns the `Cell` object at the given location in the terminal, if it
    /// exists.
    #[must_use]
    pub fn cell(&self, row: u16, col: u16) -> Option<&crate::cell::Cell> {
        self.grid().visible_cell(crate::grid::Pos { row, col })
    }

    /// Returns whether the text in row `row` should wrap to the next line.
    #[must_use]
    pub fn row_wrapped(&self, row: u16) -> bool {
        self.grid()
            .visible_row(row)
            .map_or(false, crate::row::Row::wrapped)
    }

    /// Returns the terminal's window title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the terminal's icon name.
    #[must_use]
    pub fn icon_name(&self) -> &str {
        &self.icon_name
    }

    /// Returns a value which changes every time an audible bell is received.
    ///
    /// Typically you would store this number after each call to `process`,
    /// and trigger an audible bell whenever it changes.
    ///
    /// You shouldn't rely on the exact value returned here, since the exact
    /// value will not be maintained by `contents_formatted` or
    /// `contents_diff`.
    #[must_use]
    pub fn audible_bell_count(&self) -> usize {
        self.audible_bell_count
    }

    /// Returns a value which changes every time an visual bell is received.
    ///
    /// Typically you would store this number after each call to `process`,
    /// and trigger an visual bell whenever it changes.
    ///
    /// You shouldn't rely on the exact value returned here, since the exact
    /// value will not be maintained by `contents_formatted` or
    /// `contents_diff`.
    #[must_use]
    pub fn visual_bell_count(&self) -> usize {
        self.visual_bell_count
    }

    /// Returns the number of parsing errors seen so far.
    ///
    /// Currently this only tracks invalid UTF-8 and control characters other
    /// than `0x07`-`0x0f`. This can give an idea of whether the input stream
    /// being fed to the parser is reasonable or not.
    #[must_use]
    pub fn errors(&self) -> usize {
        self.errors
    }

    /// Returns whether the alternate screen is currently in use.
    #[must_use]
    pub fn alternate_screen(&self) -> bool {
        self.mode(MODE_ALTERNATE_SCREEN)
    }

    /// Returns whether the terminal should be in application keypad mode.
    #[must_use]
    pub fn application_keypad(&self) -> bool {
        self.mode(MODE_APPLICATION_KEYPAD)
    }

    /// Returns whether the terminal should be in application cursor mode.
    #[must_use]
    pub fn application_cursor(&self) -> bool {
        self.mode(MODE_APPLICATION_CURSOR)
    }

    /// The desktop and browser clients both default to a bar cursor.
    pub fn cursor_shape(&self) -> CursorShape {
        self.cursor_shape.unwrap_or(CursorShape::Bar)
    }

    /// DECSCUSR takes precedence over the default configured using DEC mode 12.
    pub fn cursor_blinks(&self) -> bool {
        self.cursor_blink.unwrap_or(self.default_cursor_blink)
    }

    /// Switching buffers initializes the cursor even before the view has received focus.
    pub fn cursor_initialized(&self) -> bool {
        self.cursor_initialized
    }

    /// Cursor position in the visible viewport, including pending right-margin wrap.
    pub fn visible_cursor(&self) -> Option<(u16, u16)> {
        let (row, col) = self.cursor_position();
        let (rows, cols) = self.size();
        let row = usize::from(row).checked_add(self.scrollback())?;
        if row >= usize::from(rows) || self.hide_cursor() {
            return None;
        }
        let row = u16::try_from(row).ok()?;
        let col = col.min(cols - 1);
        let cell = self.cell(row, col)?;
        (!cell.is_wide_continuation()).then_some((row, col))
    }

    fn set_cursor_style(&mut self, param: u16) {
        if param == 0 {
            self.cursor_shape = None;
            self.cursor_blink = None;
        } else {
            self.cursor_shape = match param {
                1 | 2 => Some(CursorShape::Block),
                3 | 4 => Some(CursorShape::Underline),
                5 | 6 => Some(CursorShape::Bar),
                _ => self.cursor_shape,
            };
            self.cursor_blink = Some(param % 2 == 1);
        }
    }

    fn write_cursor_style(&self, out: &mut Vec<u8>, previous: Option<&Self>) {
        use std::io::Write as _;
        if previous.map_or(true, |p| p.default_cursor_blink) != self.default_cursor_blink {
            out.extend_from_slice(if self.default_cursor_blink {
                b"\x1b[?12h"
            } else {
                b"\x1b[?12l"
            });
        }
        let old = previous.map_or((None, None), |p| (p.cursor_shape, p.cursor_blink));
        if old == (self.cursor_shape, self.cursor_blink) {
            return;
        }
        if self.cursor_shape.is_none() {
            out.extend_from_slice(b"\x1b[0 q");
        }
        if let Some(blinks) = self.cursor_blink {
            let steady = match self.cursor_shape {
                Some(CursorShape::Block) => 2,
                Some(CursorShape::Underline) => 4,
                Some(CursorShape::Bar) => 6,
                None => 8,
            };
            write!(out, "\x1b[{} q", steady - i32::from(blinks)).unwrap();
        }
    }

    /// Colors belonging to the currently published visible frame.
    pub fn frame_palette(&self) -> &crate::Palette {
        self.frozen.as_ref().map_or(&self.palette, |f| &f.palette)
    }
    fn current_frame(&self) -> FrameInfo {
        FrameInfo {
            cursor: self.visible_cursor(),
            cursor_initialized: self.cursor_initialized(),
            blink: self.cursor_blinks(),
            shape: self.cursor_shape(),
            first_row: self.viewport_start(),
        }
    }

    /// The most recently published frame can differ from the live parser state during a synchronized update.
    pub fn frame_info(&self) -> FrameInfo {
        self.frozen
            .as_ref()
            .map_or_else(|| self.current_frame(), |frame| frame.info)
    }

    /// Cells of a visible row in the published frame.
    pub fn frame_row(&self, row: u16) -> &[crate::Cell] {
        let row = match &self.frozen {
            Some(frame) => frame.rows.get(usize::from(row)),
            None => self.grid().visible_row(row),
        };
        row.map_or(&[], crate::row::Row::as_slice)
    }

    fn start_synchronized_output(&mut self) {
        if self.sync_started.is_none() {
            if self.capture_frames {
                self.frozen = Some(FrozenFrame {
                    palette: self.palette.clone(),
                    rows: self.grid().visible_rows().cloned().collect(),
                    info: self.current_frame(),
                });
            }
            self.sync_started = Some(std::time::Instant::now());
        }
    }

    fn end_synchronized_output(&mut self) {
        self.frozen = None;
        self.sync_started = None;
    }

    pub(crate) fn expire_frame(&mut self) -> bool {
        if self
            .sync_started
            .is_some_and(|started| started.elapsed() >= std::time::Duration::from_secs(1))
        {
            self.end_synchronized_output();
            return true;
        }
        false
    }

    fn mode_report(&self, mode: u16, private: bool) -> u8 {
        // DECRPM distinguishes switchable, permanent and unrecognized modes.
        let enabled = if private {
            match mode {
                1 => self.application_cursor(),
                6 => self.grid().origin_mode(),
                7 => !self.mode(MODE_NO_WRAP),
                8 => return 3,
                9 => self.mouse_protocol_mode == MouseProtocolMode::Press,
                12 => self.default_cursor_blink,
                25 => !self.hide_cursor(),
                45 => self.reverse_wrap,
                47 | 1047 | 1049 => self.alternate_screen(),
                66 => self.application_keypad(),
                67 | 1005 | 1015 => return 4,
                1000 => self.mouse_protocol_mode == MouseProtocolMode::PressRelease,
                1002 => self.mouse_protocol_mode == MouseProtocolMode::ButtonMotion,
                1003 => self.mouse_protocol_mode == MouseProtocolMode::AnyMotion,
                1004 => self.focus_tracking,
                1006 => self.mouse_protocol_encoding == MouseProtocolEncoding::Sgr,
                1016 => self.mouse_protocol_encoding == MouseProtocolEncoding::SgrPixels,
                1048 => return 1,
                2004 => self.bracketed_paste(),
                2026 => self.sync_started.is_some(),
                _ => return 0,
            }
        } else {
            match mode {
                2 => return 4,
                4 => self.mode(MODE_INSERT),
                12 => return 3,
                20 => self.mode(MODE_NEWLINE),
                _ => return 0,
            }
        };
        if enabled {
            1
        } else {
            2
        }
    }

    fn soft_reset(&mut self) {
        self.end_synchronized_output();
        self.reverse_wrap = false;
        self.focus_tracking = false;
        self.modes &= MODE_ALTERNATE_SCREEN | MODE_NEWLINE;
        self.attrs = crate::attrs::Attrs::default();
        self.charsets = crate::charset::Charsets::default();
        self.set_cursor_style(0);
        self.grid_mut().soft_reset();
    }

    /// Returns whether the terminal should be in hide cursor mode.
    #[must_use]
    pub fn hide_cursor(&self) -> bool {
        self.mode(MODE_HIDE_CURSOR)
    }

    /// Returns whether the terminal should be in bracketed paste mode.
    #[must_use]
    pub fn bracketed_paste(&self) -> bool {
        self.mode(MODE_BRACKETED_PASTE)
    }

    /// While focus tracking is enabled, this serial changes for each request
    /// for the current focus, even when the application repeats DECSET 1004.
    pub fn focus_request(&self) -> Option<u64> {
        self.focus_tracking.then_some(self.focus_requests)
    }

    /// Returns the currently active `MouseProtocolMode`
    #[must_use]
    pub fn mouse_protocol_mode(&self) -> MouseProtocolMode {
        self.mouse_protocol_mode
    }

    /// Returns the currently active `MouseProtocolEncoding`
    #[must_use]
    pub fn mouse_protocol_encoding(&self) -> MouseProtocolEncoding {
        self.mouse_protocol_encoding
    }

    /// Returns the currently active foreground color.
    #[must_use]
    pub fn fgcolor(&self) -> crate::attrs::Color {
        self.attrs.fgcolor
    }

    /// Returns the currently active background color.
    #[must_use]
    pub fn bgcolor(&self) -> crate::attrs::Color {
        self.attrs.bgcolor
    }

    /// Returns whether newly drawn text should be rendered with the bold text
    /// attribute.
    #[must_use]
    pub fn bold(&self) -> bool {
        self.attrs.bold()
    }

    /// Returns whether newly drawn text should be rendered with the italic
    /// text attribute.
    #[must_use]
    pub fn italic(&self) -> bool {
        self.attrs.italic()
    }

    /// Returns whether newly drawn text should be rendered with the
    /// underlined text attribute.
    #[must_use]
    pub fn underline(&self) -> bool {
        self.attrs.underline()
    }

    /// Returns whether newly drawn text should be rendered with the inverse
    /// text attribute.
    #[must_use]
    pub fn inverse(&self) -> bool {
        self.attrs.inverse()
    }

    fn grid(&self) -> &crate::grid::Grid {
        if self.mode(MODE_ALTERNATE_SCREEN) {
            &self.alternate_grid
        } else {
            &self.grid
        }
    }

    fn grid_mut(&mut self) -> &mut crate::grid::Grid {
        if self.mode(MODE_ALTERNATE_SCREEN) {
            &mut self.alternate_grid
        } else {
            &mut self.grid
        }
    }

    fn enter_alternate_grid(&mut self) {
        self.cursor_initialized = true;
        if !self.alternate_screen() {
            self.buffer_epoch = self.buffer_epoch.wrapping_add(1);
            self.grid.set_scrollback(0);
            self.alternate_grid.allocate_rows();
            self.alternate_grid.clear();
            self.alternate_grid.activate_from(&self.grid);
            self.set_mode(MODE_ALTERNATE_SCREEN);
        }
    }

    fn exit_alternate_grid(&mut self) {
        self.cursor_initialized = true;
        if self.alternate_screen() {
            self.buffer_epoch = self.buffer_epoch.wrapping_add(1);
            self.grid.activate_from(&self.alternate_grid);
            self.clear_mode(MODE_ALTERNATE_SCREEN);
        }
    }

    fn save_cursor(&mut self) {
        self.grid_mut().save_cursor();
        self.grid_mut().saved_attrs = self.attrs;
        self.grid_mut().saved_charset = self.charsets.active;
    }

    fn restore_cursor(&mut self) {
        self.grid_mut().restore_cursor();
        self.attrs = self.grid().saved_attrs;
        self.charsets.active = self.grid().saved_charset;
    }

    fn set_mode(&mut self, mode: u8) {
        self.modes |= mode;
    }

    fn clear_mode(&mut self, mode: u8) {
        self.modes &= !mode;
    }

    fn mode(&self, mode: u8) -> bool {
        self.modes & mode != 0
    }

    fn set_mouse_mode(&mut self, mode: MouseProtocolMode) {
        self.mouse_protocol_mode = mode;
    }

    fn set_mouse_encoding(&mut self, encoding: MouseProtocolEncoding) {
        self.mouse_protocol_encoding = encoding;
    }
}

impl Screen {
    fn text(&mut self, c: char) {
        let pos = self.grid().pos();
        let size = self.grid().size();
        let attrs = self.attrs;

        if c.is_control() {
            self.preceding_printable = false;
            return;
        }
        let width = crate::width::columns(c);
        if width == 0 && self.preceding_printable && pos.col > 0 {
            let mut previous = crate::grid::Pos {
                row: pos.row,
                col: pos.col - 1,
            };
            if self
                .grid()
                .drawing_cell(previous)
                .is_some_and(crate::Cell::is_wide_continuation)
            {
                previous.col = previous.col.saturating_sub(1);
            }
            if let Some(cell) = self.grid_mut().drawing_cell_mut(previous) {
                cell.append(c);
            }
            return;
        }
        self.preceding_printable = width != 0;
        // xterm advances one slot for a standalone combining mark. At the right
        // margin that slot is outside the buffer; only the following print wraps.
        if width == 0 && pos.col == size.cols {
            self.grid_mut().col_inc(1);
            return;
        }
        let advance = width.max(1);

        // xterm carries a double-width glyph to the next row when just one column
        // remains. Record the soft wrap so history can remove its padding on resize.
        if width > size.cols {
            return;
        }
        let mut wrap = false;
        if pos.col > size.cols - advance {
            let last_cell = self
                .grid()
                .drawing_cell(crate::grid::Pos {
                    row: pos.row,
                    col: size.cols - 1,
                })
                // pos.row is valid, since it comes directly from
                // self.grid().pos() which we assume to always have a valid
                // row value. size.cols - 1 is also always a valid column.
                .unwrap();
            if last_cell.has_contents()
                || last_cell.is_wide_continuation()
                || (width == 2 && pos.col == size.cols - 1)
            {
                wrap = true;
            }
        }
        if self.mode(MODE_NO_WRAP) && pos.col > size.cols - advance {
            self.grid_mut().col_set(size.cols - 1);
            if width == 2 {
                // A dropped wide glyph leaves the cursor on the margin. xterm
                // clears a trailing continuation cell under that cursor.
                let edge = self.grid().pos();
                if let Some(cell) = self.grid_mut().drawing_cell_mut(edge) {
                    if cell.is_wide_continuation() {
                        cell.clear(attrs)
                    }
                }
                return;
            }
        } else {
            if width == 2 && pos.col == size.cols - 1 {
                if let Some(cell) = self.grid_mut().drawing_cell_mut(pos) {
                    cell.clear(attrs);
                }
            }
            let erase = self.erase_attrs();
            self.grid_mut().col_wrap(advance, wrap, erase);
        }
        if width > 0 && self.mode(MODE_INSERT) {
            self.grid_mut().shift_row(width, true, attrs);
        }
        let pos = self.grid().pos();

        if self
            .grid()
            .drawing_cell(pos)
            // pos.row is valid because we assume self.grid().pos() to
            // always have a valid row value. pos.col is valid because we
            // called col_wrap() immediately before this, which ensures
            // that self.grid().pos().col has a valid value.
            .unwrap()
            .is_wide_continuation()
        {
            let prev_cell = self
                .grid_mut()
                .drawing_cell_mut(crate::grid::Pos {
                    row: pos.row,
                    col: pos.col - 1,
                })
                // pos.row is valid because we assume self.grid().pos() to
                // always have a valid row value. pos.col is valid because
                // we called col_wrap() immediately before this, which
                // ensures that self.grid().pos().col has a valid value.
                // pos.col - 1 is valid because the cell at pos.col is a
                // wide continuation character, so it must have the first
                // half of the wide character before it.
                .unwrap();
            prev_cell.clear(attrs);
        }

        if self
            .grid()
            .drawing_cell(pos)
            // pos.row is valid because we assume self.grid().pos() to
            // always have a valid row value. pos.col is valid because we
            // called col_wrap() immediately before this, which ensures
            // that self.grid().pos().col has a valid value.
            .unwrap()
            .is_wide()
        {
            let next_cell = self
                .grid_mut()
                .drawing_cell_mut(crate::grid::Pos {
                    row: pos.row,
                    col: pos.col + 1,
                })
                // pos.row is valid because we assume self.grid().pos() to
                // always have a valid row value. pos.col is valid because
                // we called col_wrap() immediately before this, which
                // ensures that self.grid().pos().col has a valid value.
                // pos.col + 1 is valid because the cell at pos.col is a
                // wide character, so it must have the second half of the
                // wide character after it.
                .unwrap();
            next_cell.clear(attrs);
        }

        let hyperlink = self.hyperlink.clone();
        let cell = self
            .grid_mut()
            .drawing_cell_mut(pos)
            // pos.row is valid because we assume self.grid().pos() to
            // always have a valid row value. pos.col is valid because we
            // called col_wrap() immediately before this, which ensures
            // that self.grid().pos().col has a valid value.
            .unwrap();
        cell.set(c, attrs);
        cell.link = hyperlink.clone();
        self.grid_mut().col_inc(1);
        if width > 1 {
            let pos = self.grid().pos();
            if self
                .grid()
                .drawing_cell(pos)
                // pos.row is valid because we assume self.grid().pos() to
                // always have a valid row value. pos.col is valid because
                // we called col_wrap() earlier, which ensures that
                // self.grid().pos().col has a valid value. this is true
                // even though we just called col_inc, because this branch
                // only happens if width > 1, and col_wrap takes width
                // into account.
                .unwrap()
                .is_wide()
            {
                let next_next_pos = crate::grid::Pos {
                    row: pos.row,
                    col: pos.col + 1,
                };
                let next_next_cell = self
                    .grid_mut()
                    .drawing_cell_mut(next_next_pos)
                    // pos.row is valid because we assume
                    // self.grid().pos() to always have a valid row value.
                    // pos.col is valid because we called col_wrap()
                    // earlier, which ensures that self.grid().pos().col
                    // has a valid value. this is true even though we just
                    // called col_inc, because this branch only happens if
                    // width > 1, and col_wrap takes width into account.
                    // pos.col + 1 is valid because the cell at pos.col is
                    // wide, and so it must have the second half of the
                    // wide character after it.
                    .unwrap();
                next_next_cell.clear(attrs);
                if next_next_pos.col == size.cols - 1 {
                    self.grid_mut()
                        .drawing_row_mut(pos.row)
                        // we assume self.grid().pos().row is always valid
                        .unwrap()
                        .wrap(false);
                }
            }
            let next_cell = self
                .grid_mut()
                .drawing_cell_mut(pos)
                // pos.row is valid because we assume self.grid().pos() to
                // always have a valid row value. pos.col is valid because
                // we called col_wrap() earlier, which ensures that
                // self.grid().pos().col has a valid value. this is true
                // even though we just called col_inc, because this branch
                // only happens if width > 1, and col_wrap takes width
                // into account.
                .unwrap();
            next_cell.clear(attrs);
            next_cell.set_wide_continuation(true);
            next_cell.link = hyperlink;
            self.grid_mut().col_inc(1);
        }
    }

    // control codes

    fn bel(&mut self) {
        self.audible_bell_count += 1;
    }

    fn bs(&mut self) {
        let reverse = self.reverse_wrap;
        self.grid_mut().backspace(reverse);
    }

    fn repeat_character(&mut self, count: u16) {
        let mut pos = self.grid().pos();
        let Some(col) = pos.col.checked_sub(1) else {
            return;
        };
        pos.col = col;
        if self
            .grid()
            .drawing_cell(pos)
            .is_some_and(crate::Cell::is_wide_continuation)
        {
            pos.col = pos.col.saturating_sub(1);
        }
        if let Some(text) = self.grid().drawing_cell(pos).map(crate::Cell::contents) {
            for _ in 0..count {
                for ch in text.chars() {
                    self.text(ch)
                }
            }
        }
    }

    fn tab(&mut self) {
        self.grid_mut().move_tabs(1, true);
    }

    fn lf(&mut self) {
        if self.mode(MODE_NEWLINE) {
            self.cr();
        }
        self.index();
    }

    fn index(&mut self) {
        let col = self.grid().pos().col;
        self.grid_mut().col_set(col);
        let attrs = self.erase_attrs();
        self.grid_mut().index(attrs);
    }

    fn vt(&mut self) {
        self.lf();
    }

    fn ff(&mut self) {
        self.lf();
    }

    fn cr(&mut self) {
        self.grid_mut().col_set(0);
    }

    // escape codes

    // ESC 7
    fn decsc(&mut self) {
        self.save_cursor();
    }

    // ESC 8
    fn decrc(&mut self) {
        self.restore_cursor();
    }

    // ESC =
    fn deckpam(&mut self) {
        self.set_mode(MODE_APPLICATION_KEYPAD);
    }

    // ESC >
    fn deckpnm(&mut self) {
        self.clear_mode(MODE_APPLICATION_KEYPAD);
    }

    // ESC M
    fn ri(&mut self) {
        let attrs = self.erase_attrs();
        self.grid_mut().reverse_index(attrs);
    }

    // ESC c
    fn ris(&mut self) {
        let title = self.title.clone();
        let icon_name = self.icon_name.clone();
        let audible_bell_count = self.audible_bell_count;
        let visual_bell_count = self.visual_bell_count;
        let errors = self.errors;

        let focus_requests = self.focus_requests;
        let newline_mode = self.modes & MODE_NEWLINE;
        let palette = self.palette.clone();
        let capture_frames = self.capture_frames;
        let default_blink = self.default_cursor_blink;
        let initialized = self.cursor_initialized;
        let epoch = self.buffer_epoch.wrapping_add(1);
        let replies = self.replies.take();
        *self = Self::new(self.grid.size(), self.grid.scrollback_len());
        self.replies = replies;
        self.modes |= newline_mode;
        self.palette = palette;
        self.capture_frames = capture_frames;
        self.default_cursor_blink = default_blink;
        self.focus_requests = focus_requests;
        self.cursor_initialized = initialized;
        self.buffer_epoch = epoch;

        self.title = title;
        self.icon_name = icon_name;
        self.audible_bell_count = audible_bell_count;
        self.visual_bell_count = visual_bell_count;
        self.errors = errors;
    }

    // ESC g
    fn vb(&mut self) {
        self.visual_bell_count += 1;
    }

    // csi codes

    // CSI @
    fn ich(&mut self, count: u16) {
        let attrs = self.erase_attrs();
        self.grid_mut().shift_row(count, true, attrs);
    }

    // CSI A
    fn cuu(&mut self, offset: u16) {
        self.grid_mut().move_cursor(0, -i32::from(offset));
    }

    // CSI B
    fn cud(&mut self, offset: u16) {
        self.grid_mut().move_cursor(0, i32::from(offset));
    }

    // CSI C
    fn cuf(&mut self, offset: u16) {
        self.grid_mut().move_cursor(i32::from(offset), 0);
    }

    // CSI D
    fn cub(&mut self, offset: u16) {
        self.grid_mut().move_cursor(-i32::from(offset), 0);
    }

    // CSI G
    fn cha(&mut self, col: u16) {
        self.grid_mut().col_set(col - 1);
    }

    // CSI H
    fn cup(&mut self, (row, col): (u16, u16)) {
        self.grid_mut().set_pos(crate::grid::Pos {
            row: row - 1,
            col: col - 1,
        });
    }

    // CSI J
    fn erase_attrs(&self) -> crate::attrs::Attrs {
        crate::attrs::Attrs {
            bgcolor: self.attrs.bgcolor,
            ..Default::default()
        }
    }

    fn ed(&mut self, mode: u16) {
        let attrs = self.erase_attrs();
        match mode {
            0 => self.grid_mut().erase_all_forward(attrs),
            1 => self.grid_mut().erase_all_backward(attrs),
            2 => self.grid_mut().erase_all(attrs),
            3 => self.grid_mut().clear_scrollback(),
            n => {
                log::debug!("unhandled ED mode: {n}");
            }
        }
    }

    // CSI ? J
    fn decsed(&mut self, mode: u16) {
        let attrs = self.erase_attrs();
        self.grid_mut().selective_erase(mode, true, attrs);
    }

    // CSI K
    fn el(&mut self, mode: u16) {
        let attrs = self.erase_attrs();
        match mode {
            0 => self.grid_mut().erase_row_forward(attrs),
            1 => self.grid_mut().erase_row_backward(attrs),
            2 => self.grid_mut().erase_row(attrs),
            n => {
                log::debug!("unhandled EL mode: {n}");
            }
        }
    }

    // CSI ? K
    fn decsel(&mut self, mode: u16) {
        let attrs = self.erase_attrs();
        self.grid_mut().selective_erase(mode, false, attrs);
    }

    // CSI L
    fn il(&mut self, count: u16) {
        let attrs = self.erase_attrs();
        self.grid_mut().edit_lines(count, true, attrs);
    }

    // CSI M
    fn dl(&mut self, count: u16) {
        let attrs = self.erase_attrs();
        self.grid_mut().edit_lines(count, false, attrs);
    }

    // CSI P
    fn dch(&mut self, count: u16) {
        let attrs = self.erase_attrs();
        self.grid_mut().shift_row(count, false, attrs);
    }

    // CSI S
    fn su(&mut self, count: u16) {
        let attrs = self.erase_attrs();
        self.grid_mut().scroll_up(count, attrs, false);
    }

    // CSI T
    fn sd(&mut self, count: u16) {
        self.grid_mut().scroll_down(count, Default::default());
    }

    // CSI X
    fn ech(&mut self, count: u16) {
        let attrs = self.erase_attrs();
        self.grid_mut().erase_cells(count, attrs);
    }

    // CSI d
    fn vpa(&mut self, row: u16) {
        self.grid_mut().row_set(row - 1);
    }

    // CSI h
    fn ansi_modes(&mut self, params: &vte::Params, enable: bool) {
        for param in params {
            let mode = match param {
                &[4] => MODE_INSERT,
                &[20] => MODE_NEWLINE,
                _ => continue,
            };
            if enable {
                self.set_mode(mode);
            } else {
                self.clear_mode(mode);
            }
        }
    }

    // CSI ? h
    fn decset(&mut self, params: &vte::Params) {
        for param in params {
            match param {
                &[12] => self.default_cursor_blink = true,
                &[1] => self.set_mode(MODE_APPLICATION_CURSOR),
                &[2] => {
                    for level in 0..4 {
                        self.charsets.designate(level, b'B');
                    }
                }
                &[6] => self.grid_mut().set_origin_mode(true),
                &[7] => self.clear_mode(MODE_NO_WRAP),
                &[9] => self.set_mouse_mode(MouseProtocolMode::Press),
                &[25] => self.clear_mode(MODE_HIDE_CURSOR),
                &[1048] => self.decsc(),
                &[47] | &[1047] => self.enter_alternate_grid(),
                &[1000] => {
                    self.set_mouse_mode(MouseProtocolMode::PressRelease);
                }
                &[1002] => {
                    self.set_mouse_mode(MouseProtocolMode::ButtonMotion);
                }
                &[1003] => self.set_mouse_mode(MouseProtocolMode::AnyMotion),
                &[1004] => {
                    self.focus_tracking = true;
                    self.focus_requests = self.focus_requests.wrapping_add(1);
                }
                &[1016] => self.set_mouse_encoding(MouseProtocolEncoding::SgrPixels),
                &[1006] => {
                    self.set_mouse_encoding(MouseProtocolEncoding::Sgr);
                }
                &[1049] => {
                    self.decsc();
                    self.enter_alternate_grid();
                }
                &[45] => self.reverse_wrap = true,
                &[66] => self.deckpam(),
                &[2004] => self.set_mode(MODE_BRACKETED_PASTE),
                &[2026] => self.start_synchronized_output(),
                ns => {
                    if log::log_enabled!(log::Level::Debug) {
                        let n = if ns.len() == 1 {
                            format!(
                                "{}",
                                // we just checked that ns.len() == 1, so 0
                                // must be valid
                                ns[0]
                            )
                        } else {
                            format!("{ns:?}")
                        };
                        log::debug!("unhandled DECSET mode: {n}");
                    }
                }
            }
        }
    }

    // CSI ? l
    fn decrst(&mut self, params: &vte::Params) {
        for param in params {
            match param {
                &[12] => self.default_cursor_blink = false,
                &[1] => self.clear_mode(MODE_APPLICATION_CURSOR),
                &[6] => self.grid_mut().set_origin_mode(false),
                &[7] => self.set_mode(MODE_NO_WRAP),
                &[9] | &[1000] | &[1002] | &[1003] => self.set_mouse_mode(MouseProtocolMode::None),
                &[25] => self.set_mode(MODE_HIDE_CURSOR),
                &[1048] => self.decrc(),
                &[47] | &[1047] => {
                    self.exit_alternate_grid();
                }
                &[1004] => self.focus_tracking = false,
                &[1006] | &[1016] => self.set_mouse_encoding(MouseProtocolEncoding::Default),
                &[1049] => {
                    self.exit_alternate_grid();
                    self.decrc();
                }
                &[45] => self.reverse_wrap = false,
                &[66] => self.deckpnm(),
                &[2004] => self.clear_mode(MODE_BRACKETED_PASTE),
                &[2026] => self.end_synchronized_output(),
                ns => {
                    if log::log_enabled!(log::Level::Debug) {
                        let n = if ns.len() == 1 {
                            format!(
                                "{}",
                                // we just checked that ns.len() == 1, so 0
                                // must be valid
                                ns[0]
                            )
                        } else {
                            format!("{ns:?}")
                        };
                        log::debug!("unhandled DECRST mode: {n}");
                    }
                }
            }
        }
    }

    // CSI m
    fn sgr(&mut self, params: &vte::Params) {
        // XXX really i want to just be able to pass in a default Params
        // instance with a 0 in it, but vte doesn't allow creating new Params
        // instances
        if params.is_empty() {
            self.attrs = crate::attrs::Attrs::default();
            return;
        }

        let mut iter = params.iter();

        macro_rules! next_param {
            () => {
                match iter.next() {
                    Some(n) => n,
                    _ => return,
                }
            };
        }

        macro_rules! to_u8 {
            ($n:expr) => {
                if let Some(n) = u16_to_u8($n) {
                    n
                } else {
                    return;
                }
            };
        }

        macro_rules! next_param_u8 {
            () => {
                if let &[n] = next_param!() {
                    to_u8!(n)
                } else {
                    return;
                }
            };
        }

        loop {
            match next_param!() {
                &[0] => self.attrs = crate::attrs::Attrs::default(),
                &[1] => self.attrs.set_bold(true),
                &[3] => self.attrs.set_italic(true),
                &[4] => self.attrs.underline_style = crate::Underline::Single,
                &[4, style, ..] => self.attrs.underline_style = crate::Underline::from_sgr(style),
                &[21] => self.attrs.underline_style = crate::Underline::Double,
                &[53] => self.attrs.set_overline(true),
                &[55] => self.attrs.set_overline(false),
                &[7] => self.attrs.set_inverse(true),
                &[2] => self.attrs.set_dim(true),
                &[8] => self.attrs.set_invisible(true),
                &[9] => self.attrs.set_strike(true),
                &[22] => {
                    self.attrs.set_bold(false);
                    self.attrs.set_dim(false);
                }
                &[28] => self.attrs.set_invisible(false),
                &[29] => self.attrs.set_strike(false),
                &[23] => self.attrs.set_italic(false),
                &[24] => self.attrs.underline_style = crate::Underline::None,
                &[27] => self.attrs.set_inverse(false),
                &[n] if (30..=37).contains(&n) => {
                    self.attrs.fgcolor = crate::attrs::Color::Idx(to_u8!(n) - 30);
                }
                &[target @ (38 | 48 | 58), ref components @ ..] => {
                    use crate::Color;
                    let color = match components {
                        [5, i] => Color::Idx(to_u8!(*i)),
                        [2, r, g, b] | [2, _, r, g, b] => {
                            Color::Rgb(to_u8!(*r), to_u8!(*g), to_u8!(*b))
                        }
                        [] => match next_param!() {
                            [5] => Color::Idx(next_param_u8!()),
                            [2] => Color::Rgb(next_param_u8!(), next_param_u8!(), next_param_u8!()),
                            _ => return,
                        },
                        _ => continue,
                    };
                    match target {
                        38 => self.attrs.fgcolor = color,
                        48 => self.attrs.bgcolor = color,
                        _ => self.attrs.underline_color = color,
                    }
                }
                &[59] => self.attrs.underline_color = crate::Color::Default,
                &[39] => {
                    self.attrs.fgcolor = crate::attrs::Color::Default;
                }
                &[n] if (40..=47).contains(&n) => {
                    self.attrs.bgcolor = crate::attrs::Color::Idx(to_u8!(n) - 40);
                }
                &[49] => {
                    self.attrs.bgcolor = crate::attrs::Color::Default;
                }
                &[n] if (90..=97).contains(&n) => {
                    self.attrs.fgcolor = crate::attrs::Color::Idx(to_u8!(n) - 82);
                }
                &[n] if (100..=107).contains(&n) => {
                    self.attrs.bgcolor = crate::attrs::Color::Idx(to_u8!(n) - 92);
                }
                ns => {
                    if log::log_enabled!(log::Level::Debug) {
                        let n = if ns.len() == 1 {
                            format!(
                                "{}",
                                // we just checked that ns.len() == 1, so 0
                                // must be valid
                                ns[0]
                            )
                        } else {
                            format!("{ns:?}")
                        };
                        log::debug!("unhandled SGR mode: {n}");
                    }
                }
            }
        }
    }

    // CSI r
    fn decstbm(&mut self, (top, bottom): (u16, u16)) {
        self.grid_mut().set_scroll_region(top - 1, bottom - 1);
    }

    // osc codes

    fn osc0(&mut self, s: &[u8]) {
        self.osc1(s);
        self.osc2(s);
    }

    fn osc1(&mut self, s: &[u8]) {
        if let Ok(s) = std::str::from_utf8(s) {
            self.icon_name = s.to_string();
        }
    }

    fn osc2(&mut self, s: &[u8]) {
        if let Ok(s) = std::str::from_utf8(s) {
            self.title = s.to_string();
        }
    }
}

impl vte::Perform for Screen {
    fn print(&mut self, c: char) {
        if c == '\u{fffd}' || ('\u{80}'..'\u{a0}').contains(&c) {
            self.errors = self.errors.saturating_add(1);
        }
        self.text(self.charsets.decode(c));
    }

    fn execute(&mut self, b: u8) {
        self.preceding_printable = false;
        match b {
            7 => self.bel(),
            8 => self.bs(),
            9 => self.tab(),
            10 => self.lf(),
            11 => self.vt(),
            12 => self.ff(),
            13 => self.cr(),
            14 => self.charsets.select(1),
            15 => self.charsets.select(0),
            _ => {
                self.errors = self.errors.saturating_add(1);
                log::debug!("unhandled control character: {b}");
            }
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, b: u8) {
        self.preceding_printable = false;
        if ignore {
            return;
        }
        match (intermediates, b) {
            ([b'#'], b'8') => {
                let attrs = self.attrs;
                self.grid_mut().alignment_pattern(attrs)
            }
            ([], b'7') => self.decsc(),
            ([], b'8') => self.decrc(),
            ([], b'=') => self.deckpam(),
            ([], b'>') => self.deckpnm(),
            ([], b'D') => self.index(),
            ([], b'E') => {
                self.cr();
                self.index();
            }
            ([], b'H') => self.grid_mut().set_tab_stop(),
            ([], b'M') => self.ri(),
            ([], b'c') => self.ris(),
            ([], b'g') => self.vb(),
            ([], b'~') => self.charsets.select(1),
            ([], b'n' | b'}') => self.charsets.select(2),
            ([], b'o' | b'|') => self.charsets.select(3),
            ([b'%'], b'G' | b'@') => {
                self.charsets.select(0);
                self.charsets.designate(0, b'B');
            }
            ([b'('], name) => self.charsets.designate(0, name),
            ([b')' | b'-'], name) => self.charsets.designate(1, name),
            ([b'*' | b'.'], name) => self.charsets.designate(2, name),
            ([b'+'], name) => self.charsets.designate(3, name),
            _ => log::debug!("unhandled escape code: ESC {intermediates:?} {b}"),
        }
    }

    fn csi_dispatch(&mut self, params: &vte::Params, intermediates: &[u8], ignore: bool, c: char) {
        let preceded_by_text = std::mem::take(&mut self.preceding_printable);
        if ignore {
            return;
        }
        match (intermediates, c) {
            ([], 'b') => {
                if preceded_by_text {
                    self.repeat_character(canonicalize_params_1(params, 1));
                    self.preceding_printable = false;
                }
                return;
            }
            ([b' '], '@' | 'A') | ([b'\''], '}' | '~') => {
                let col = if intermediates == b" " {
                    0
                } else {
                    self.grid().pos().col
                };
                let attrs = self.erase_attrs();
                self.grid_mut().shift_columns(
                    col,
                    canonicalize_params_1(params, 1),
                    matches!(c, 'A' | '}'),
                    attrs,
                );
                return;
            }
            ([b'"'], 'q') => {
                match canonicalize_params_1(params, 0) {
                    1 => self.attrs.set_protected(true),
                    0 | 2 => self.attrs.set_protected(false),
                    _ => {}
                }
                return;
            }
            ([b' '], 'q') => {
                self.set_cursor_style(canonicalize_params_1(params, 0));
                return;
            }
            ([b'$'] | [b'?', b'$'], 'p') => {
                if self.replies.is_some() {
                    use std::io::Write as _;
                    let mode = canonicalize_params_1(params, 0);
                    let private = intermediates[0] == b'?';
                    let prefix = if private { "?" } else { "" };
                    let value = self.mode_report(mode, private);
                    write!(
                        self.replies.as_mut().unwrap(),
                        "\x1b[{prefix}{mode};{value}$y"
                    )
                    .unwrap();
                }
                return;
            }
            ([b'!'], 'p') => {
                self.soft_reset();
                return;
            }
            _ => {}
        }
        if self.replies.is_some() && matches!(c, 'n' | 'c') {
            let param = params
                .iter()
                .next()
                .and_then(|p| p.first())
                .copied()
                .unwrap_or(0);
            let (row, col) = self.cursor_position();
            let replies = self.replies.as_mut().unwrap();
            match (intermediates, c, param) {
                ([], 'n', 5) => replies.extend_from_slice(b"\x1b[0n"),
                ([] | [b'?'], 'n', 6) => {
                    use std::io::Write as _;
                    let prefix = if intermediates.is_empty() { "" } else { "?" };
                    write!(
                        replies,
                        "\x1b[{prefix}{};{}R",
                        u32::from(row) + 1,
                        u32::from(col) + 1
                    )
                    .unwrap();
                }
                ([], 'c', 0) => replies.extend_from_slice(b"\x1b[?1;2c"),
                ([b'>'], 'c', 0) => replies.extend_from_slice(b"\x1b[>0;276;0c"),
                _ => {}
            }
            return;
        }
        match intermediates.first() {
            None => match c {
                '@' => self.ich(canonicalize_params_1(params, 1)),
                'A' => self.cuu(canonicalize_params_1(params, 1)),
                'B' | 'e' => self.cud(canonicalize_params_1(params, 1)),
                'C' | 'a' => self.cuf(canonicalize_params_1(params, 1)),
                'D' => self.cub(canonicalize_params_1(params, 1)),
                'E' => {
                    self.cud(canonicalize_params_1(params, 1));
                    self.cr();
                }
                'F' => {
                    self.cuu(canonicalize_params_1(params, 1));
                    self.cr();
                }
                's' => self.save_cursor(),
                'u' => self.restore_cursor(),
                'G' | '`' => self.cha(canonicalize_params_1(params, 1)),
                'H' | 'f' => self.cup(canonicalize_params_2(params, 1, 1)),
                'I' => self
                    .grid_mut()
                    .move_tabs(canonicalize_params_1(params, 1), true),
                'Z' => self
                    .grid_mut()
                    .move_tabs(canonicalize_params_1(params, 1), false),
                'g' => self
                    .grid_mut()
                    .clear_tab_stops(canonicalize_params_1(params, 0)),
                'J' => self.ed(canonicalize_params_1(params, 0)),
                'K' => self.el(canonicalize_params_1(params, 0)),
                'L' => self.il(canonicalize_params_1(params, 1)),
                'M' => self.dl(canonicalize_params_1(params, 1)),
                'P' => self.dch(canonicalize_params_1(params, 1)),
                'S' => self.su(canonicalize_params_1(params, 1)),
                'T' => self.sd(canonicalize_params_1(params, 1)),
                'X' => self.ech(canonicalize_params_1(params, 1)),
                'd' => self.vpa(canonicalize_params_1(params, 1)),
                'h' => self.ansi_modes(params, true),
                'l' => self.ansi_modes(params, false),
                'm' => self.sgr(params),
                'r' => self.decstbm(canonicalize_params_decstbm(params, self.grid().size())),
                _ => {
                    if log::log_enabled!(log::Level::Debug) {
                        log::debug!("unhandled csi sequence: CSI {} {}", param_str(params), c);
                    }
                }
            },
            Some(b'?') => match c {
                'J' => self.decsed(canonicalize_params_1(params, 0)),
                'K' => self.decsel(canonicalize_params_1(params, 0)),
                'h' => self.decset(params),
                'l' => self.decrst(params),
                _ => {
                    if log::log_enabled!(log::Level::Debug) {
                        log::debug!("unhandled csi sequence: CSI ? {} {}", param_str(params), c);
                    }
                }
            },
            Some(i) => {
                if log::log_enabled!(log::Level::Debug) {
                    log::debug!(
                        "unhandled csi sequence: CSI {} {} {}",
                        i,
                        param_str(params),
                        c
                    );
                }
            }
        }
    }

    fn osc_single_payload(&self, command: &[u8]) -> bool {
        matches!(command, b"4" | b"10" | b"11" | b"12" | b"104")
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bel_terminated: bool) {
        self.preceding_printable = false;
        if let Some((tag, fields)) = params.split_first() {
            if self.palette.osc(
                tag,
                fields.first().copied().unwrap_or(b""),
                self.replies.as_mut(),
            ) {
                return;
            }
        }
        if let [b"8", attributes, uri] = params {
            if !uri.is_empty() || attributes.iter().all(u8::is_ascii_whitespace) {
                self.hyperlink = crate::Hyperlink::parse(attributes, uri);
            }
            return;
        }
        match (params.get(0), params.get(1)) {
            (Some(&b"0"), Some(s)) => self.osc0(s),
            (Some(&b"1"), Some(s)) => self.osc1(s),
            (Some(&b"2"), Some(s)) => self.osc2(s),
            _ => {
                if log::log_enabled!(log::Level::Debug) {
                    log::debug!("unhandled osc sequence: OSC {}", osc_param_str(params),);
                }
            }
        }
    }

    fn hook(&mut self, _: &vte::Params, intermediates: &[u8], ignore: bool, action: char) {
        self.preceding_printable = false;
        self.status_query =
            (!ignore && intermediates == b"$" && action == 'q' && self.replies.is_some())
                .then(|| Vec::with_capacity(3));
    }

    fn put(&mut self, byte: u8) {
        // All supported identifiers are one or two bytes. A third marks an unknown request.
        if let Some(query) = &mut self.status_query {
            if query.len() < 3 {
                query.push(byte);
            }
        }
    }

    fn unhook(&mut self) {
        let Some(query) = self.status_query.take() else {
            return;
        };
        let (top, bottom) = self.grid().margins();
        let Some(reply) = self.replies.as_mut() else {
            return;
        };
        use std::io::Write as _;
        match query.as_slice() {
            b" q" => reply.extend_from_slice(if self.default_cursor_blink {
                b"\x1bP1$r5 q"
            } else {
                b"\x1bP1$r6 q"
            }),
            b"\"q" => reply.extend_from_slice(if self.attrs.protected() {
                b"\x1bP1$r1\"q"
            } else {
                b"\x1bP1$r0\"q"
            }),
            b"\"p" => reply.extend_from_slice(b"\x1bP1$r61;1\"p"),
            b"m" => reply.extend_from_slice(b"\x1bP1$r0m"),
            b"r" => write!(reply, "\x1bP1$r{};{}r", top + 1, bottom + 1).unwrap(),
            _ => reply.extend_from_slice(b"\x1bP0$r"),
        }
        reply.extend_from_slice(b"\x1b\\");
    }
}

fn canonicalize_params_1(params: &vte::Params, default: u16) -> u16 {
    let first = params.iter().next().map_or(0, |x| *x.first().unwrap_or(&0));
    if first == 0 {
        default
    } else {
        first
    }
}

fn canonicalize_params_2(params: &vte::Params, default1: u16, default2: u16) -> (u16, u16) {
    let mut iter = params.iter();
    let first = iter.next().map_or(0, |x| *x.first().unwrap_or(&0));
    let first = if first == 0 { default1 } else { first };

    let second = iter.next().map_or(0, |x| *x.first().unwrap_or(&0));
    let second = if second == 0 { default2 } else { second };

    (first, second)
}

fn canonicalize_params_decstbm(params: &vte::Params, size: crate::grid::Size) -> (u16, u16) {
    let mut iter = params.iter();
    let top = iter.next().map_or(0, |x| *x.first().unwrap_or(&0));
    let top = if top == 0 { 1 } else { top };

    let bottom = iter.next().map_or(0, |x| *x.first().unwrap_or(&0));
    let bottom = if bottom == 0 { size.rows } else { bottom };

    (top, bottom)
}

fn u16_to_u8(i: u16) -> Option<u8> {
    if i > u16::from(u8::max_value()) {
        None
    } else {
        // safe because we just ensured that the value fits in a u8
        Some(i.try_into().unwrap())
    }
}

fn param_str(params: &vte::Params) -> String {
    let strs: Vec<_> = params
        .iter()
        .map(|subparams| {
            let subparam_strs: Vec<_> = subparams
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            subparam_strs.join(" : ")
        })
        .collect();
    strs.join(" ; ")
}

fn osc_param_str(params: &[&[u8]]) -> String {
    let strs: Vec<_> = params
        .iter()
        .map(|b| format!("\"{}\"", std::string::String::from_utf8_lossy(b)))
        .collect();
    strs.join(" ; ")
}
