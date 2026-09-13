#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct SessionId(pub(crate) u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct TabId(pub(crate) u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PageTarget {
    session: SessionId,
    tab: TabId,
    generation: u64,
}

impl PageTarget {
    #[cfg(test)]
    fn new(session: SessionId, tab: TabId, generation: u64) -> Self {
        Self {
            session,
            tab,
            generation,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PolicyError {
    Denied,
    NoSelection,
    SessionEnded,
    StaleTarget,
    UnknownTab,
    WrongSession,
}

pub(crate) struct BrowserPolicy {
    session: SessionId,
    tabs: Vec<TabState>,
    selected: Option<TabId>,
    grant: Option<TabId>,
    is_session_active: bool,
}

struct TabState {
    id: TabId,
    generation: u64,
}

impl BrowserPolicy {
    pub(crate) fn new(session: SessionId, tabs: impl IntoIterator<Item = TabId>) -> Self {
        Self {
            session,
            tabs: tabs
                .into_iter()
                .map(|id| TabState { id, generation: 0 })
                .collect(),
            selected: None,
            grant: None,
            is_session_active: true,
        }
    }

    pub(crate) fn select(&mut self, tab: TabId) -> Result<(), PolicyError> {
        self.tab(tab)?;
        self.selected = Some(tab);
        Ok(())
    }

    pub(crate) fn grant_selected(&mut self) -> Result<(), PolicyError> {
        self.require_active_session()?;
        let selected = self.selected.ok_or(PolicyError::NoSelection)?;
        self.tab(selected)?;
        self.grant = Some(selected);
        Ok(())
    }

    pub(crate) fn revoke(&mut self) {
        self.grant = None;
    }

    pub(crate) fn close(&mut self, tab: TabId) -> Result<(), PolicyError> {
        let index = self
            .tabs
            .iter()
            .position(|state| state.id == tab)
            .ok_or(PolicyError::UnknownTab)?;
        self.tabs.remove(index);
        if self.selected == Some(tab) {
            self.selected = None;
        }
        if self.grant == Some(tab) {
            self.grant = None;
        }
        Ok(())
    }

    pub(crate) fn end_session(&mut self) {
        self.is_session_active = false;
        self.selected = None;
        self.grant = None;
    }

    pub(crate) fn navigation_started(&mut self, tab: TabId) -> Result<PageTarget, PolicyError> {
        let session = self.session;
        let state = self.tab_mut(tab)?;
        state.generation += 1;
        Ok(PageTarget {
            session,
            tab,
            generation: state.generation,
        })
    }

    pub(crate) fn target(&self, tab: TabId) -> Result<PageTarget, PolicyError> {
        let state = self.tab(tab)?;
        Ok(PageTarget {
            session: self.session,
            tab,
            generation: state.generation,
        })
    }

    pub(crate) fn authorize(&self, target: PageTarget) -> Result<(), PolicyError> {
        self.require_active_session()?;
        if target.session != self.session {
            return Err(PolicyError::WrongSession);
        }
        let state = self.tab(target.tab)?;
        if state.generation != target.generation {
            return Err(PolicyError::StaleTarget);
        }
        if self.grant != Some(target.tab) || self.selected != Some(target.tab) {
            return Err(PolicyError::Denied);
        }
        Ok(())
    }

    fn require_active_session(&self) -> Result<(), PolicyError> {
        if self.is_session_active {
            Ok(())
        } else {
            Err(PolicyError::SessionEnded)
        }
    }

    fn tab(&self, id: TabId) -> Result<&TabState, PolicyError> {
        self.tabs
            .iter()
            .find(|state| state.id == id)
            .ok_or(PolicyError::UnknownTab)
    }

    fn tab_mut(&mut self, id: TabId) -> Result<&mut TabState, PolicyError> {
        self.tabs
            .iter_mut()
            .find(|state| state.id == id)
            .ok_or(PolicyError::UnknownTab)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: TabId = TabId(1);
    const SECOND: TabId = TabId(2);

    #[test]
    fn control_follows_the_current_selection() {
        let mut policy = BrowserPolicy::new(SessionId(7), [FIRST, SECOND]);
        policy.select(FIRST).unwrap();
        policy.grant_selected().unwrap();
        let target = policy.target(FIRST).unwrap();
        policy.select(SECOND).unwrap();
        assert_eq!(policy.authorize(target), Err(PolicyError::Denied));
        policy.select(FIRST).unwrap();
        assert_eq!(policy.authorize(target), Ok(()));
    }

    #[test]
    fn grant_requires_a_selected_known_tab() {
        let mut policy = BrowserPolicy::new(SessionId(7), [FIRST]);
        assert_eq!(policy.grant_selected(), Err(PolicyError::NoSelection));
        assert_eq!(policy.select(SECOND), Err(PolicyError::UnknownTab));
        policy.select(FIRST).unwrap();
        assert_eq!(policy.grant_selected(), Ok(()));
    }

    #[test]
    fn grant_is_bound_to_its_tab_and_session() {
        let mut policy = BrowserPolicy::new(SessionId(7), [FIRST, SECOND]);
        policy.select(FIRST).unwrap();
        policy.grant_selected().unwrap();
        let target = policy.target(FIRST).unwrap();
        assert_eq!(policy.authorize(target), Ok(()));
        assert_eq!(
            policy.authorize(policy.target(SECOND).unwrap()),
            Err(PolicyError::Denied)
        );
        assert_eq!(
            policy.authorize(PageTarget::new(SessionId(8), FIRST, 0)),
            Err(PolicyError::WrongSession)
        );
    }

    #[test]
    fn revoke_and_close_invalidate_a_grant() {
        let mut policy = BrowserPolicy::new(SessionId(7), [FIRST]);
        policy.select(FIRST).unwrap();
        policy.grant_selected().unwrap();
        policy.revoke();
        assert_eq!(
            policy.authorize(policy.target(FIRST).unwrap()),
            Err(PolicyError::Denied)
        );
        policy.grant_selected().unwrap();
        policy.close(FIRST).unwrap();
        assert_eq!(policy.target(FIRST), Err(PolicyError::UnknownTab));
    }

    #[test]
    fn session_end_invalidates_every_target() {
        let mut policy = BrowserPolicy::new(SessionId(7), [FIRST]);
        policy.select(FIRST).unwrap();
        policy.grant_selected().unwrap();
        let target = policy.target(FIRST).unwrap();
        policy.end_session();
        assert_eq!(policy.authorize(target), Err(PolicyError::SessionEnded));
    }

    #[test]
    fn navigation_preserves_grant_but_rejects_stale_targets() {
        let mut policy = BrowserPolicy::new(SessionId(7), [FIRST]);
        policy.select(FIRST).unwrap();
        policy.grant_selected().unwrap();
        let stale = policy.target(FIRST).unwrap();
        let current = policy.navigation_started(FIRST).unwrap();
        assert_eq!(policy.authorize(stale), Err(PolicyError::StaleTarget));
        assert_eq!(policy.authorize(current), Ok(()));
    }
}
