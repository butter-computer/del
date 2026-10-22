//! The set of open tabs in the top bar.

/// Title given to every new tab until it has a conversation to name it after.
pub const NEW_TAB_TITLE: &str = "New session";

/// One open tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    /// Stable identifier; never reused within a [`Tabs`] value.
    pub id: u64,
    pub title: String,
}

/// An ordered list of tabs with exactly one active tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tabs {
    tabs: Vec<Tab>,
    active: u64,
    next_id: u64,
}

impl Default for Tabs {
    fn default() -> Self {
        Self::new()
    }
}

impl Tabs {
    /// A tab list holding one new, active tab.
    pub fn new() -> Self {
        Self {
            tabs: vec![Tab {
                id: 0,
                title: NEW_TAB_TITLE.to_string(),
            }],
            active: 0,
            next_id: 1,
        }
    }

    /// Open a new tab at the end and make it active. Returns its id.
    pub fn add(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab {
            id,
            title: NEW_TAB_TITLE.to_string(),
        });
        self.active = id;
        id
    }

    /// Make `id` the active tab. Unknown ids are ignored.
    pub fn select(&mut self, id: u64) {
        if self.tabs.iter().any(|tab| tab.id == id) {
            self.active = id;
        }
    }

    /// The tabs in display order.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Id of the active tab.
    pub fn active_id(&self) -> u64 {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_one_active_tab() {
        let tabs = Tabs::new();
        assert_eq!(tabs.tabs().len(), 1);
        assert_eq!(tabs.active_id(), tabs.tabs()[0].id);
    }

    #[test]
    fn add_appends_and_activates_with_unique_ids() {
        let mut tabs = Tabs::new();
        let a = tabs.add();
        let b = tabs.add();
        assert_ne!(a, b);
        assert_eq!(tabs.tabs().len(), 3);
        assert_eq!(tabs.active_id(), b);
    }

    #[test]
    fn select_switches_and_ignores_unknown_ids() {
        let mut tabs = Tabs::new();
        let first = tabs.active_id();
        tabs.add();
        tabs.select(first);
        assert_eq!(tabs.active_id(), first);
        tabs.select(999);
        assert_eq!(tabs.active_id(), first);
    }
}
