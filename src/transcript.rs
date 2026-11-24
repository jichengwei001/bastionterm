// Offsets in the terminal API count decoded Unicode scalar values.
use std::collections::VecDeque;

const LIMIT: usize = 2_000_000;

#[derive(Clone, Default)]
pub struct Transcript {
    chunks: VecDeque<(usize, String)>,
    base: usize,
    end: usize,
    incomplete: Vec<u8>,
}

impl Transcript {
    pub fn append(&mut self, bytes: &[u8], final_chunk: bool) {
        let mut input = std::mem::take(&mut self.incomplete);
        input.extend_from_slice(bytes);
        let mut rest = &input[..];
        let mut decoded = String::new();
        while !rest.is_empty() {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    decoded.push_str(s);
                    break;
                }
                Err(e) => {
                    let (valid, tail) = rest.split_at(e.valid_up_to());
                    decoded.push_str(std::str::from_utf8(valid).unwrap());
                    match e.error_len() {
                        Some(n) => {
                            decoded.push('\u{fffd}');
                            rest = &tail[n..];
                        }
                        None => {
                            if final_chunk {
                                decoded.push('\u{fffd}');
                            } else {
                                self.incomplete.extend_from_slice(tail);
                            }
                            break;
                        }
                    }
                }
            }
        }
        if decoded.is_empty() {
            return;
        }
        let n = decoded.chars().count();
        self.end += n;
        if let Some((count, last)) = self
            .chunks
            .back_mut()
            .filter(|(_, last)| last.len() + decoded.len() <= 8192)
        {
            last.push_str(&decoded);
            *count += n;
        } else {
            self.chunks.push_back((n, decoded));
        }
        let mut excess = self.end.saturating_sub(self.base + LIMIT);
        self.base += excess;
        while excess > 0 {
            let (n, first) = self.chunks.front_mut().unwrap();
            if *n > excess {
                let cut = first.char_indices().nth(excess).unwrap().0;
                first.drain(..cut);
                *n -= excess;
                break;
            }
            excess -= *n;
            self.chunks.pop_front();
        }
    }

    pub fn read_terminal(&self, offset: usize) -> (String, usize, bool) {
        // A viewer may still have the cursor from a previously closed tab for this host.
        self.read(if offset > self.end { self.base } else { offset })
    }

    pub fn read(&self, offset: usize) -> (String, usize, bool) {
        let mut remaining = self.end - offset.clamp(self.base, self.end);
        let mut parts = Vec::new();
        for (n, chunk) in self.chunks.iter().rev() {
            if remaining == 0 {
                break;
            }
            if remaining < *n {
                let i = chunk.char_indices().nth(n - remaining).unwrap().0;
                parts.push(&chunk[i..]);
                break;
            }
            parts.push(chunk.as_str());
            remaining -= n;
        }
        (
            parts.into_iter().rev().collect(),
            self.end,
            offset < self.base,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offsets_survive_fragmented_utf8_and_truncation() {
        let mut text = Transcript::default();
        for b in "你😀".as_bytes() {
            text.append(&[*b], false);
        }
        assert_eq!(text.read(1), ("😀".into(), 2, false));
        text.append(&vec![b'x'; LIMIT - 1], false);
        let (s, end, truncated) = text.read(0);
        assert!(truncated);
        assert_eq!(end, LIMIT + 1);
        assert!(s.starts_with('😀'));
        assert_eq!(s.chars().count(), LIMIT);
        assert_eq!(text.read(end - 1), ("x".into(), end, false));
    }
}
