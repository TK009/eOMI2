// WSOP joiner state machine — platform-independent, host-testable.
//
// Drives the onboarding lifecycle from the device (joiner) perspective:
// Scanning → Connecting → Requesting → Polling → Decrypting → Succeeded/Failed.
//
// Event-driven like WifiSm: platform layer feeds events, SM returns actions.

/// Joiner state machine states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardState {
    /// Scanning for the onboarding AP / SSID.
    Scanning,
    /// Connecting to the onboarding network.
    Connecting,
    /// Sending JOIN_REQUEST to the gateway.
    Requesting,
    /// Waiting for the gateway's JOIN_RESPONSE.
    Polling { attempt: u32 },
    /// Received an approved response; decrypting credentials.
    Decrypting,
    /// Onboarding succeeded — credentials available.
    Succeeded,
    /// Onboarding failed (denied, timeout, or crypto error).
    Failed { reason: FailReason },
}

/// Why onboarding failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailReason {
    /// Gateway denied the join request.
    Denied,
    /// No response after max poll attempts.
    Timeout,
    /// Decryption of credentials failed (wrong key or tampered).
    DecryptionError,
    /// Could not connect to onboarding AP.
    ConnectionFailed,
    /// Nonce in response did not match request.
    NonceMismatch,
}

/// Actions the platform layer should perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardAction {
    /// Scan for the onboarding SSID.
    StartScan,
    /// Connect to the onboarding network.
    Connect,
    /// Send the JOIN_REQUEST.
    SendRequest,
    /// Wait before polling again (ms).
    WaitPoll { ms: u64 },
    /// Decrypt the received ciphertext.
    Decrypt,
    /// No action needed (terminal state).
    Idle,
}

/// Events fed into the joiner state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardEvent {
    /// Onboarding AP found during scan.
    ApFound,
    /// Scan completed with no onboarding AP found.
    ApNotFound,
    /// Successfully connected to the onboarding network.
    Connected,
    /// Failed to connect to the onboarding network.
    ConnectFailed,
    /// JOIN_REQUEST sent successfully.
    RequestSent,
    /// Poll timer expired — check for response.
    PollTimeout,
    /// Received approved JOIN_RESPONSE.
    ResponseApproved,
    /// Received denied JOIN_RESPONSE.
    ResponseDenied,
    /// No response yet (poll returned empty).
    NoResponse,
    /// Response nonce did not match.
    NonceMismatch,
    /// Decryption succeeded.
    DecryptOk,
    /// Decryption failed.
    DecryptFailed,
}

/// Configuration for the onboarding state machine.
#[derive(Debug, Clone)]
pub struct OnboardConfig {
    /// Interval between poll attempts (ms).
    pub poll_interval_ms: u64,
    /// Maximum number of poll attempts before timeout.
    pub max_poll_attempts: u32,
    /// Maximum scan retries before giving up.
    pub max_scan_retries: u32,
}

impl Default for OnboardConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 10_000,
            max_poll_attempts: 6,
            max_scan_retries: 3,
        }
    }
}

/// Platform-independent joiner state machine for WSOP onboarding.
pub struct OnboardSm {
    state: OnboardState,
    config: OnboardConfig,
    scan_retries: u32,
}

impl OnboardSm {
    /// Create a new state machine starting in the Scanning state.
    pub fn new(config: OnboardConfig) -> Self {
        Self {
            state: OnboardState::Scanning,
            config,
            scan_retries: 0,
        }
    }

    /// Get the current state.
    pub fn state(&self) -> &OnboardState {
        &self.state
    }

    /// Get the initial action to perform after construction.
    pub fn initial_action(&self) -> OnboardAction {
        OnboardAction::StartScan
    }

