use serde_json::Value;
use std::sync::OnceLock;
pub fn skins() -> &'static [Value] {
    static SKINS: OnceLock<Vec<Value>> = OnceLock::new();
    SKINS.get_or_init(|| serde_json::from_str(include_str!("skins.json")).unwrap())
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Theme(pub usize);
impl Theme {
    pub fn named(id: &str) -> Option<Self> {
        skins().iter().position(|s| s["id"] == id).map(Self)
    }
    pub fn saved(dir: &std::path::Path) -> Self {
        std::fs::read(dir.join("appearance.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| Self::named(v["skin"].as_str()?))
            .unwrap_or_default()
    }
    pub fn colors(self) -> &'static vt100::Palette {
        static COLORS: OnceLock<Vec<vt100::Palette>> = OnceLock::new();
        &COLORS.get_or_init(|| {
            (0..skins().len())
                .map(|i| {
                    let theme = Self(i);
                    let ansi = [
                        "black",
                        "red",
                        "green",
                        "yellow",
                        "blue",
                        "magenta",
                        "cyan",
                        "white",
                        "brightBlack",
                        "brightRed",
                        "brightGreen",
                        "brightYellow",
                        "brightBlue",
                        "brightMagenta",
                        "brightCyan",
                        "brightWhite",
                    ];
                    vt100::Palette::new(
                        ansi.map(|name| theme.ansi(name)),
                        theme.ansi("foreground"),
                        theme.ansi("background"),
                        theme.ansi("cursor"),
                    )
                })
                .collect()
        })[self.0]
    }
    pub fn viewport(self, rows: u16, cols: u16) -> vt100::Parser {
        let mut p = vt100::Parser::new_viewport(rows, cols, 10000);
        p.set_palette(self.colors(), true);
        p
    }
    pub fn palette(&self) -> &'static Value {
        &skins()[self.0]
    }
    pub fn color(&self, k: &str) -> u32 {
        parse(&self.palette()["ui"][k])
    }
    pub fn ansi(&self, k: &str) -> u32 {
        parse(&self.palette()["terminal"][k])
    }
}
fn parse(v: &Value) -> u32 {
    u32::from_str_radix(v.as_str().unwrap().trim_start_matches('#'), 16).unwrap()
}
