#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum Glyph {
    #[default]
    Empty,
    Scalar(char),
    Combined(String),
}

/// Represents a single terminal cell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cell {
    glyph: Glyph,
    width: u16,
    continuation: bool,
    pub(crate) link: Option<std::sync::Arc<crate::Hyperlink>>,
    attrs: crate::attrs::Attrs,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            glyph: Glyph::Empty,
            width: 1,
            continuation: false,
            link: None,
            attrs: Default::default(),
        }
    }
}

impl Cell {
    pub(crate) fn set(&mut self, c: char, a: crate::attrs::Attrs) {
        self.link = None;
        self.glyph = Glyph::Scalar(c);
        self.width = crate::width::columns(c);
        self.continuation = false;
        self.attrs = a;
    }

    pub(crate) fn append(&mut self, c: char) {
        match &mut self.glyph {
            Glyph::Empty => self.glyph = Glyph::Scalar(c),
            Glyph::Scalar(base) => self.glyph = Glyph::Combined([*base, c].iter().collect()),
            Glyph::Combined(text) => text.push(c),
        }
    }

    pub(crate) fn clear(&mut self, attrs: crate::attrs::Attrs) {
        *self = Self {
            attrs,
            ..Self::default()
        };
    }

    pub(crate) fn append_contents_to(&self, output: &mut String) {
        match &self.glyph {
            Glyph::Empty => {}
            Glyph::Scalar(c) => output.push(*c),
            Glyph::Combined(text) => output.push_str(text),
        }
    }

    /// Returns all code points stored in the cell, including combining marks.
    #[must_use]
    pub fn contents(&self) -> String {
        let mut result = String::new();
        self.append_contents_to(&mut result);
        result
    }

    /// Decoration used by the terminal renderer; OSC 8 marks are dashed at rest.
    pub fn underline_style(&self) -> crate::Underline {
        if self.link.is_some() {
            crate::Underline::Dashed
        } else {
            self.attrs.underline_style
        }
    }
    pub fn underline_color(&self) -> crate::Color {
        self.attrs.underline_color
    }
    pub fn overline(&self) -> bool {
        self.attrs.overline()
    }

    /// Returns whether the cell contains any text data.
    #[must_use]
    pub fn has_contents(&self) -> bool {
        !matches!(self.glyph, Glyph::Empty)
    }

    /// Hyperlink attached to this cell, including a double-width character's trailing cell.
    pub fn hyperlink(&self) -> Option<&std::sync::Arc<crate::Hyperlink>> {
        self.link.as_ref()
    }

    pub(crate) fn width(&self) -> u16 {
        self.width
    }

    /// True for the leading cell of a double-width character.
    #[must_use]
    pub fn is_wide(&self) -> bool {
        self.width == 2
    }

    /// True for the trailing cell of a double-width character.
    #[must_use]
    pub fn is_wide_continuation(&self) -> bool {
        self.continuation
    }

    pub(crate) fn set_wide_continuation(&mut self, wide: bool) {
        self.continuation = wide;
        self.width = if wide { 0 } else { 1 };
    }

    pub(crate) fn attrs(&self) -> &crate::attrs::Attrs {
        &self.attrs
    }

    /// Returns the foreground color of the cell.
    #[must_use]
    pub fn fgcolor(&self) -> crate::attrs::Color {
        self.attrs.fgcolor
    }

    /// Returns the background color of the cell.
    #[must_use]
    pub fn bgcolor(&self) -> crate::attrs::Color {
        self.attrs.bgcolor
    }

    /// Returns whether the cell should be rendered with the bold text
    /// attribute.
    #[must_use]
    pub fn bold(&self) -> bool {
        self.attrs.bold()
    }

    /// Returns whether to draw the foreground at reduced intensity.
    #[must_use]
    pub fn dim(&self) -> bool {
        self.attrs.dim()
    }

    /// Returns whether to suppress the glyph while retaining its grid contents.
    #[must_use]
    pub fn invisible(&self) -> bool {
        self.attrs.invisible()
    }

    /// Returns whether the cell has a strikethrough.
    #[must_use]
    pub fn strike(&self) -> bool {
        self.attrs.strike()
    }

    /// Returns whether the cell should be rendered with the italic text
    /// attribute.
    #[must_use]
    pub fn italic(&self) -> bool {
        self.attrs.italic()
    }

    /// Returns whether the cell should be rendered with the underlined text
    /// attribute.
    #[must_use]
    pub fn underline(&self) -> bool {
        self.attrs.underline()
    }

    /// Returns whether the cell should be rendered with the inverse text
    /// attribute.
    #[must_use]
    pub fn inverse(&self) -> bool {
        self.attrs.inverse()
    }
}
