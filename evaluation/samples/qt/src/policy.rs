use crate::ffi::PageTarget;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SessionId(pub(crate) u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct TabId(pub(crate) u64);

#[derive(Clone, Copy)]
struct TabState {
    navigation_generation: u64,
    is_loaded: bool,
}

#[derive(Clone, Copy)]
struct ControlGrant {
    session_id: SessionId,
    tab_id: TabId,
}

pub(crate) struct BrowserPolicy {
    session_id: SessionId,
    is_session_active: bool,
    tabs: Vec<(TabId, TabState)>,
    selected_tab: Option<TabId>,
    grant: Option<ControlGrant>,
}

impl BrowserPolicy {
    pub(crate) fn new(session_id: SessionId, tab_ids: impl IntoIterator<Item = TabId>) -> Self {
        Self {
            session_id,
            is_session_active: true,
            tabs: tab_ids
                .into_iter()
                .map(|tab_id| {
                    (
                        tab_id,
                        TabState {
                            navigation_generation: 0,
                            is_loaded: true,
                        },
                    )
                })
                .collect(),
            selected_tab: None,
            grant: None,
        }
    }

    pub(crate) fn is_tab_selected(&mut self, tab_id: TabId) -> bool {
        if self.tab(tab_id).is_none() {
            return false;
        }
        self.selected_tab = Some(tab_id);
        true
    }

    pub(crate) fn is_selected_tab_granted(&mut self) -> bool {
        let Some(tab_id) = self.selected_tab else {
            return false;
        };
        self.grant = Some(ControlGrant {
            session_id: self.session_id,
            tab_id,
        });
        true
    }

    pub(crate) fn revoke(&mut self, tab_id: TabId) {
        if self.grant.is_some_and(|grant| grant.tab_id == tab_id) {
            self.grant = None;
        }
    }

    pub(crate) fn is_navigation_started(&mut self, tab_id: TabId) -> bool {
        let Some(tab) = self.tab_mut(tab_id) else {
            return false;
        };
        tab.navigation_generation += 1;
        tab.is_loaded = true;
        true
    }

    pub(crate) fn is_discarded(&mut self, tab_id: TabId) -> bool {
        let Some(tab) = self.tab_mut(tab_id) else {
            return false;
        };
        tab.is_loaded = false;
        true
    }

    pub(crate) fn is_reload_started(&mut self, tab_id: TabId) -> bool {
        self.is_navigation_started(tab_id)
    }

    pub(crate) fn is_tab_closed(&mut self, tab_id: TabId) -> bool {
        let Some(index) = self.tabs.iter().position(|(id, _)| *id == tab_id) else {
            return false;
        };
        self.tabs.remove(index);
        if self.selected_tab == Some(tab_id) {
            self.selected_tab = None;
        }
        self.revoke(tab_id);
        true
    }

    pub(crate) fn end_session(&mut self) {
        self.is_session_active = false;
        self.grant = None;
    }

    pub(crate) fn target_for(&self, tab_id: TabId) -> Option<PageTarget> {
        self.tab(tab_id).map(|tab| PageTarget {
            session_id: self.session_id.0,
            tab_id: tab_id.0,
            navigation_generation: tab.navigation_generation,
        })
    }

    pub(crate) fn is_control_allowed(&self, target: &PageTarget) -> bool {
        let tab_id = TabId(target.tab_id);
        self.is_session_active
            && target.session_id == self.session_id.0
            && self.selected_tab == Some(tab_id)
            && self
                .grant
                .is_some_and(|grant| grant.session_id == self.session_id && grant.tab_id == tab_id)
            && self.tab(tab_id).is_some_and(|tab| {
                tab.is_loaded && tab.navigation_generation == target.navigation_generation
            })
    }

    pub(crate) fn is_tab_granted(&self, tab_id: TabId) -> bool {
        self.grant.is_some_and(|grant| grant.tab_id == tab_id)
    }

    fn tab(&self, tab_id: TabId) -> Option<&TabState> {
        self.tabs
            .iter()
            .find_map(|(id, tab)| (*id == tab_id).then_some(tab))
    }

    fn tab_mut(&mut self, tab_id: TabId) -> Option<&mut TabState> {
        self.tabs
            .iter_mut()
            .find_map(|(id, tab)| (*id == tab_id).then_some(tab))
    }
}

#[cfg(test)]
mod tests {
    use super::{BrowserPolicy, SessionId, TabId};
    use crate::ffi::PageTarget;

    #[test]
    fn navigation_rejects_a_stale_target_without_revoking_the_tab_grant() {
        let mut policy = BrowserPolicy::new(SessionId(1), [TabId(1), TabId(2)]);
        assert!(policy.is_tab_selected(TabId(1)));
        assert!(policy.is_selected_tab_granted());
        let target = policy
            .target_for(TabId(1))
            .expect("selected tab has a target");

        assert!(policy.is_control_allowed(&target));
        assert!(policy.is_navigation_started(TabId(1)));
        assert!(!policy.is_control_allowed(&target));
        assert!(policy.is_tab_granted(TabId(1)));
    }

    #[test]
    fn controls_require_the_selected_granted_loaded_tab_in_the_active_session() {
        let mut policy = BrowserPolicy::new(SessionId(1), [TabId(1), TabId(2)]);
        assert!(policy.is_tab_selected(TabId(1)));
        assert!(policy.is_selected_tab_granted());
        let first_target = policy.target_for(TabId(1)).expect("first tab has a target");

        assert!(policy.is_tab_selected(TabId(2)));
        assert!(!policy.is_control_allowed(&first_target));
        assert!(policy.is_tab_selected(TabId(1)));
        assert!(policy.is_discarded(TabId(1)));
        assert!(!policy.is_control_allowed(&first_target));
        assert!(policy.is_reload_started(TabId(1)));
        let reloaded_target = policy
            .target_for(TabId(1))
            .expect("reloaded tab has a target");
        assert!(policy.is_control_allowed(&reloaded_target));

        policy.end_session();
        assert!(!policy.is_control_allowed(&reloaded_target));
    }

    #[test]
    fn revocation_and_tab_close_remove_a_grant_immediately() {
        let mut policy = BrowserPolicy::new(SessionId(1), [TabId(1), TabId(2)]);
        assert!(policy.is_tab_selected(TabId(1)));
        assert!(policy.is_selected_tab_granted());
        let revocable_target = policy
            .target_for(TabId(1))
            .expect("granted tab has a target");
        assert!(policy.is_control_allowed(&revocable_target));
        policy.revoke(TabId(1));
        assert!(!policy.is_tab_granted(TabId(1)));
        assert!(!policy.is_control_allowed(&revocable_target));

        assert!(policy.is_selected_tab_granted());
        let closable_target = policy
            .target_for(TabId(1))
            .expect("granted tab has a target");
        assert!(policy.is_tab_closed(TabId(1)));
        assert!(!policy.is_tab_granted(TabId(1)));
        assert!(!policy.is_control_allowed(&closable_target));
        assert!(policy.target_for(TabId(1)).is_none());
    }

    #[test]
    fn target_identity_is_bound_to_its_tab_and_navigation_generation() {
        let mut policy = BrowserPolicy::new(SessionId(1), [TabId(1), TabId(2)]);
        assert!(policy.is_tab_selected(TabId(1)));
        assert!(policy.is_selected_tab_granted());

        assert!(!policy.is_control_allowed(&PageTarget {
            session_id: 1,
            tab_id: 2,
            navigation_generation: 0,
        }));
        assert!(!policy.is_control_allowed(&PageTarget {
            session_id: 1,
            tab_id: 1,
            navigation_generation: 1,
        }));
    }

    #[test]
    fn policies_with_distinct_sessions_do_not_share_grants() {
        let mut first = BrowserPolicy::new(SessionId(101), [TabId(1)]);
        let mut second = BrowserPolicy::new(SessionId(102), [TabId(1)]);
        assert!(first.is_tab_selected(TabId(1)));
        assert!(first.is_selected_tab_granted());
        assert!(second.is_tab_selected(TabId(1)));
        assert!(second.is_selected_tab_granted());
        let target = first.target_for(TabId(1)).expect("first target exists");

        assert!(first.is_control_allowed(&target));
        assert!(!second.is_control_allowed(&target));
        first.revoke(TabId(1));
        assert!(!first.is_control_allowed(&target));
        first.end_session();
        assert!(!first.is_control_allowed(&target));
    }
}
