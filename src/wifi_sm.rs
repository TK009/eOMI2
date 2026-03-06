// WiFi state machine — platform-independent.
//
// Drives WiFi connection lifecycle: boot credential resolution,
// round-robin SSID scanning with exponential backoff, and
// captive portal fallback. Testable on host (no ESP deps).

/// WiFi state machine states.
#[derive(Debug, Clone, PartialEq)]
pub enum WifiState {
    /// No credentials available. Portal should be started immediately.
    Unconfigured,
    /// Attempting to connect to a specific SSID.
    Connecting { ssid_index: usize },
    /// Successfully connected to a network.
    Connected,
    /// Waiting for backoff before next rotation attempt.
    Backoff,
    /// Captive portal active (all SSIDs exhausted after max rotations).
    Portal,
}

/// Actions the state machine requests from the platform layer.
#[derive(Debug, Clone, PartialEq)]
pub enum WifiAction {
    /// Try connecting to the credential at the given index.
    TryConnect { ssid_index: usize },
    /// Start the captive portal.
    StartPortal,
    /// Stop the captive portal (auto-reconnected).
    StopPortal,
    /// Wait for the given duration (ms) before calling `backoff_complete()`.
    WaitBackoff { ms: u64 },
    /// No action needed.
    Idle,
}

/// Events fed into the state machine.
#[derive(Debug, Clone, PartialEq)]
pub enum WifiEvent {
    /// Connection attempt succeeded.
    ConnectSuccess,
    /// Connection attempt failed.
    ConnectFailed,
    /// An established connection was lost.
    ConnectionLost,
    /// Backoff wait completed — ready for next rotation.
    BackoffComplete,
    /// Background scan found a saved SSID while portal is active.
    SavedSsidFound { ssid_index: usize },
}

/// Configuration for the state machine.
#[derive(Debug, Clone)]
pub struct WifiSmConfig {
    /// Maximum full rotations through all SSIDs before portal fallback.
    pub max_rotations: u32,
    /// Initial backoff delay in ms (doubles each rotation).
    pub initial_backoff_ms: u64,
    /// Maximum backoff delay in ms.
    pub max_backoff_ms: u64,
}

impl Default for WifiSmConfig {
    fn default() -> Self {
        Self {
            max_rotations: 5,
            initial_backoff_ms: 1000,
            max_backoff_ms: 30_000,
        }
    }
}

/// Platform-independent WiFi state machine.
///
/// Given a list of credentials (build-time first, then NVS-saved),
/// drives connection attempts with round-robin and exponential backoff,
/// falling back to captive portal when all are exhausted.
pub struct WifiSm {
    state: WifiState,
    num_creds: usize,
    config: WifiSmConfig,
    rotation: u32,
}

impl WifiSm {
    /// Create a new state machine with the given number of credentials.
    ///
    /// `num_creds`: total number of SSID/password pairs available
    /// (build-time creds should be at index 0 if present).
    pub fn new(num_creds: usize, config: WifiSmConfig) -> Self {
        let state = if num_creds == 0 {
            WifiState::Unconfigured
        } else {
            WifiState::Connecting { ssid_index: 0 }
        };
        Self {
            state,
            num_creds,
            config,
            rotation: 0,
        }
    }

    /// Get the current state.
    pub fn state(&self) -> &WifiState {
        &self.state
    }

    /// Get the initial action to perform after construction.
    pub fn initial_action(&self) -> WifiAction {
        match &self.state {
            WifiState::Unconfigured => WifiAction::StartPortal,
            WifiState::Connecting { ssid_index } => WifiAction::TryConnect {
                ssid_index: *ssid_index,
            },
            _ => WifiAction::Idle,
        }
    }

    /// Feed an event into the state machine and get the next action.
    pub fn handle_event(&mut self, event: WifiEvent) -> WifiAction {
        match (&self.state, event) {
            // --- Connecting ---
            (WifiState::Connecting { .. }, WifiEvent::ConnectSuccess) => {
                self.state = WifiState::Connected;
                WifiAction::Idle
            }
            (WifiState::Connecting { ssid_index }, WifiEvent::ConnectFailed) => {
                let next_index = ssid_index + 1;
                if next_index < self.num_creds {
                    // Try next SSID in this rotation
                    self.state = WifiState::Connecting {
                        ssid_index: next_index,
                    };
                    WifiAction::TryConnect {
                        ssid_index: next_index,
                    }
                } else {
                    // Completed one full rotation — start backoff or portal
                    self.start_backoff_or_portal()
                }
            }

            // --- Connected ---
            (WifiState::Connected, WifiEvent::ConnectionLost) => {
                // Lost connection — reset rotation counter, start scanning
                self.rotation = 0;
                self.state = WifiState::Connecting { ssid_index: 0 };
                WifiAction::TryConnect { ssid_index: 0 }
            }

            // --- Backoff ---
            (WifiState::Backoff, WifiEvent::BackoffComplete) => {
                // Start a new rotation from SSID 0
                self.state = WifiState::Connecting { ssid_index: 0 };
                WifiAction::TryConnect { ssid_index: 0 }
            }

            // --- Portal ---
            (WifiState::Portal, WifiEvent::SavedSsidFound { ssid_index }) => {
                self.state = WifiState::Connecting {
                    ssid_index,
                };
                WifiAction::TryConnect {
                    ssid_index,
                }
            }

            // --- Unconfigured (portal started, now has creds from form) ---
            (WifiState::Unconfigured, WifiEvent::SavedSsidFound { ssid_index }) => {
                self.state = WifiState::Connecting {
                    ssid_index,
                };
                WifiAction::TryConnect {
                    ssid_index,
                }
            }

            // Ignore mismatched events
            _ => WifiAction::Idle,
        }
    }

