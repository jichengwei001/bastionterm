// VT character mappings used by xterm.js. Copyright (c) 2016 The xterm.js authors.
// MIT license, see assets/browser/LICENSE-xterm.

#[derive(Clone, Debug)]
pub(crate) struct Charsets {
    slots: [u8; 4],
    level: usize,
    pub active: u8,
}

impl Default for Charsets {
    fn default() -> Self {
        Self {
            slots: [b'B'; 4],
            level: 0,
            active: b'B',
        }
    }
}

impl Charsets {
    pub fn select(&mut self, level: usize) {
        self.level = level;
        self.active = self.slots[level];
    }

    pub fn designate(&mut self, level: usize, name: u8) {
        if name != b'B' && mapping(name).is_none() {
            return;
        }
        self.slots[level] = name;
        if level == self.level {
            self.active = name;
        }
    }

    pub fn decode(&self, ch: char) -> char {
        if self.active == b'B' || !ch.is_ascii() {
            return ch;
        }
        let Some((source, target)) = mapping(self.active) else {
            return ch;
        };
        source
            .find(ch)
            .and_then(|i| target.chars().nth(i))
            .unwrap_or(ch)
    }
}

// Source positions are single-byte ASCII. Each replacement occupies one cell.
fn mapping(name: u8) -> Option<(&'static str, &'static str)> {
    Some(match name {
        b'0' => (
            "`abcdefghijklmnopqrstuvwxyz{|}~",
            "◆▒␉␌␍␊°±␤␋┘┐┌└┼⎺⎻─⎼⎽├┤┴┬│≤≥π≠£·",
        ),
        b'A' => ("#", "£"),
        b'4' => ("#@[\\]{|}~", "£¾i½|¨f¼´"),
        b'C' | b'5' => ("[\\]^`{|}~", "ÄÖÅÜéäöåü"),
        b'R' => ("#@[\\]{|}~", "£à°ç§éùè¨"),
        b'Q' => ("@[\\]^`{|}~", "àâçêîôéùèû"),
        b'K' => ("@[\\]{|}~", "§ÄÖÜäöüß"),
        b'Y' => ("#@[\\]`{|}~", "£§°çéùàòèì"),
        b'E' | b'6' => ("@[\\]^`{|}~", "ÄÆØÅÜäæøåü"),
        b'Z' => ("#@[\\]{|}", "£§¡Ñ¿°ñç"),
        b'H' | b'7' => ("@[\\]^`{|}~", "ÉÄÖÅÜéäöåü"),
        b'=' => ("#@[\\]^_`{|}~", "ùàéçêîèôäöüû"),
        _ => return None,
    })
}
