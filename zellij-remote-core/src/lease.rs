use std::collections::HashSet;
use zellij_remote_protocol::{ControllerLease, ControllerPolicy, DisplaySize};

#[cfg(not(test))]
use std::time::{Duration, Instant};

#[cfg(test)]
pub use test_time::{Duration, Instant, TestClock};

#[cfg(test)]
mod test_time {
    use std::cell::RefCell;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub struct Instant(u64);

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Duration(u64);

    thread_local! {
        static CURRENT_TIME: RefCell<u64> = const { RefCell::new(0) };
    }

    impl Instant {
        pub fn now() -> Self {
            CURRENT_TIME.with(|t| Instant(*t.borrow()))
        }

        pub fn elapsed(&self) -> Duration {
            let now = Self::now();
            Duration(now.0.saturating_sub(self.0))
        }

        pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
            Duration(self.0.saturating_sub(earlier.0))
        }
    }

    impl Duration {
        pub const fn from_millis(millis: u64) -> Self {
            Duration(millis)
        }

        pub const fn from_secs(secs: u64) -> Self {
            Duration(secs * 1000)
        }

        pub fn as_millis(&self) -> u128 {
            self.0 as u128
        }

        pub fn saturating_sub(self, rhs: Duration) -> Duration {
            Duration(self.0.saturating_sub(rhs.0))
        }
    }

    impl std::ops::Add<Duration> for Instant {
        type Output = Instant;
        fn add(self, rhs: Duration) -> Instant {
            Instant(self.0 + rhs.0)
        }
    }

    impl PartialOrd<Duration> for Duration {
        fn partial_cmp(&self, other: &Duration) -> Option<std::cmp::Ordering> {
            Some(self.0.cmp(&other.0))
        }
    }

    impl Ord for Duration {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.0.cmp(&other.0)
        }
    }

    pub struct TestClock;

    impl TestClock {
        pub fn reset() {
            CURRENT_TIME.with(|t| *t.borrow_mut() = 0);
        }

        pub fn advance(duration: Duration) {
            CURRENT_TIME.with(|t| *t.borrow_mut() += duration.0);
        }

        pub fn set(millis: u64) {
            CURRENT_TIME.with(|t| *t.borrow_mut() = millis);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LeaseState {
    NoController,
    Active {
        owner_client_id: u64,
        lease_id: u64,
        granted_at: Instant,
        duration: Duration,
        current_size: DisplaySize,
    },
    Expired {
        previous_owner: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LeaseResult {
    Granted(ControllerLease),
    Denied {
        reason: String,
        current_lease: Option<ControllerLease>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LeaseEvent {
    Expired {
        lease_id: u64,
        owner: u64,
    },
    Revoked {
        lease_id: u64,
        owner: u64,
        reason: String,
    },
}

pub struct LeaseManager {
    state: LeaseState,
    policy: ControllerPolicy,
    next_lease_id: u64,
    default_duration: Duration,
    viewers: HashSet<u64>,
}

impl LeaseManager {
    pub fn new(policy: ControllerPolicy, duration: Duration) -> Self {
        Self {
            state: LeaseState::NoController,
            policy,
            next_lease_id: 1,
            default_duration: duration,
            viewers: HashSet::new(),
        }
    }

    pub fn request_control(
        &mut self,
        client_id: u64,
        desired_size: Option<DisplaySize>,
        force: bool,
    ) -> LeaseResult {
        let size = desired_size.unwrap_or(DisplaySize { cols: 80, rows: 24 });

        match &self.state {
            LeaseState::NoController | LeaseState::Expired { .. } => {
                let lease_id = self.next_lease_id;
                self.next_lease_id += 1;
                let now = Instant::now();

                self.state = LeaseState::Active {
                    owner_client_id: client_id,
                    lease_id,
                    granted_at: now,
                    duration: self.default_duration,
                    current_size: size.clone(),
                };

                self.viewers.remove(&client_id);

                LeaseResult::Granted(self.build_lease(
                    lease_id,
                    client_id,
                    &size,
                    self.default_duration,
                ))
            },
            LeaseState::Active {
                owner_client_id,
                lease_id,
                granted_at,
                duration,
                current_size,
            } => {
                if *owner_client_id == client_id {
                    return LeaseResult::Granted(self.build_lease(
                        *lease_id,
                        client_id,
                        current_size,
                        duration.saturating_sub(granted_at.elapsed()),
                    ));
                }

                let can_takeover = match self.policy {
                    ControllerPolicy::LastWriterWins => true,
                    ControllerPolicy::ExplicitOnly => force,
                    ControllerPolicy::Unspecified => force,
                };

                if can_takeover {
                    let new_lease_id = self.next_lease_id;
                    self.next_lease_id += 1;
                    let now = Instant::now();

                    self.viewers.insert(*owner_client_id);

                    self.state = LeaseState::Active {
                        owner_client_id: client_id,
                        lease_id: new_lease_id,
                        granted_at: now,
                        duration: self.default_duration,
                        current_size: size.clone(),
                    };

                    self.viewers.remove(&client_id);

                    LeaseResult::Granted(self.build_lease(
                        new_lease_id,
                        client_id,
                        &size,
                        self.default_duration,
                    ))
                } else {
                    LeaseResult::Denied {
                        reason: format!(
                            "Lease held by client {} (policy: {:?})",
                            owner_client_id, self.policy
                        ),
                        current_lease: Some(self.build_lease(
                            *lease_id,
                            *owner_client_id,
                            current_size,
                            duration.saturating_sub(granted_at.elapsed()),
                        )),
                    }
                }
            },
        }
    }

    pub fn release_control(&mut self, client_id: u64, lease_id: u64) -> bool {
        if let LeaseState::Active {
            owner_client_id,
            lease_id: current_lease_id,
            ..
        } = &self.state
        {
            if *owner_client_id == client_id && *current_lease_id == lease_id {
                self.state = LeaseState::Expired {
                    previous_owner: client_id,
                };
                return true;
            }
        }
        false
    }

    pub fn keepalive(&mut self, client_id: u64, lease_id: u64) -> bool {
        if let LeaseState::Active {
            owner_client_id,
            lease_id: current_lease_id,
            granted_at: _,
            duration,
            current_size,
        } = &self.state
        {
            if *owner_client_id == client_id && *current_lease_id == lease_id {
                self.state = LeaseState::Active {
                    owner_client_id: *owner_client_id,
                    lease_id: *current_lease_id,
                    granted_at: Instant::now(),
                    duration: *duration,
                    current_size: current_size.clone(),
                };
                return true;
            }
        }
        false
    }

    pub fn tick(&mut self) -> Option<LeaseEvent> {
        if let LeaseState::Active {
            owner_client_id,
            lease_id,
            granted_at,
            duration,
            ..
        } = &self.state
        {
            if granted_at.elapsed() >= *duration {
                let event = LeaseEvent::Expired {
                    lease_id: *lease_id,
                    owner: *owner_client_id,
                };
                self.state = LeaseState::Expired {
                    previous_owner: *owner_client_id,
                };
                return Some(event);
            }
        }
        None
    }

    pub fn current_size(&self) -> Option<DisplaySize> {
        if let LeaseState::Active { current_size, .. } = &self.state {
            Some(current_size.clone())
        } else {
            None
        }
    }

    pub fn set_size(&mut self, client_id: u64, lease_id: u64, size: DisplaySize) -> bool {
        if let LeaseState::Active {
            owner_client_id,
            lease_id: current_lease_id,
            granted_at,
            duration,
            ..
        } = &self.state
        {
            if *owner_client_id == client_id && *current_lease_id == lease_id {
                self.state = LeaseState::Active {
                    owner_client_id: *owner_client_id,
                    lease_id: *current_lease_id,
                    granted_at: *granted_at,
                    duration: *duration,
                    current_size: size,
                };
                return true;
            }
        }
        false
    }

    pub fn is_controller(&self, client_id: u64) -> bool {
        if let LeaseState::Active {
            owner_client_id, ..
        } = &self.state
        {
            *owner_client_id == client_id
        } else {
            false
        }
    }

    pub fn get_current_lease(&self) -> Option<ControllerLease> {
        if let LeaseState::Active {
            owner_client_id,
            lease_id,
            granted_at,
            duration,
            current_size,
        } = &self.state
        {
            let remaining = duration.saturating_sub(granted_at.elapsed());
            Some(self.build_lease(*lease_id, *owner_client_id, current_size, remaining))
        } else {
            None
        }
    }

    pub fn add_viewer(&mut self, client_id: u64) {
        if !self.is_controller(client_id) {
            self.viewers.insert(client_id);
        }
    }

    pub fn remove_client(&mut self, client_id: u64) -> Option<LeaseEvent> {
        self.viewers.remove(&client_id);

        if let LeaseState::Active {
            owner_client_id,
            lease_id,
            ..
        } = &self.state
        {
            if *owner_client_id == client_id {
                let event = LeaseEvent::Revoked {
                    lease_id: *lease_id,
                    owner: *owner_client_id,
                    reason: "disconnect".to_string(),
                };
                self.state = LeaseState::Expired {
                    previous_owner: client_id,
                };
                return Some(event);
            }
        }
        None
    }

    pub fn is_viewer(&self, client_id: u64) -> bool {
        self.viewers.contains(&client_id)
    }

    pub fn viewer_count(&self) -> usize {
        self.viewers.len()
    }

    fn build_lease(
        &self,
        lease_id: u64,
        owner_client_id: u64,
        size: &DisplaySize,
        remaining: Duration,
    ) -> ControllerLease {
        ControllerLease {
            lease_id,
            owner_client_id,
            policy: self.policy.into(),
            current_size: Some(size.clone()),
            remaining_ms: remaining.as_millis() as u32,
            duration_ms: self.default_duration.as_millis() as u32,
        }
    }
}
