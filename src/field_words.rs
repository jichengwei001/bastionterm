use icu_segmenter::WordSegmenter;
use std::ops::Range;

fn ranges(text: &str) -> Vec<Range<usize>> {
    // Browsers stop at the dot in a hostname but keep decimal numbers together.
    // The substitute has the same byte width so offsets still index the source.
    let bytes = text.as_bytes();
    let tailored = text
        .char_indices()
        .map(|(i, c)| {
            if c == ':'
                || (c == '.'
                    && !(i > 0
                        && bytes[i - 1].is_ascii_digit()
                        && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)))
            {
                '-'
            } else {
                c
            }
        })
        .collect::<String>();
    let mut groups: Vec<Range<usize>> = Vec::new();
    let mut start = 0;
    let mut previous_punctuation = false;
    for end in WordSegmenter::new_dictionary(Default::default())
        .segment_str(&tailored)
        .skip(1)
    {
        let punct = text[start..end]
            .chars()
            .all(|c| c.is_ascii_punctuation() && c != '_');
        if punct && previous_punctuation {
            groups.last_mut().unwrap().end = end;
        } else {
            groups.push(start..end);
        }
        previous_punctuation = punct;
        start = end;
    }
    groups
}

pub(super) fn at(text: &str, offset: usize, password: bool) -> Range<usize> {
    if password {
        return 0..text.len();
    }
    ranges(text)
        .into_iter()
        .find(|r| r.contains(&offset))
        .unwrap_or(text.len()..text.len())
}

pub(super) fn step(text: &str, cursor: usize, right: bool, password: bool) -> usize {
    if password {
        return if right { text.len() } else { 0 };
    }
    let spans = ranges(text);
    let blank = |r: &Range<usize>| text[r.clone()].chars().all(char::is_whitespace);
    if !right {
        return spans
            .iter()
            .rev()
            .find(|r| r.start < cursor && !blank(r))
            .map_or(0, |r| r.start);
    }
    if cfg!(windows) {
        // Windows Ctrl+Right goes to the next word's start, across its leading spaces.
        spans
            .iter()
            .find(|r| r.start > cursor && !blank(r))
            .map_or(text.len(), |r| r.start)
    } else {
        spans
            .iter()
            .find(|r| r.end > cursor && !blank(r))
            .map_or(text.len(), |r| r.end)
    }
}

#[cfg(test)]
mod tests {
    use super::{at, step};
    #[test]
    fn hostname_punctuation_decimal_ports_and_dictionary_words() {
        let name = "qa.host 10.2 中文测试主机";
        for word in ["qa", ".", "host", "10.2", "中文", "测试", "主机"] {
            let start = name.find(word).unwrap();
            assert_eq!(&name[at(name, start, false)], word);
        }
        assert_eq!(
            step(name, name.len(), false, false),
            name.find("主机").unwrap()
        );
        assert_eq!(at(name, 2, true), 0..name.len());
    }
}
