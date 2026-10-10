use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::document::DocumentId;

use super::{Tile, TileKey};

/// LRU tile cache capped by decoded RGBA bytes.
pub struct TileCache {
    cap: usize,
    used: usize,
    map: HashMap<TileKey, Arc<Tile>>,
    order: VecDeque<TileKey>,
}

impl TileCache {
    pub fn new(cap_bytes: usize) -> Self {
        Self {
            cap: cap_bytes.max(1),
            used: 0,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn bytes_used(&self) -> usize {
        self.used
    }

    /// `(Arc ptr, RGBA bytes)` for each cached tile. Same pointer is one buffer.
    pub fn buffer_idents(&self) -> Vec<(usize, usize)> {
        self.map
            .values()
            .map(|t| (Arc::as_ptr(t) as usize, t.byte_len()))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Change the byte budget. Evicts LRU tiles when the new cap is smaller
    /// than [`Self::bytes_used`].
    pub fn set_cap(&mut self, cap_bytes: usize) {
        self.cap = cap_bytes.max(1);
        while self.used > self.cap {
            if !self.evict_one() {
                break;
            }
        }
    }

    pub fn get(&mut self, key: &TileKey) -> Option<Arc<Tile>> {
        let tile = self.map.get(key).cloned()?;
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
            self.order.push_back(*key);
        }
        Some(tile)
    }

    pub fn insert(&mut self, tile: Arc<Tile>) {
        if tile.byte_len() > self.cap {
            // Single tile larger than the cap: keep only this one.
            self.map.clear();
            self.order.clear();
            self.used = 0;
        }
        if let Some(old) = self.map.remove(&tile.key) {
            self.used = self.used.saturating_sub(old.byte_len());
            if let Some(pos) = self.order.iter().position(|k| *k == tile.key) {
                self.order.remove(pos);
            }
        }
        while self.used + tile.byte_len() > self.cap {
            if !self.evict_one() {
                break;
            }
        }
        self.used += tile.byte_len();
        self.order.push_back(tile.key);
        self.map.insert(tile.key, tile);
    }

    pub fn invalidate_doc(&mut self, doc: DocumentId) {
        let keys: Vec<TileKey> = self.map.keys().filter(|k| k.doc == doc).copied().collect();
        for key in keys {
            if let Some(old) = self.map.remove(&key) {
                self.used = self.used.saturating_sub(old.byte_len());
            }
            if let Some(pos) = self.order.iter().position(|k| *k == key) {
                self.order.remove(pos);
            }
        }
    }

    fn evict_one(&mut self) -> bool {
        let Some(key) = self.order.pop_front() else {
            return false;
        };
        if let Some(old) = self.map.remove(&key) {
            self.used = self.used.saturating_sub(old.byte_len());
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::DocumentId;

    fn tile(doc: u64, x: u32, bytes: usize) -> Arc<Tile> {
        Arc::new(Tile {
            key: TileKey {
                doc: DocumentId(doc),
                page: 0,
                zoom_q: 1000,
                x,
                y: 0,
            },
            width: 1,
            height: 1,
            rgba: vec![0; bytes.max(4)],
        })
    }

    #[test]
    fn evicts_oldest_when_over_cap() {
        let mut cache = TileCache::new(12);
        cache.insert(tile(1, 0, 4));
        cache.insert(tile(1, 1, 4));
        cache.insert(tile(1, 2, 4));
        cache.insert(tile(1, 3, 4));
        assert_eq!(cache.len(), 3);
        assert!(cache
            .get(&TileKey {
                doc: DocumentId(1),
                page: 0,
                zoom_q: 1000,
                x: 0,
                y: 0,
            })
            .is_none());
        assert!(cache
            .get(&TileKey {
                doc: DocumentId(1),
                page: 0,
                zoom_q: 1000,
                x: 3,
                y: 0,
            })
            .is_some());
    }

    #[test]
    fn invalidate_drops_one_document() {
        let mut cache = TileCache::new(64);
        cache.insert(tile(1, 0, 4));
        cache.insert(tile(2, 0, 4));
        cache.invalidate_doc(DocumentId(1));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn set_cap_evicts_when_shrinking() {
        let mut cache = TileCache::new(16);
        cache.insert(tile(1, 0, 4));
        cache.insert(tile(1, 1, 4));
        cache.insert(tile(1, 2, 4));
        cache.insert(tile(1, 3, 4));
        assert_eq!(cache.len(), 4);
        cache.set_cap(8);
        assert_eq!(cache.cap(), 8);
        assert!(cache.bytes_used() <= 8);
        assert_eq!(cache.len(), 2);
        assert!(cache
            .get(&TileKey {
                doc: DocumentId(1),
                page: 0,
                zoom_q: 1000,
                x: 0,
                y: 0,
            })
            .is_none());
        assert!(cache
            .get(&TileKey {
                doc: DocumentId(1),
                page: 0,
                zoom_q: 1000,
                x: 3,
                y: 0,
            })
            .is_some());
    }
}
