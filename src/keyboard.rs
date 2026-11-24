//! Encode terminal keys using the same VT sequences as the bundled xterm.js.
//! Ordinary text stays with the platform's text input and IME.
use gpui::Modifiers;

pub(crate) fn sequence(key: &str, m: Modifiers, application_cursor: bool) -> Option<String> {
    let modifier = 1
        + u8::from(m.shift)
        + 2 * u8::from(m.alt)
        + 4 * u8::from(m.control)
        + 8 * u8::from(m.platform);
    let tilde = |number| {
        if modifier == 1 {
            format!("\x1b[{number}~")
        } else {
            format!("\x1b[{number};{modifier}~")
        }
    };
    let special = match key {
        "up" | "down" | "right" | "left" if m.platform => return None,
        "up" => 'A',
        "down" => 'B',
        "right" => 'C',
        "left" => 'D',
        "home" => 'H',
        "end" => 'F',
        "f1" => 'P',
        "f2" => 'Q',
        "f3" => 'R',
        "f4" => 'S',
        _ => {
            let data = match key {
                "backspace" => {
                    if m.control {
                        "\x08"
                    } else {
                        "\x7f"
                    }
                }
                "enter" => "\r",
                "escape" => "\x1b",
                "tab" => return Some(if m.shift { "\x1b[Z" } else { "\t" }.into()),
                "insert" if !m.shift && !m.control => return Some("\x1b[2~".into()),
                "delete" => return Some(tilde(3)),
                "pageup" | "pagedown" if !m.shift => {
                    let n = if key == "pageup" { 5 } else { 6 };
                    return Some(if m.control {
                        tilde(n)
                    } else {
                        format!("\x1b[{n}~")
                    });
                }
                "f5" => return Some(tilde(15)),
                "f6" => return Some(tilde(17)),
                "f7" => return Some(tilde(18)),
                "f8" => return Some(tilde(19)),
                "f9" => return Some(tilde(20)),
                "f10" => return Some(tilde(21)),
                "f11" => return Some(tilde(23)),
                "f12" => return Some(tilde(24)),
                _ => return modified_character(key, m),
            };
            return Some(if m.alt {
                format!("\x1b{data}")
            } else {
                data.into()
            });
        }
    };
    Some(if modifier > 1 {
        format!("\x1b[1;{modifier}{special}")
    } else if application_cursor || matches!(key, "f1" | "f2" | "f3" | "f4") {
        format!("\x1bO{special}")
    } else {
        format!("\x1b[{special}")
    })
}

fn modified_character(key: &str, m: Modifiers) -> Option<String> {
    if m.platform {
        return None;
    }
    if m.control && !m.alt {
        let byte = match key {
            "space" if !m.shift => 0,
            "@" => 0,
            "_" => 31,
            "2" if m.shift => 0,
            "-" if m.shift => 31,
            _ if m.shift => return None,
            "3" | "[" => 27,
            "4" | "\\" => 28,
            "5" | "]" => 29,
            "6" => 30,
            "7" => 31,
            "8" => 127,
            _ if key.len() == 1 && key.as_bytes()[0].is_ascii_alphabetic() => {
                key.as_bytes()[0].to_ascii_uppercase() - b'@'
            }
            _ => return None,
        };
        return Some(char::from(byte).to_string());
    }
    // Option on Mac and AltGr on Windows can generate printable text.
    if !cfg!(target_os = "macos") && m.alt && !m.control {
        if key == "space" {
            return Some("\x1b ".into());
        }
        if key.len() == 1 && key.is_ascii() {
            let ch = key.as_bytes()[0] as char;
            let shifted = if m.shift {
                const BASE: &str = "0123456789;=,-./`[\\]'";
                const SHIFTED: &str = ")!@#$%^&*(:+<_>?~{|}\"";
                BASE.find(ch)
                    .map(|i| SHIFTED.as_bytes()[i] as char)
                    .unwrap_or_else(|| ch.to_ascii_uppercase())
            } else {
                ch
            };
            return Some(format!("\x1b{shifted}"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_function_and_editing_keys_match_xterm() {
        let plain = Modifiers::default();
        let ctrl = Modifiers {
            control: true,
            ..plain
        };
        let shift = Modifiers {
            shift: true,
            ..plain
        };
        let alt = Modifiers { alt: true, ..plain };
        for (key, mods, app, expected) in [
            ("left", ctrl, false, "\x1b[1;5D"),
            ("up", shift, true, "\x1b[1;2A"),
            ("home", plain, true, "\x1bOH"),
            ("end", plain, false, "\x1b[F"),
            ("f4", plain, false, "\x1bOS"),
            ("f12", ctrl, true, "\x1b[24;5~"),
            ("delete", alt, false, "\x1b[3;3~"),
            ("backspace", ctrl, true, "\x08"),
            ("escape", alt, true, "\x1b\x1b"),
            ("pageup", ctrl, true, "\x1b[5;5~"),
            ("space", ctrl, false, "\0"),
            ("6", ctrl, true, "\x1e"),
        ] {
            assert_eq!(sequence(key, mods, app).as_deref(), Some(expected), "{key}");
        }
        for (key, mods) in [
            ("insert", shift),
            ("pageup", shift),
            ("9", ctrl),
            ("界", plain),
        ] {
            assert_eq!(sequence(key, mods, false), None, "{key}");
        }
    }
}
