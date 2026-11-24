use crate::term::BufWrite as _;

/// Represents a foreground or background color for cells.
#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum Color {
    /// The default terminal color.
    Default,

    /// An indexed terminal color.
    Idx(u8),

    /// An RGB terminal color. The parameters are (red, green, blue).
    Rgb(u8, u8, u8),
}

impl Default for Color {
    fn default() -> Self {
        Self::Default
    }
}

const TEXT_MODE_BOLD: u8 = 0b0000_0001;
const TEXT_MODE_ITALIC: u8 = 0b0000_0010;
const TEXT_MODE_OVERLINE: u8 = 0b0000_0100;

/// SGR 4 underline styles, shared by the parser, formatter and desktop renderer.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Wavy,
    Dotted,
    Dashed,
}

impl Underline {
    pub(crate) fn from_sgr(style: u16) -> Self {
        match style {
            0 => Self::None,
            2 => Self::Double,
            3 => Self::Wavy,
            4 => Self::Dotted,
            5 => Self::Dashed,
            _ => Self::Single,
        }
    }
}
const TEXT_MODE_INVERSE: u8 = 0b0000_1000;

const TEXT_MODE_DIM: u8 = 1 << 4;
const TEXT_MODE_INVISIBLE: u8 = 1 << 5;
const TEXT_MODE_STRIKE: u8 = 1 << 6;

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Attrs {
    pub fgcolor: Color,
    pub bgcolor: Color,
    pub mode: u8,
    pub underline_style: Underline,
    pub underline_color: Color,
}

impl Attrs {
    pub fn protected(&self) -> bool {
        self.mode & 0x80 != 0
    }
    pub fn set_protected(&mut self, protect: bool) {
        self.set_flag(0x80, protect);
    }

    pub fn bold(&self) -> bool {
        self.mode & TEXT_MODE_BOLD != 0
    }

    pub fn set_bold(&mut self, bold: bool) {
        if bold {
            self.mode |= TEXT_MODE_BOLD;
        } else {
            self.mode &= !TEXT_MODE_BOLD;
        }
    }

    pub fn italic(&self) -> bool {
        self.mode & TEXT_MODE_ITALIC != 0
    }

    pub fn set_italic(&mut self, italic: bool) {
        if italic {
            self.mode |= TEXT_MODE_ITALIC;
        } else {
            self.mode &= !TEXT_MODE_ITALIC;
        }
    }

    pub fn underline(&self) -> bool {
        self.underline_style != Underline::None
    }

    pub fn overline(&self) -> bool {
        self.mode & TEXT_MODE_OVERLINE != 0
    }
    pub fn set_overline(&mut self, on: bool) {
        self.set_flag(TEXT_MODE_OVERLINE, on);
    }

    pub fn inverse(&self) -> bool {
        self.mode & TEXT_MODE_INVERSE != 0
    }

    pub fn set_inverse(&mut self, inverse: bool) {
        if inverse {
            self.mode |= TEXT_MODE_INVERSE;
        } else {
            self.mode &= !TEXT_MODE_INVERSE;
        }
    }

    pub fn dim(&self) -> bool {
        self.mode & TEXT_MODE_DIM != 0
    }
    pub fn invisible(&self) -> bool {
        self.mode & TEXT_MODE_INVISIBLE != 0
    }
    pub fn strike(&self) -> bool {
        self.mode & TEXT_MODE_STRIKE != 0
    }

    pub fn set_dim(&mut self, value: bool) {
        self.set_flag(TEXT_MODE_DIM, value);
    }
    pub fn set_invisible(&mut self, value: bool) {
        self.set_flag(TEXT_MODE_INVISIBLE, value);
    }
    pub fn set_strike(&mut self, value: bool) {
        self.set_flag(TEXT_MODE_STRIKE, value);
    }

    fn set_flag(&mut self, flag: u8, enabled: bool) {
        if enabled {
            self.mode |= flag;
        } else {
            self.mode &= !flag;
        }
    }

    pub fn write_escape_code_diff(&self, contents: &mut Vec<u8>, other: &Self) {
        if self != other && self == &Self::default() {
            crate::term::ClearAttrs::default().write_buf(contents);
            return;
        }

        if self.protected() != other.protected() {
            contents.extend_from_slice(if self.protected() {
                b"\x1b[1\"q"
            } else {
                b"\x1b[0\"q"
            });
        }

        let attrs = crate::term::Attrs::default();

        let attrs = if self.fgcolor == other.fgcolor {
            attrs
        } else {
            attrs.fgcolor(self.fgcolor)
        };
        let attrs = if self.bgcolor == other.bgcolor {
            attrs
        } else {
            attrs.bgcolor(self.bgcolor)
        };
        // SGR 22 disables both bold and dim. Reapply any surviving attribute.
        let reset_intensity = (other.bold() && !self.bold()) || (other.dim() && !self.dim());
        let attrs = attrs.intensity(
            reset_intensity,
            self.bold() && (reset_intensity || !other.bold()),
            self.dim() && (reset_intensity || !other.dim()),
        );
        let attrs = attrs.decorations(
            (self.invisible() != other.invisible()).then_some(self.invisible()),
            (self.strike() != other.strike()).then_some(self.strike()),
        );
        let attrs = if self.italic() == other.italic() {
            attrs
        } else {
            attrs.italic(self.italic())
        };
        let attrs = attrs.lines(
            (self.underline_style != other.underline_style).then_some(self.underline_style),
            (self.underline_color != other.underline_color).then_some(self.underline_color),
            (self.overline() != other.overline()).then_some(self.overline()),
        );
        let attrs = if self.inverse() == other.inverse() {
            attrs
        } else {
            attrs.inverse(self.inverse())
        };

        attrs.write_buf(contents);
    }
}
