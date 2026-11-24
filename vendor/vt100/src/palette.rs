//! Indexed and special terminal colors, including application changes made with OSC.
use std::{collections::BTreeMap, io::Write, sync::Arc};

const FG: usize = 256;
const BG: usize = 257;
const CURSOR: usize = 258;

#[derive(Clone, Debug)]
pub struct Palette {
    base: Arc<[u32; 259]>,
    changes: Arc<BTreeMap<usize, u32>>,
}

impl Palette {
    pub fn new(ansi: [u32; 16], foreground: u32, background: u32, cursor: u32) -> Self {
        let mut base = [0; 259];
        base[..16].copy_from_slice(&ansi);
        for (i, slot) in base[16..232].iter_mut().enumerate() {
            let levels = [0, 95, 135, 175, 215, 255];
            *slot = levels[i / 36] << 16 | levels[i / 6 % 6] << 8 | levels[i % 6];
        }
        for (i, slot) in base[232..256].iter_mut().enumerate() {
            *slot = (8 + 10 * i as u32) * 0x010101;
        }
        base[FG..].copy_from_slice(&[foreground, background, cursor]);
        Self {
            base: Arc::new(base),
            changes: Default::default(),
        }
    }
    fn get(&self, slot: usize) -> u32 {
        self.changes.get(&slot).copied().unwrap_or(self.base[slot])
    }
    pub fn indexed(&self, idx: u8) -> u32 {
        self.get(usize::from(idx))
    }
    pub fn foreground(&self) -> u32 {
        self.get(FG)
    }
    pub fn background(&self) -> u32 {
        self.get(BG)
    }
    pub fn cursor(&self) -> u32 {
        self.get(CURSOR)
    }
    pub(crate) fn rebase(&mut self, theme: &Self, reset: bool) {
        self.base = theme.base.clone();
        if reset {
            self.changes = Default::default();
        }
    }
    fn apply(&mut self, slot: usize, spec: &[u8], reply: Option<&mut Vec<u8>>) {
        if spec == b"?" {
            if let Some(out) = reply {
                let color = self.get(slot);
                if slot < 256 {
                    write!(out, "\x1b]4;{slot};").unwrap()
                } else {
                    write!(out, "\x1b]{};", slot - FG + 10).unwrap()
                }
                let [_, r, g, b] = color.to_be_bytes();
                write!(
                    out,
                    "rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x1b\\"
                )
                .unwrap()
            }
        } else if let Some(rgb) = parse(spec) {
            Arc::make_mut(&mut self.changes).insert(slot, rgb);
        }
    }
    pub(crate) fn osc(&mut self, tag: &[u8], data: &[u8], mut reply: Option<&mut Vec<u8>>) -> bool {
        let mut fields = data.split(|&b| b == b';');
        match tag {
            b"4" => {
                while let (Some(slot), Some(spec)) = (fields.next(), fields.next()) {
                    if let Some(index) = index(slot) {
                        self.apply(index, spec, reply.as_deref_mut())
                    }
                }
            }
            b"10" | b"11" | b"12" => {
                for (slot, spec) in (FG + usize::from(tag[1] - b'0')..=CURSOR).zip(fields) {
                    self.apply(slot, spec, reply.as_deref_mut());
                }
            }
            b"104" => {
                let changes = Arc::make_mut(&mut self.changes);
                if data.is_empty() {
                    changes.retain(|slot, _| *slot >= FG)
                } else {
                    for slot in fields.filter_map(index) {
                        changes.remove(&slot);
                    }
                }
            }
            b"110" | b"111" | b"112" => {
                Arc::make_mut(&mut self.changes).remove(&(FG + usize::from(tag[2] - b'0')));
            }
            _ => return false,
        }
        true
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::new(
            [
                0x2e3436, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf,
                0x555753, 0xef2929, 0x8ae234, 0xfce94f, 0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec,
            ],
            0xffffff,
            0,
            0xffffff,
        )
    }
}
fn index(data: &[u8]) -> Option<usize> {
    if !data.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(data)
        .ok()?
        .parse::<u8>()
        .ok()
        .map(usize::from)
}
fn parse(data: &[u8]) -> Option<u32> {
    let value = std::str::from_utf8(data).ok()?;
    fn hex(v: &str) -> Option<u32> {
        if !v.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(v, 16).ok()
    }
    let channels = if let Some(s) = value.strip_prefix('#') {
        if !s.is_ascii() || !matches!(s.len(), 3 | 6 | 9 | 12) {
            return None;
        }
        let n = s.len() / 3;
        let channel = |v: &str| hex(v).map(|x| if n == 1 { x << 4 } else { x >> ((n - 2) * 4) });
        [
            channel(&s[..n])?,
            channel(&s[n..2 * n])?,
            channel(&s[2 * n..])?,
        ]
    } else {
        let (prefix, s) = value.split_once(':')?;
        if !prefix.eq_ignore_ascii_case("rgb") {
            return None;
        }
        let mut fields = s.split('/');
        let (r, g, b) = (fields.next()?, fields.next()?, fields.next()?);
        if fields.next().is_some()
            || !(1..=4).contains(&r.len())
            || g.len() != r.len()
            || b.len() != r.len()
        {
            return None;
        }
        let max = (1u32 << (4 * r.len())) - 1;
        let channel = |v: &str| hex(v).map(|x| (x * 255 + max / 2) / max);
        [channel(r)?, channel(g)?, channel(b)?]
    };
    Some(channels[0] << 16 | channels[1] << 8 | channels[2])
}
