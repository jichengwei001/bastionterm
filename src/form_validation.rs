//! Constraint validation for the desktop connection form.
use gpui::SharedString;
use std::time::{Duration, Instant};

pub struct Hint {
    pub field: usize,
    pub submitted_value: SharedString,
    pub message: SharedString,
    pub expires: Instant,
    #[cfg(target_os = "macos")]
    pub popover: Option<crate::mac_window::ValidationPopover>,
}

impl Hint {
    pub fn new(field: usize, submitted_value: SharedString, message: String) -> Self {
        Self {
            field,
            submitted_value,
            message: message.into(),
            expires: Instant::now() + Duration::from_secs(5),
            #[cfg(target_os = "macos")]
            popover: None,
        }
    }
}

pub fn message(value: &str, port: bool) -> Option<String> {
    #[cfg(windows)]
    // Windows selects the validation UI language independently of number formatting.
    let chinese =
        unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff == 4;
    #[cfg(not(windows))]
    let chinese = false;
    let text = if value.is_empty() {
        if cfg!(target_os = "macos") {
            "Fill out this field"
        } else if chinese {
            "请填写此字段。"
        } else {
            "Please fill out this field."
        }
    } else if !port {
        return None;
    } else {
        match value.parse::<f64>() {
            Ok(n) if n < 1. => {
                if chinese {
                    "值必须大于或等于 1。"
                } else if cfg!(target_os = "macos") {
                    "Value must be greater than or equal to 1"
                } else {
                    "Value must be greater than or equal to 1."
                }
            }
            Ok(n) if n > 65535. => {
                if chinese {
                    "值必须小于或等于 65535。"
                } else if cfg!(target_os = "macos") {
                    "Value must be less than or equal to 65535"
                } else {
                    "Value must be less than or equal to 65535."
                }
            }
            Ok(n) if n.fract() == 0. => return None,
            Ok(n) if n.is_finite() && !cfg!(target_os = "macos") => {
                let (lo, hi) = (n.floor(), n.ceil());
                return Some(if chinese {
                    format!("请输入一个有效的值。最接近的两个有效值为 {lo} 和 {hi}。")
                } else {
                    format!(
                        "Please enter a valid value. The two nearest valid values are {lo} and {hi}."
                    )
                });
            }
            Ok(_) => "Enter a valid value",
            Err(_) if chinese => "请输入一个数字。",
            Err(_) if cfg!(target_os = "macos") => "Enter a number",
            Err(_) => "Please enter a number.",
        }
    };
    Some(text.into())
}

#[cfg(not(target_os = "macos"))]
pub fn bubble(message: SharedString, light: bool) -> impl gpui::IntoElement {
    use gpui::{prelude::*, *};
    let background = if light { 0xffffff } else { 0x3b3b3b };
    let border = if light { 0x828282 } else { 0x626262 };
    let arrow = canvas(
        |_, _, _| (),
        move |b, _, w, _| {
            for (inset, color) in [(0., border), (1.3, background)] {
                let mut path = PathBuilder::fill();
                path.move_to(point(b.left() + px(inset), b.bottom()));
                path.line_to(point(b.center().x, b.top() + px(inset)));
                path.line_to(point(b.right() - px(inset), b.bottom()));
                path.close();
                if let Ok(path) = path.build() {
                    w.paint_path(path, rgb(color));
                }
            }
        },
    )
    .absolute()
    .top(px(-8.))
    .left(px(9.))
    .w(px(16.))
    .h(px(9.));
    let body = div()
        .relative()
        .flex()
        .items_center()
        .gap(px(8.))
        .max_w(px(300.))
        .px(px(8.))
        .py(px(8.))
        .rounded(px(4.))
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(background))
        .shadow_sm()
        .text_size(px(13.))
        .line_height(px(19.))
        .text_color(rgb(if light { 0x181818 } else { 0xffffff }))
        .child(
            div()
                .flex_none()
                .size(px(23.))
                .rounded(px(2.))
                .bg(rgb(0xed5d00))
                .text_color(rgb(0xffffff))
                .text_size(px(17.))
                .font_weight(FontWeight::BOLD)
                .line_height(px(23.))
                .text_center()
                .child("!"),
        )
        .child(div().min_w_0().child(message))
        .child(arrow);
    div()
        .absolute()
        .top(relative(1.))
        .left(px(-130.))
        .right(px(-130.))
        .child(deferred(div().pt(px(8.)).flex().justify_center().child(body)).with_priority(3))
}

#[cfg(test)]
mod tests {
    use super::message;

    #[test]
    fn form_constraints_distinguish_missing_range_and_step() {
        let required = message("", false).unwrap();
        assert_eq!(message("", true).as_deref(), Some(required.as_str()));
        assert!(message("ssh.example", false).is_none());
        for value in ["1", "0022", "65535", "2e1"] {
            assert!(message(value, true).is_none(), "{value}");
        }
        let lower = message("0", true).unwrap();
        let upper = message("65536", true).unwrap();
        let fractional = message("1.5", true).unwrap();
        assert!(lower.contains('1') && upper.contains("65535"));
        assert_ne!(fractional, lower);
        assert_ne!(fractional, upper);
        assert!(message("-", true).is_some());
    }
}
