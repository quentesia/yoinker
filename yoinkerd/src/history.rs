use std::time::{SystemTime, UNIX_EPOCH};
use yoinker_common::{ClipboardEntry, Config, EntryContent};

pub struct ClipboardHistory {
    pub entries: Vec<ClipboardEntry>,
    pub config: Config,
    next_id: u64,
    last_hash: u64,
    dirty: bool,
}

impl ClipboardHistory {
    pub fn load(config: &Config) -> Self {
        let mut entries = Vec::new();
        let mut next_id = 1;

        if config.history_path.exists() {
            if let Ok(data) = std::fs::read_to_string(&config.history_path) {
                if let Ok(loaded) = serde_json::from_str::<Vec<ClipboardEntry>>(&data) {
                    next_id = loaded.iter().map(|e| e.id).max().unwrap_or(0) + 1;
                    entries = loaded;
                }
            }
        }

        let last_hash = entries
            .first()
            .map(|e| e.content.content_hash())
            .unwrap_or(0);

        Self {
            entries,
            config: config.clone(),
            next_id,
            last_hash,
            dirty: false,
        }
    }

    pub fn save(&mut self) {
        if let Some(parent) = self.config.history_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Ok(data) = serde_json::to_string_pretty(&self.entries) {
            let tmp_path = self.config.history_path.with_extension("json.tmp");
            if std::fs::write(&tmp_path, &data).is_ok() {
                std::fs::rename(&tmp_path, &self.config.history_path).ok();
            }
        }
        self.dirty = false;
    }

    pub fn save_if_dirty(&mut self) {
        if self.dirty {
            self.save();
        }
    }

    /// Add a new entry. Returns true if it was actually added (not a duplicate).
    pub fn add(&mut self, content: EntryContent) -> bool {
        let hash = content.content_hash();
        if hash == self.last_hash {
            return false;
        }
        self.last_hash = hash;

        // If this content already exists, move it to the front
        if let Some(pos) = self
            .entries
            .iter()
            .position(|e| e.content.content_hash() == hash)
        {
            let mut entry = self.entries.remove(pos);
            entry.timestamp = now();
            self.entries.insert(0, entry);
        } else {
            let entry = ClipboardEntry {
                id: self.next_id,
                content,
                timestamp: now(),
                pinned: false,
                tag: None,
            };
            self.next_id += 1;
            self.entries.insert(0, entry);
        }

        self.trim();
        self.dirty = true;
        true
    }

