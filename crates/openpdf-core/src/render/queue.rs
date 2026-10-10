use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};

use super::{Priority, TileKey};

#[derive(Clone, Eq, PartialEq)]
struct Job {
    priority: Priority,
    seq: u64,
    generation: u64,
    key: TileKey,
}

impl Ord for Job {
    fn cmp(&self, other: &Self) -> Ordering {
        // Lower Priority discriminant first (Visible before Thumbnail).
        other
            .priority
            .cmp(&self.priority)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

impl PartialOrd for Job {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Priority queue that drops stale work when the generation bumps or a key is cancelled.
pub struct RenderQueue {
    heap: BinaryHeap<Job>,
    next_seq: u64,
    generation: u64,
    cancelled: HashSet<TileKey>,
}

impl RenderQueue {
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            next_seq: 1,
            generation: 1,
            cancelled: HashSet::new(),
        }
    }

    #[allow(dead_code)]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn bump_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.generation = 1;
        }
        self.cancelled.clear();
        self.generation
    }

    pub fn push(&mut self, key: TileKey, priority: Priority) {
        self.cancelled.remove(&key);
        let seq = self.next_seq;
        self.next_seq += 1;
        self.heap.push(Job {
            priority,
            seq,
            generation: self.generation,
            key,
        });
    }

    pub fn cancel(&mut self, key: TileKey) {
        self.cancelled.insert(key);
    }

    pub fn cancel_except(&mut self, keep: &[TileKey]) {
        let keep: HashSet<_> = keep.iter().copied().collect();
        for job in self.heap.iter() {
            if !keep.contains(&job.key) {
                self.cancelled.insert(job.key);
            }
        }
    }

    pub fn is_cancelled(&self, key: &TileKey, generation: u64) -> bool {
        generation != self.generation || self.cancelled.contains(key)
    }

    pub fn pop(&mut self) -> Option<(TileKey, Priority, u64)> {
        while let Some(job) = self.heap.pop() {
            if self.is_cancelled(&job.key, job.generation) {
                continue;
            }
            return Some((job.key, job.priority, job.generation));
        }
        None
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.heap.len()
    }
}

impl Default for RenderQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::DocumentId;

    fn key(x: u32) -> TileKey {
        TileKey {
            doc: DocumentId(1),
            page: 0,
            zoom_q: 1000,
            x,
            y: 0,
        }
    }

    #[test]
    fn visible_before_thumbnail() {
        let mut q = RenderQueue::new();
        q.push(key(1), Priority::Thumbnail);
        q.push(key(2), Priority::Visible);
        q.push(key(3), Priority::Adjacent);
        let first = q.pop().unwrap();
        assert_eq!(first.0.x, 2);
        assert_eq!(first.1, Priority::Visible);
    }

    #[test]
    fn bump_generation_drops_stale() {
        let mut q = RenderQueue::new();
        q.push(key(1), Priority::Visible);
        q.bump_generation();
        q.push(key(2), Priority::Visible);
        let (k, _, _) = q.pop().unwrap();
        assert_eq!(k.x, 2);
        assert!(q.pop().is_none());
    }

    #[test]
    fn cancel_except_keeps_named_keys() {
        let mut q = RenderQueue::new();
        q.push(key(1), Priority::Visible);
        q.push(key(2), Priority::Visible);
        q.cancel_except(&[key(2)]);
        let (k, _, _) = q.pop().unwrap();
        assert_eq!(k.x, 2);
        assert!(q.pop().is_none());
    }

    #[test]
    fn same_priority_is_fifo_so_first_requested_is_first_popped() {
        let mut q = RenderQueue::new();
        q.push(key(0), Priority::Visible);
        q.push(key(1), Priority::Visible);
        q.push(key(2), Priority::Visible);
        assert_eq!(q.pop().unwrap().0.x, 0);
        assert_eq!(q.pop().unwrap().0.x, 1);
        assert_eq!(q.pop().unwrap().0.x, 2);
    }
}