    /// Feed an event and get the next action.
    pub fn handle_event(&mut self, event: OnboardEvent) -> OnboardAction {
        match (&self.state, event) {
            // --- Scanning ---
            (OnboardState::Scanning, OnboardEvent::ApFound) => {
                self.state = OnboardState::Connecting;
                OnboardAction::Connect
            }
            (OnboardState::Scanning, OnboardEvent::ApNotFound) => {
                self.scan_retries += 1;
                if self.scan_retries >= self.config.max_scan_retries {
                    self.state = OnboardState::Failed {
                        reason: FailReason::Timeout,
                    };
                    OnboardAction::Idle
                } else {
                    OnboardAction::StartScan
                }
            }

            // --- Connecting ---
            (OnboardState::Connecting, OnboardEvent::Connected) => {
                self.state = OnboardState::Requesting;
                OnboardAction::SendRequest
            }
            (OnboardState::Connecting, OnboardEvent::ConnectFailed) => {
                self.state = OnboardState::Failed {
                    reason: FailReason::ConnectionFailed,
                };
                OnboardAction::Idle
            }

            // --- Requesting ---
            (OnboardState::Requesting, OnboardEvent::RequestSent) => {
                self.state = OnboardState::Polling { attempt: 0 };
                OnboardAction::WaitPoll {
                    ms: self.config.poll_interval_ms,
                }
            }

            // --- Polling ---
            (OnboardState::Polling { attempt }, OnboardEvent::PollTimeout)
            | (OnboardState::Polling { attempt }, OnboardEvent::NoResponse) => {
                let attempt = *attempt + 1;
                if attempt >= self.config.max_poll_attempts {
                    self.state = OnboardState::Failed {
                        reason: FailReason::Timeout,
                    };
                    OnboardAction::Idle
                } else {
                    self.state = OnboardState::Polling { attempt };
                    OnboardAction::WaitPoll {
                        ms: self.config.poll_interval_ms,
                    }
                }
            }
            (OnboardState::Polling { .. }, OnboardEvent::ResponseApproved) => {
                self.state = OnboardState::Decrypting;
                OnboardAction::Decrypt
            }
            (OnboardState::Polling { .. }, OnboardEvent::ResponseDenied) => {
                self.state = OnboardState::Failed {
                    reason: FailReason::Denied,
                };
                OnboardAction::Idle
            }
            (OnboardState::Polling { .. }, OnboardEvent::NonceMismatch) => {
                self.state = OnboardState::Failed {
                    reason: FailReason::NonceMismatch,
                };
                OnboardAction::Idle
            }

            // --- Decrypting ---
            (OnboardState::Decrypting, OnboardEvent::DecryptOk) => {
                self.state = OnboardState::Succeeded;
                OnboardAction::Idle
            }
            (OnboardState::Decrypting, OnboardEvent::DecryptFailed) => {
                self.state = OnboardState::Failed {
                    reason: FailReason::DecryptionError,
                };
                OnboardAction::Idle
            }

            // Ignore mismatched events
            _ => OnboardAction::Idle,
        }
    }

