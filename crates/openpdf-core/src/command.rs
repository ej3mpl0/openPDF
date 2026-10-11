/// Reversible edit recorded in the undo stack.
#[derive(Clone, Debug)]
pub enum Command {
    Rotate { index: usize, delta: i32 },
    Delete { index: usize },
    Move { from: usize, to: usize },
}

#[derive(Clone, Debug)]
pub struct History {
    undo: Vec<Applied>,
    redo: Vec<Applied>,
}

#[derive(Clone, Debug)]
pub(crate) enum Applied {
    Rotate { index: usize, delta: i32 },
    Delete { index: usize, slot: super::document::PageSlot },
    Insert { index: usize },
    Move { from: usize, to: usize },
}

impl History {
    pub(crate) fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, applied: Applied) {
        self.undo.push(applied);
        self.redo.clear();
    }

    pub(crate) fn pop_undo(&mut self) -> Option<Applied> {
        self.undo.pop()
    }

    pub(crate) fn pop_redo(&mut self) -> Option<Applied> {
        self.redo.pop()
    }

    pub(crate) fn record_redo(&mut self, applied: Applied) {
        self.redo.push(applied);
    }

    pub(crate) fn record_undo(&mut self, applied: Applied) {
        self.undo.push(applied);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn len(&self) -> usize {
        self.undo.len()
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }
}

pub(crate) fn normalize_rotation(degrees: i32) -> i32 {
    let mut r = degrees % 360;
    if r < 0 {
        r += 360;
    }
    // Snap to PDF-legal values.
    match r {
        0..=44 | 316..=359 => 0,
        45..=134 => 90,
        135..=224 => 180,
        _ => 270,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_snaps() {
        assert_eq!(normalize_rotation(90), 90);
        assert_eq!(normalize_rotation(-90), 270);
        assert_eq!(normalize_rotation(450), 90);
        assert_eq!(normalize_rotation(0), 0);
    }

    #[test]
    fn history_undo_redo_flags() {
        let mut h = History::new();
        assert!(!h.can_undo());
        h.push(Applied::Move { from: 0, to: 1 });
        assert!(h.can_undo());
        assert!(!h.can_redo());
        let a = h.pop_undo().unwrap();
        h.record_redo(a);
        assert!(h.can_redo());
    }
}