    /// Notify the state machine that new credentials were added (e.g., from portal).
    /// Restarts the connection sequence from the given index.
    pub fn credentials_updated(&mut self, num_creds: usize, start_index: usize) -> WifiAction {
        self.num_creds = num_creds;
        self.rotation = 0;
        if num_creds == 0 {
            self.state = WifiState::Unconfigured;
            return WifiAction::StartPortal;
        }
        let idx = start_index.min(num_creds - 1);
        self.state = WifiState::Connecting { ssid_index: idx };
        WifiAction::TryConnect { ssid_index: idx }
    }

    /// Stop the portal after a successful connection from background scan.
    /// Call this after ConnectSuccess when state was previously Portal.
    pub fn portal_connect_succeeded(&mut self) -> WifiAction {
        self.state = WifiState::Connected;
        WifiAction::StopPortal
    }

    fn start_backoff_or_portal(&mut self) -> WifiAction {
        self.rotation += 1;
        if self.rotation >= self.config.max_rotations {
            self.state = WifiState::Portal;
            WifiAction::StartPortal
        } else {
            let backoff_ms = self.backoff_ms(self.rotation);
            self.state = WifiState::Backoff;
            WifiAction::WaitBackoff { ms: backoff_ms }
        }
    }

    fn backoff_ms(&self, rotation: u32) -> u64 {
        let ms = self
            .config
            .initial_backoff_ms
            .saturating_mul(1u64 << rotation.min(16));
        ms.min(self.config.max_backoff_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> WifiSmConfig {
        WifiSmConfig {
            max_rotations: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 5000,
        }
    }

    #[test]
    fn unconfigured_starts_portal() {
        let sm = WifiSm::new(0, test_config());
        assert_eq!(*sm.state(), WifiState::Unconfigured);
        assert_eq!(sm.initial_action(), WifiAction::StartPortal);
    }

    #[test]
    fn single_cred_connects() {
        let sm = WifiSm::new(1, test_config());
        assert_eq!(*sm.state(), WifiState::Connecting { ssid_index: 0 });
        assert_eq!(
            sm.initial_action(),
            WifiAction::TryConnect { ssid_index: 0 }
        );
    }

    #[test]
    fn connect_success_transitions_to_connected() {
        let mut sm = WifiSm::new(1, test_config());
        let action = sm.handle_event(WifiEvent::ConnectSuccess);
        assert_eq!(*sm.state(), WifiState::Connected);
        assert_eq!(action, WifiAction::Idle);
    }

    #[test]
    fn connect_fail_tries_next_ssid() {
        let mut sm = WifiSm::new(3, test_config());
        let action = sm.handle_event(WifiEvent::ConnectFailed);
        assert_eq!(*sm.state(), WifiState::Connecting { ssid_index: 1 });
        assert_eq!(
            action,
            WifiAction::TryConnect { ssid_index: 1 }
        );
    }

    #[test]
    fn full_rotation_triggers_backoff() {
        let mut sm = WifiSm::new(2, test_config());
        sm.handle_event(WifiEvent::ConnectFailed); // ssid 0 fails
        let action = sm.handle_event(WifiEvent::ConnectFailed); // ssid 1 fails
        assert!(matches!(*sm.state(), WifiState::Backoff));
        assert!(matches!(action, WifiAction::WaitBackoff { .. }));
    }

    #[test]
    fn backoff_complete_restarts_rotation() {
        let mut sm = WifiSm::new(2, test_config());
        sm.handle_event(WifiEvent::ConnectFailed);
        sm.handle_event(WifiEvent::ConnectFailed);
        let action = sm.handle_event(WifiEvent::BackoffComplete);
        assert_eq!(*sm.state(), WifiState::Connecting { ssid_index: 0 });
        assert_eq!(
            action,
            WifiAction::TryConnect { ssid_index: 0 }
        );
    }

    #[test]
    fn max_rotations_triggers_portal() {
        let mut sm = WifiSm::new(1, test_config()); // max_rotations = 3
        // Rotation 0: fail
        sm.handle_event(WifiEvent::ConnectFailed);
        // Rotation 1: backoff complete, fail
        sm.handle_event(WifiEvent::BackoffComplete);
        sm.handle_event(WifiEvent::ConnectFailed);
        // Rotation 2: backoff complete, fail
        sm.handle_event(WifiEvent::BackoffComplete);
        let action = sm.handle_event(WifiEvent::ConnectFailed);
        assert_eq!(*sm.state(), WifiState::Portal);
        assert_eq!(action, WifiAction::StartPortal);
    }

    #[test]
    fn connection_lost_restarts_scanning() {
        let mut sm = WifiSm::new(2, test_config());
        sm.handle_event(WifiEvent::ConnectSuccess);
        let action = sm.handle_event(WifiEvent::ConnectionLost);
        assert_eq!(*sm.state(), WifiState::Connecting { ssid_index: 0 });
        assert_eq!(
            action,
            WifiAction::TryConnect { ssid_index: 0 }
        );
    }

    #[test]
    fn portal_saved_ssid_found_reconnects() {
        let mut sm = WifiSm::new(1, test_config());
        // Exhaust all rotations to reach portal
        sm.handle_event(WifiEvent::ConnectFailed);
        sm.handle_event(WifiEvent::BackoffComplete);
        sm.handle_event(WifiEvent::ConnectFailed);
        sm.handle_event(WifiEvent::BackoffComplete);
        sm.handle_event(WifiEvent::ConnectFailed);
        assert_eq!(*sm.state(), WifiState::Portal);

        let action = sm.handle_event(WifiEvent::SavedSsidFound { ssid_index: 0 });
        assert_eq!(*sm.state(), WifiState::Connecting { ssid_index: 0 });
        assert_eq!(
            action,
            WifiAction::TryConnect { ssid_index: 0 }
        );
    }

    #[test]
    fn portal_connect_succeeded_stops_portal() {
        let mut sm = WifiSm::new(1, test_config());
        sm.handle_event(WifiEvent::ConnectFailed);
        sm.handle_event(WifiEvent::BackoffComplete);
        sm.handle_event(WifiEvent::ConnectFailed);
        sm.handle_event(WifiEvent::BackoffComplete);
        sm.handle_event(WifiEvent::ConnectFailed);
        sm.handle_event(WifiEvent::SavedSsidFound { ssid_index: 0 });
        let action = sm.portal_connect_succeeded();
        assert_eq!(*sm.state(), WifiState::Connected);
        assert_eq!(action, WifiAction::StopPortal);
    }

    #[test]
    fn credentials_updated_restarts() {
        let mut sm = WifiSm::new(0, test_config());
        assert_eq!(*sm.state(), WifiState::Unconfigured);

        let action = sm.credentials_updated(2, 0);
        assert_eq!(*sm.state(), WifiState::Connecting { ssid_index: 0 });
        assert_eq!(
            action,
            WifiAction::TryConnect { ssid_index: 0 }
        );
    }

    #[test]
    fn backoff_increases_exponentially() {
        let sm = WifiSm::new(1, test_config());
        assert_eq!(sm.backoff_ms(0), 100);
        assert_eq!(sm.backoff_ms(1), 200);
        assert_eq!(sm.backoff_ms(2), 400);
        // Capped at max
        assert_eq!(sm.backoff_ms(10), 5000);
    }

    #[test]
    fn backoff_caps_at_max() {
        let config = WifiSmConfig {
            max_rotations: 20,
            initial_backoff_ms: 1000,
            max_backoff_ms: 30_000,
        };
        let sm = WifiSm::new(1, config);
        assert_eq!(sm.backoff_ms(0), 1000);
        assert_eq!(sm.backoff_ms(5), 30_000); // 1000 * 32 = 32000, capped to 30000
    }

    #[test]
    fn ignores_mismatched_events() {
        let mut sm = WifiSm::new(1, test_config());
        // Connected state ignoring BackoffComplete
        sm.handle_event(WifiEvent::ConnectSuccess);
        let action = sm.handle_event(WifiEvent::BackoffComplete);
        assert_eq!(*sm.state(), WifiState::Connected);
        assert_eq!(action, WifiAction::Idle);
    }

    #[test]
    fn multiple_ssids_full_cycle() {
        let mut sm = WifiSm::new(3, test_config());

        // First rotation: try all 3 SSIDs
        assert_eq!(
            sm.initial_action(),
            WifiAction::TryConnect { ssid_index: 0 }
        );
        sm.handle_event(WifiEvent::ConnectFailed);
        assert_eq!(*sm.state(), WifiState::Connecting { ssid_index: 1 });
        sm.handle_event(WifiEvent::ConnectFailed);
        assert_eq!(*sm.state(), WifiState::Connecting { ssid_index: 2 });
        let action = sm.handle_event(WifiEvent::ConnectFailed);
        // After full rotation, backoff
        assert!(matches!(*sm.state(), WifiState::Backoff));
        assert!(matches!(action, WifiAction::WaitBackoff { .. }));

        // Second rotation: succeed on SSID 1
        sm.handle_event(WifiEvent::BackoffComplete);
        sm.handle_event(WifiEvent::ConnectFailed); // ssid 0
        let action = sm.handle_event(WifiEvent::ConnectSuccess); // ssid 1
        assert_eq!(*sm.state(), WifiState::Connected);
        assert_eq!(action, WifiAction::Idle);
    }
}