    /// Whether the state machine has reached a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            OnboardState::Succeeded | OnboardState::Failed { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> OnboardConfig {
        OnboardConfig {
            poll_interval_ms: 1000,
            max_poll_attempts: 3,
            max_scan_retries: 2,
        }
    }

    #[test]
    fn starts_in_scanning() {
        let sm = OnboardSm::new(test_config());
        assert_eq!(*sm.state(), OnboardState::Scanning);
        assert_eq!(sm.initial_action(), OnboardAction::StartScan);
        assert!(!sm.is_terminal());
    }

    #[test]
    fn happy_path_scan_to_succeeded() {
        let mut sm = OnboardSm::new(test_config());

        // Scan → AP found → Connecting
        let action = sm.handle_event(OnboardEvent::ApFound);
        assert_eq!(*sm.state(), OnboardState::Connecting);
        assert_eq!(action, OnboardAction::Connect);

        // Connect → Connected → Requesting
        let action = sm.handle_event(OnboardEvent::Connected);
        assert_eq!(*sm.state(), OnboardState::Requesting);
        assert_eq!(action, OnboardAction::SendRequest);

        // Request sent → Polling
        let action = sm.handle_event(OnboardEvent::RequestSent);
        assert_eq!(*sm.state(), OnboardState::Polling { attempt: 0 });
        assert_eq!(action, OnboardAction::WaitPoll { ms: 1000 });

        // Response approved → Decrypting
        let action = sm.handle_event(OnboardEvent::ResponseApproved);
        assert_eq!(*sm.state(), OnboardState::Decrypting);
        assert_eq!(action, OnboardAction::Decrypt);

        // Decrypt OK → Succeeded
        let action = sm.handle_event(OnboardEvent::DecryptOk);
        assert_eq!(*sm.state(), OnboardState::Succeeded);
        assert_eq!(action, OnboardAction::Idle);
        assert!(sm.is_terminal());
    }

    #[test]
    fn scan_retries_then_timeout() {
        let mut sm = OnboardSm::new(test_config()); // max_scan_retries = 2

        // First scan miss — retry
        let action = sm.handle_event(OnboardEvent::ApNotFound);
        assert_eq!(*sm.state(), OnboardState::Scanning);
        assert_eq!(action, OnboardAction::StartScan);

        // Second scan miss — fail
        let action = sm.handle_event(OnboardEvent::ApNotFound);
        assert_eq!(
            *sm.state(),
            OnboardState::Failed {
                reason: FailReason::Timeout
            }
        );
        assert_eq!(action, OnboardAction::Idle);
        assert!(sm.is_terminal());
    }

    #[test]
    fn connect_failed() {
        let mut sm = OnboardSm::new(test_config());
        sm.handle_event(OnboardEvent::ApFound);

        let action = sm.handle_event(OnboardEvent::ConnectFailed);
        assert_eq!(
            *sm.state(),
            OnboardState::Failed {
                reason: FailReason::ConnectionFailed
            }
        );
        assert_eq!(action, OnboardAction::Idle);
    }

    #[test]
    fn poll_timeout_retries_then_fails() {
        let mut sm = OnboardSm::new(test_config()); // max_poll_attempts = 3
        sm.handle_event(OnboardEvent::ApFound);
        sm.handle_event(OnboardEvent::Connected);
        sm.handle_event(OnboardEvent::RequestSent);

        // Poll attempt 0 → 1
        let action = sm.handle_event(OnboardEvent::PollTimeout);
        assert_eq!(*sm.state(), OnboardState::Polling { attempt: 1 });
        assert_eq!(action, OnboardAction::WaitPoll { ms: 1000 });

        // Poll attempt 1 → 2
        let action = sm.handle_event(OnboardEvent::NoResponse);
        assert_eq!(*sm.state(), OnboardState::Polling { attempt: 2 });
        assert_eq!(action, OnboardAction::WaitPoll { ms: 1000 });

        // Poll attempt 2 → 3 (>= max) → Failed
        let action = sm.handle_event(OnboardEvent::PollTimeout);
        assert_eq!(
            *sm.state(),
            OnboardState::Failed {
                reason: FailReason::Timeout
            }
        );
        assert_eq!(action, OnboardAction::Idle);
    }

    #[test]
    fn response_denied() {
        let mut sm = OnboardSm::new(test_config());
        sm.handle_event(OnboardEvent::ApFound);
        sm.handle_event(OnboardEvent::Connected);
        sm.handle_event(OnboardEvent::RequestSent);

        let action = sm.handle_event(OnboardEvent::ResponseDenied);
        assert_eq!(
            *sm.state(),
            OnboardState::Failed {
                reason: FailReason::Denied
            }
        );
        assert_eq!(action, OnboardAction::Idle);
    }

    #[test]
    fn nonce_mismatch() {
        let mut sm = OnboardSm::new(test_config());
        sm.handle_event(OnboardEvent::ApFound);
        sm.handle_event(OnboardEvent::Connected);
        sm.handle_event(OnboardEvent::RequestSent);

        let action = sm.handle_event(OnboardEvent::NonceMismatch);
        assert_eq!(
            *sm.state(),
            OnboardState::Failed {
                reason: FailReason::NonceMismatch
            }
        );
        assert_eq!(action, OnboardAction::Idle);
    }

    #[test]
    fn decrypt_failed() {
        let mut sm = OnboardSm::new(test_config());
        sm.handle_event(OnboardEvent::ApFound);
        sm.handle_event(OnboardEvent::Connected);
        sm.handle_event(OnboardEvent::RequestSent);
        sm.handle_event(OnboardEvent::ResponseApproved);

        let action = sm.handle_event(OnboardEvent::DecryptFailed);
        assert_eq!(
            *sm.state(),
            OnboardState::Failed {
                reason: FailReason::DecryptionError
            }
        );
        assert_eq!(action, OnboardAction::Idle);
    }

    #[test]
    fn poll_then_approved_mid_retry() {
        let mut sm = OnboardSm::new(test_config());
        sm.handle_event(OnboardEvent::ApFound);
        sm.handle_event(OnboardEvent::Connected);
        sm.handle_event(OnboardEvent::RequestSent);

        // One poll timeout, then approved
        sm.handle_event(OnboardEvent::PollTimeout);
        assert_eq!(*sm.state(), OnboardState::Polling { attempt: 1 });

        let action = sm.handle_event(OnboardEvent::ResponseApproved);
        assert_eq!(*sm.state(), OnboardState::Decrypting);
        assert_eq!(action, OnboardAction::Decrypt);
    }

    #[test]
    fn terminal_states_ignore_events() {
        let mut sm = OnboardSm::new(test_config());
        sm.handle_event(OnboardEvent::ApFound);
        sm.handle_event(OnboardEvent::ConnectFailed);
        assert!(sm.is_terminal());

        // All events should return Idle in terminal state
        assert_eq!(
            sm.handle_event(OnboardEvent::ApFound),
            OnboardAction::Idle
        );
        assert_eq!(
            sm.handle_event(OnboardEvent::Connected),
            OnboardAction::Idle
        );
    }

    #[test]
    fn default_config_values() {
        let config = OnboardConfig::default();
        assert_eq!(config.poll_interval_ms, 10_000);
        assert_eq!(config.max_poll_attempts, 6);
        assert_eq!(config.max_scan_retries, 3);
    }
}