    pub fn set_tag(&mut self, index: usize, tag: Option<String>) -> bool {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.tag = tag;
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn delete(&mut self, index: usize) -> bool {
        if index < self.entries.len() {
            self.entries.remove(index);
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn pin(&mut self, index: usize) -> bool {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.pinned = true;
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn unpin(&mut self, index: usize) -> bool {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.pinned = false;
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn clear_unpinned(&mut self) {
        self.entries.retain(|e| e.pinned);
        self.dirty = true;
    }

    fn trim(&mut self) {
        let unpinned_count = self.entries.iter().filter(|e| !e.pinned).count();
        if unpinned_count <= self.config.max_history {
            return;
        }
        // Remove oldest unpinned entries (from the end of the list).
        // Walk backwards, counting unpinned entries to remove.
        let to_remove = unpinned_count - self.config.max_history;
        let mut removed = 0;
        // Mark indices to remove (from the back)
        let len = self.entries.len();
        let keep: Vec<bool> = self
            .entries
            .iter()
            .enumerate()
            .rev()
            .map(|(_, e)| {
                if e.pinned || removed >= to_remove {
                    true
                } else {
                    removed += 1;
                    false
                }
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        let mut i = 0;
        self.entries.retain(|_| {
            let k = keep[i];
            i += 1;
            k
        });
        debug_assert!(self.entries.len() <= len);
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use yoinker_common::test_config;

    fn make_history(max: usize) -> (ClipboardHistory, TempDir) {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(tmp.path());
        config.max_history = max;
        let history = ClipboardHistory::load(&config);
        (history, tmp)
    }

    fn text(s: &str) -> EntryContent {
        EntryContent::Text {
            text: s.to_string(),
        }
    }

    // --- Basic add/get ---

    #[test]
    fn add_single_entry() {
        let (mut h, _tmp) = make_history(10);
        assert!(h.add(text("hello")));
        assert_eq!(h.entries.len(), 1);
        match &h.entries[0].content {
            EntryContent::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn add_multiple_entries_newest_first() {
        let (mut h, _tmp) = make_history(10);
        h.add(text("first"));
        h.add(text("second"));
        h.add(text("third"));
        assert_eq!(h.entries.len(), 3);
        match &h.entries[0].content {
            EntryContent::Text { text } => assert_eq!(text, "third"),
            _ => panic!("expected text"),
        }
        match &h.entries[2].content {
            EntryContent::Text { text } => assert_eq!(text, "first"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn ids_are_sequential() {
        let (mut h, _tmp) = make_history(10);
        h.add(text("a"));
        h.add(text("b"));
        h.add(text("c"));
        // newest first, so c=3, b=2, a=1
        assert_eq!(h.entries[0].id, 3);
        assert_eq!(h.entries[1].id, 2);
        assert_eq!(h.entries[2].id, 1);
    }

    // --- Deduplication ---

    #[test]
    fn consecutive_duplicate_rejected() {
        let (mut h, _tmp) = make_history(10);
        assert!(h.add(text("same")));
        assert!(!h.add(text("same"))); // consecutive dup
        assert_eq!(h.entries.len(), 1);
    }

    #[test]
    fn non_consecutive_duplicate_moves_to_front() {
        let (mut h, _tmp) = make_history(10);
        h.add(text("first"));
        h.add(text("second"));
        h.add(text("first")); // should move "first" to front, not add new
        assert_eq!(h.entries.len(), 2);
        match &h.entries[0].content {
            EntryContent::Text { text } => assert_eq!(text, "first"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn duplicate_after_different_entry_is_not_consecutive() {
        let (mut h, _tmp) = make_history(10);
        h.add(text("a"));
        h.add(text("b"));
        // "a" again — not consecutive (last was "b"), so it should be moved to front
        assert!(h.add(text("a")));
        assert_eq!(h.entries.len(), 2);
        match &h.entries[0].content {
            EntryContent::Text { text } => assert_eq!(text, "a"),
            _ => panic!("expected text"),
        }
    }

    // --- Trimming ---

    #[test]
    fn trim_removes_oldest_unpinned() {
        let (mut h, _tmp) = make_history(3);
        h.add(text("1"));
        h.add(text("2"));
        h.add(text("3"));
        h.add(text("4")); // should trim "1" (oldest)
        assert_eq!(h.entries.len(), 3);
        // Should have 4, 3, 2
        let texts: Vec<_> = h
            .entries
            .iter()
            .map(|e| match &e.content {
                EntryContent::Text { text } => text.clone(),
                _ => panic!(),
            })
            .collect();
        assert_eq!(texts, vec!["4", "3", "2"]);
    }

    #[test]
    fn trim_preserves_pinned() {
        let (mut h, _tmp) = make_history(2);
        h.add(text("1"));
        h.pin(0);
        h.add(text("2"));
        h.add(text("3"));
        h.add(text("4")); // trim, but "1" is pinned
                          // Pinned "1" must survive, plus 2 newest unpinned
        let pinned_count = h.entries.iter().filter(|e| e.pinned).count();
        assert_eq!(pinned_count, 1);
        // Total: pinned "1" + 2 unpinned = 3
        assert_eq!(h.entries.len(), 3);
        assert!(h.entries.iter().any(|e| match &e.content {
            EntryContent::Text { text } => text == "1",
            _ => false,
        }));
    }

    #[test]
    fn trim_at_exact_limit() {
        let (mut h, _tmp) = make_history(3);
        h.add(text("1"));
        h.add(text("2"));
        h.add(text("3"));
        assert_eq!(h.entries.len(), 3); // exactly at limit, no trim
    }

    // --- Pinning ---

    #[test]
    fn pin_entry() {
        let (mut h, _tmp) = make_history(10);
        h.add(text("a"));
        assert!(!h.entries[0].pinned);
        assert!(h.pin(0));
        assert!(h.entries[0].pinned);
    }

    #[test]
    fn pin_out_of_range() {
        let (mut h, _tmp) = make_history(10);
        h.add(text("a"));
        assert!(!h.pin(5));
    }

    #[test]
    fn unpin_entry() {
        let (mut h, _tmp) = make_history(10);
        h.add(text("a"));
        h.pin(0);
        assert!(h.entries[0].pinned);
        assert!(h.unpin(0));
        assert!(!h.entries[0].pinned);
    }

    #[test]
    fn unpin_out_of_range() {
        let (mut h, _tmp) = make_history(10);
        assert!(!h.unpin(0));
    }

    #[test]
    fn pin_multiple_entries() {
        let (mut h, _tmp) = make_history(10);
        h.add(text("a"));
        h.add(text("b"));
        h.add(text("c"));
        h.pin(0);
        h.pin(2);
        assert!(h.entries[0].pinned);
        assert!(!h.entries[1].pinned);
        assert!(h.entries[2].pinned);
    }

    // --- Clear ---

    #[test]
    fn clear_removes_unpinned() {
        let (mut h, _tmp) = make_history(10);
        h.add(text("a"));
        h.add(text("b"));
        h.add(text("c"));
        h.pin(1); // pin "b"
        h.clear_unpinned();
        assert_eq!(h.entries.len(), 1);
        match &h.entries[0].content {
            EntryContent::Text { text } => assert_eq!(text, "b"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn clear_with_no_pinned_empties_all() {
        let (mut h, _tmp) = make_history(10);
        h.add(text("a"));
        h.add(text("b"));
        h.clear_unpinned();
        assert!(h.entries.is_empty());
    }

    #[test]
    fn clear_with_all_pinned_keeps_all() {
        let (mut h, _tmp) = make_history(10);
        h.add(text("a"));
        h.add(text("b"));
        h.pin(0);
        h.pin(1);
        h.clear_unpinned();
        assert_eq!(h.entries.len(), 2);
    }

    #[test]
    fn clear_empty_history() {
        let (mut h, _tmp) = make_history(10);
        h.clear_unpinned(); // should not panic
        assert!(h.entries.is_empty());
    }

    // --- Persistence ---

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(tmp.path());

        {
            let mut h = ClipboardHistory::load(&config);
            h.add(text("persisted1"));
            h.add(text("persisted2"));
            h.pin(0);
            h.save();
        }

        let h = ClipboardHistory::load(&config);
        assert_eq!(h.entries.len(), 2);
        assert!(h.entries[0].pinned);
        match &h.entries[0].content {
            EntryContent::Text { text } => assert_eq!(text, "persisted2"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn load_nonexistent_history_file() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(tmp.path());
        let h = ClipboardHistory::load(&config);
        assert!(h.entries.is_empty());
    }

    #[test]
    fn load_corrupted_history_file() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(tmp.path());
        std::fs::create_dir_all(config.history_path.parent().unwrap()).unwrap();
        std::fs::write(&config.history_path, "not valid json!!!").unwrap();
        let h = ClipboardHistory::load(&config);
        assert!(h.entries.is_empty()); // graceful fallback
    }

    #[test]
    fn next_id_continues_after_reload() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(tmp.path());

        {
            let mut h = ClipboardHistory::load(&config);
            h.add(text("a"));
            h.add(text("b"));
            h.save();
        }

        let mut h = ClipboardHistory::load(&config);
        h.add(text("c"));
        // id should be 3, not 1
        assert_eq!(h.entries[0].id, 3);
    }

    // --- Image entries ---

    #[test]
    fn add_image_entry() {
        let (mut h, _tmp) = make_history(10);
        let img = EntryContent::Image {
            width: 2,
            height: 2,
            bytes: vec![0, 1, 2, 3],
        };
        assert!(h.add(img));
        assert_eq!(h.entries.len(), 1);
        match &h.entries[0].content {
            EntryContent::Image {
                width,
                height,
                bytes,
            } => {
                assert_eq!(*width, 2);
                assert_eq!(*height, 2);
                assert_eq!(bytes, &vec![0, 1, 2, 3]);
            }
            _ => panic!("expected image"),
        }
    }

    #[test]
    fn duplicate_image_rejected() {
        let (mut h, _tmp) = make_history(10);
        let img1 = EntryContent::Image {
            width: 1,
            height: 1,
            bytes: vec![42],
        };
        let img2 = EntryContent::Image {
            width: 1,
            height: 1,
            bytes: vec![42],
        };
        assert!(h.add(img1));
        assert!(!h.add(img2));
        assert_eq!(h.entries.len(), 1);
    }

    #[test]
    fn mixed_text_and_image() {
        let (mut h, _tmp) = make_history(10);
        h.add(text("hello"));
        h.add(EntryContent::Image {
            width: 1,
            height: 1,
            bytes: vec![1],
        });
        h.add(text("world"));
        assert_eq!(h.entries.len(), 3);
    }

    // --- Edge cases ---

    #[test]
    fn max_history_of_one() {
        let (mut h, _tmp) = make_history(1);
        h.add(text("a"));
        h.add(text("b"));
        assert_eq!(h.entries.len(), 1);
        match &h.entries[0].content {
            EntryContent::Text { text } => assert_eq!(text, "b"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn rapid_alternating_copies() {
        let (mut h, _tmp) = make_history(10);
        for i in 0..20 {
            if i % 2 == 0 {
                h.add(text("even"));
            } else {
                h.add(text("odd"));
            }
        }
        // Should only have 2 unique entries (dedup moves to front)
        assert_eq!(h.entries.len(), 2);
    }

    #[test]
    fn pin_survives_clear_and_new_adds() {
        let (mut h, _tmp) = make_history(3);
        h.add(text("keep me"));
        h.pin(0);
        h.add(text("x1"));
        h.add(text("x2"));
        h.add(text("x3"));
        h.clear_unpinned();
        assert_eq!(h.entries.len(), 1);
        h.add(text("new1"));
        h.add(text("new2"));
        assert_eq!(h.entries.len(), 3);
        // pinned entry still exists
        assert!(h.entries.iter().any(|e| e.pinned));
    }

    #[test]
    fn timestamps_are_recent() {
        let (mut h, _tmp) = make_history(10);
        let before = now();
        h.add(text("timed"));
        let after = now();
        assert!(h.entries[0].timestamp >= before);
        assert!(h.entries[0].timestamp <= after);
    }
}
