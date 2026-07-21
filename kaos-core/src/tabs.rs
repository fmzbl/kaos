//! Tabs, independent of what is in them and of what draws them.
//!
//! Generic over the content so the terminal app and the visual editor share
//! one implementation: a tab may hold a mandala, a workspace, or a
//! conversation, and the rules — which one is active, what happens when you
//! close it, how you cycle — are written once here and tested without a
//! screen.

use std::fmt;

/// Stable handle for a tab. Ids are never reused, so a handle held across
/// edits either resolves or is plainly gone, never silently retargeted.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct TabId(pub u32);

/// One tab: a title and whatever it holds.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Tab<T> {
    pub id: TabId,
    pub title: String,
    pub content: T,
}

/// An ordered set of tabs with exactly one active, unless it is empty.
#[derive(Clone, Debug, Default)]
pub struct Tabs<T> {
    items: Vec<Tab<T>>,
    active: usize,
    next_id: u32,
}

impl<T> Tabs<T> {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            active: 0,
            next_id: 0,
        }
    }

    /// Open a tab and make it active — opening something you cannot see would
    /// never be what was meant.
    pub fn open(&mut self, title: impl Into<String>, content: T) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        self.items.push(Tab {
            id,
            title: title.into(),
            content,
        });
        self.active = self.items.len() - 1;
        id
    }

    /// Close a tab.
    ///
    /// Closing the active tab moves to the one that took its place, or to the
    /// new last tab when it was the rightmost — the same thing every editor
    /// does, so the eye does not have to search.
    pub fn close(&mut self, id: TabId) -> Option<T> {
        let at = self.items.iter().position(|t| t.id == id)?;
        let removed = self.items.remove(at);
        if self.items.is_empty() {
            self.active = 0;
        } else if at < self.active || self.active >= self.items.len() {
            self.active = self.active.saturating_sub(1).min(self.items.len() - 1);
        }
        Some(removed.content)
    }

    pub fn select(&mut self, id: TabId) -> bool {
        match self.items.iter().position(|t| t.id == id) {
            Some(at) => {
                self.active = at;
                true
            }
            None => false,
        }
    }

    /// Move to the next tab, wrapping. No-op when empty.
    pub fn next(&mut self) {
        if !self.items.is_empty() {
            self.active = (self.active + 1) % self.items.len();
        }
    }

    /// Move to the previous tab, wrapping.
    pub fn prev(&mut self) {
        if !self.items.is_empty() {
            self.active = (self.active + self.items.len() - 1) % self.items.len();
        }
    }

    pub fn rename(&mut self, id: TabId, title: impl Into<String>) {
        if let Some(t) = self.items.iter_mut().find(|t| t.id == id) {
            t.title = title.into();
        }
    }

    pub fn active_id(&self) -> Option<TabId> {
        self.items.get(self.active).map(|t| t.id)
    }

    pub fn active(&self) -> Option<&T> {
        self.items.get(self.active).map(|t| &t.content)
    }

    pub fn active_mut(&mut self) -> Option<&mut T> {
        self.items.get_mut(self.active).map(|t| &mut t.content)
    }

    pub fn get(&self, id: TabId) -> Option<&T> {
        self.items.iter().find(|t| t.id == id).map(|t| &t.content)
    }

    pub fn get_mut(&mut self, id: TabId) -> Option<&mut T> {
        self.items
            .iter_mut()
            .find(|t| t.id == id)
            .map(|t| &mut t.content)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tab<T>> {
        self.items.iter()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Index of the active tab, for a front-end drawing the bar.
    pub fn active_index(&self) -> usize {
        self.active
    }
}

impl<T> fmt::Display for Tabs<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, t) in self.items.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            if i == self.active {
                write!(f, "[{}]", t.title)?;
            } else {
                write!(f, " {} ", t.title)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three() -> Tabs<&'static str> {
        let mut t = Tabs::new();
        t.open("a", "A");
        t.open("b", "B");
        t.open("c", "C");
        t
    }

    #[test]
    fn a_new_set_is_empty_and_has_no_active_tab() {
        let t: Tabs<()> = Tabs::new();
        assert!(t.is_empty());
        assert_eq!(t.active_id(), None);
        assert!(t.active().is_none());
    }

    #[test]
    fn opening_activates_what_was_opened() {
        let t = three();
        assert_eq!(t.len(), 3);
        assert_eq!(t.active(), Some(&"C"));
    }

    #[test]
    fn closing_the_active_tab_moves_to_its_neighbour() {
        let mut t = three();
        let b = t.iter().nth(1).unwrap().id;
        t.select(b);
        assert_eq!(t.close(b), Some("B"));
        // The tab that slid into b's place is now active.
        assert_eq!(t.active(), Some(&"C"));
    }

    #[test]
    fn closing_the_last_tab_falls_back_to_the_new_last() {
        let mut t = three();
        let c = t.active_id().unwrap();
        t.close(c);
        assert_eq!(t.active(), Some(&"B"));
    }

    #[test]
    fn closing_before_the_active_one_keeps_the_same_tab_active() {
        let mut t = three();
        let a = t.iter().next().unwrap().id;
        assert_eq!(t.active(), Some(&"C"));
        t.close(a);
        assert_eq!(t.active(), Some(&"C"), "the active tab must not shift");
    }

    #[test]
    fn closing_everything_leaves_nothing_active() {
        let mut t = three();
        let ids: Vec<TabId> = t.iter().map(|x| x.id).collect();
        for id in ids {
            t.close(id);
        }
        assert!(t.is_empty());
        assert_eq!(t.active_id(), None);
    }

    #[test]
    fn closing_an_unknown_tab_changes_nothing() {
        let mut t = three();
        assert_eq!(t.close(TabId(999)), None);
        assert_eq!(t.len(), 3);
        assert_eq!(t.active(), Some(&"C"));
    }

    #[test]
    fn cycling_wraps_both_ways() {
        let mut t = three();
        t.next();
        assert_eq!(t.active(), Some(&"A"), "next from the last wraps to first");
        t.prev();
        assert_eq!(t.active(), Some(&"C"), "prev from the first wraps to last");
    }

    #[test]
    fn cycling_an_empty_set_does_not_panic() {
        let mut t: Tabs<()> = Tabs::new();
        t.next();
        t.prev();
        assert!(t.is_empty());
    }

    #[test]
    fn ids_are_not_reused_after_closing() {
        let mut t = three();
        let a = t.iter().next().unwrap().id;
        t.close(a);
        let fresh = t.open("d", "D");
        assert_ne!(fresh, a);
        assert!(t.get(a).is_none());
    }

    #[test]
    fn selecting_an_unknown_id_is_refused() {
        let mut t = three();
        assert!(!t.select(TabId(999)));
        assert_eq!(t.active(), Some(&"C"));
    }

    #[test]
    fn content_is_reachable_and_mutable_by_id() {
        let mut t: Tabs<String> = Tabs::new();
        let id = t.open("one", "first".to_string());
        t.get_mut(id).unwrap().push_str(" edited");
        assert_eq!(t.get(id).unwrap(), "first edited");
        t.active_mut().unwrap().push('!');
        assert_eq!(t.get(id).unwrap(), "first edited!");
    }

    #[test]
    fn renaming_shows_in_the_bar() {
        let mut t = three();
        let a = t.iter().next().unwrap().id;
        t.rename(a, "renamed");
        assert_eq!(t.to_string(), " renamed   b  [c]");
    }
}
