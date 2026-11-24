use gpui::SharedString;
use std::{collections::VecDeque, ops::Range};

#[derive(Clone, PartialEq)]
pub(super) struct FieldState {
    pub text: SharedString,
    pub selection: Range<usize>,
    pub reversed: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Edit {
    Typing,
    Backspace,
    Delete,
    Separate,
}

struct Change {
    before: FieldState,
    after: FieldState,
}

impl Change {
    fn bytes(&self) -> usize {
        self.before.text.len() + self.after.text.len()
    }
}

#[derive(Default)]
pub(super) struct FieldHistory {
    changes: VecDeque<Change>,
    applied: usize,
    bytes: usize,
    group: Option<Edit>,
}

impl FieldHistory {
    pub fn can_undo(&self) -> bool {
        self.applied > 0
    }
    pub fn can_redo(&self) -> bool {
        self.applied < self.changes.len()
    }

    pub fn end_group(&mut self) {
        self.group = None;
    }

    pub fn record(&mut self, mut before: FieldState, after: FieldState, kind: Edit) {
        if before.text == after.text {
            return;
        }
        while self.changes.len() > self.applied {
            self.bytes -= self.changes.pop_back().unwrap().bytes();
        }
        let continuing =
            self.group == Some(kind) && self.changes.back().is_some_and(|c| c.after == before);
        // On macOS undoing Backspace selects the characters which it restored.
        if cfg!(target_os = "macos") && before.selection.is_empty() {
            let removed = before.text.len().saturating_sub(after.text.len());
            match kind {
                Edit::Backspace => before.selection.start -= removed,
                Edit::Delete => before.selection.end += removed,
                _ => {}
            }
        }
        if continuing {
            let change = self.changes.back_mut().unwrap();
            self.bytes -= change.bytes();
            if cfg!(target_os = "macos") && kind == Edit::Backspace {
                change.before.selection.start = before.selection.start;
            }
            change.after = after;
            self.bytes += change.bytes();
        } else {
            let change = Change { before, after };
            self.bytes += change.bytes();
            self.changes.push_back(change);
        }
        // Bound retained revisions even for unusually large clipboard pastes.
        while self.changes.len() > 1 && (self.changes.len() > 256 || self.bytes > 4 * 1024 * 1024) {
            self.bytes -= self.changes.pop_front().unwrap().bytes();
        }
        self.applied = self.changes.len();
        self.group = (kind != Edit::Separate).then_some(kind);
    }

    pub fn undo(&mut self) -> Option<FieldState> {
        self.end_group();
        self.applied = self.applied.checked_sub(1)?;
        Some(self.changes[self.applied].before.clone())
    }

    pub fn redo(&mut self) -> Option<FieldState> {
        self.end_group();
        let state = self.changes.get(self.applied)?.after.clone();
        self.applied += 1;
        Some(state)
    }
}

#[cfg(test)]
mod tests {
    use super::{Edit, FieldHistory, FieldState};

    fn at(text: &str, cursor: usize) -> FieldState {
        FieldState {
            text: text.to_owned().into(),
            selection: cursor..cursor,
            reversed: false,
        }
    }

    #[test]
    fn replacement_group_restores_reversed_selection_and_branches_on_edit() {
        let mut h = FieldHistory::default();
        let mut initial = at("old", 0);
        initial.selection = 0..3;
        initial.reversed = true;
        h.record(initial.clone(), at("新", 3), Edit::Typing);
        h.record(at("新", 3), at("新名", 6), Edit::Typing);
        h.record(at("新名", 6), at("新名-2", 8), Edit::Separate);
        assert!(h.undo().unwrap() == at("新名", 6));
        assert!(h.undo().unwrap() == initial);
        assert!(h.redo().unwrap() == at("新名", 6));
        h.record(at("新名", 6), at("新名A", 7), Edit::Typing);
        assert!(h.redo().is_none());
    }
}
