# Feature Specification: WiFi Provisioning with Captive Portal

**Feature Branch**: `002-wifi-provisioning`
**Created**: 2026-03-06
**Status**: Draft
**Input**: User description: "Wifi provisioning with captive portal, triggered when no wifi configured at build time or no saved SSIDs found. Includes hostname, API key management, and multi-AP support."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - First-Time WiFi Setup (Priority: P1)

A user powers on a new device that has no build-time WiFi configuration. The device automatically starts a captive portal (open access point). The user connects to this AP from their phone or laptop, is redirected to a setup page, and configures the device's WiFi connection, hostname, and API key in a single form.

**Why this priority**: Without WiFi provisioning, the device cannot connect to any network and is non-functional. This is the foundational capability.

**Independent Test**: Can be tested by powering on an unconfigured device, connecting to its AP, completing the form, and verifying the device joins the configured network.

**Acceptance Scenarios**:

1. **Given** a device with no build-time WiFi and no saved SSIDs, **When** the device powers on, **Then** it starts a captive portal AP within 10 seconds.
2. **Given** the captive portal is active, **When** a user connects to the AP and opens a browser, **Then** they are redirected to the provisioning form.
3. **Given** the provisioning form is displayed, **When** the user fills in SSID, password, hostname, and API key (generate/set), **Then** the device saves the configuration, closes the portal, and connects to the specified network.
4. **Given** a user selects "Generate" for the API key, **When** the form is submitted, **Then** a new random API key is generated and displayed once for the user to copy.

---

### User Story 2 - Reconnection After WiFi Loss (Priority: P1)

A previously configured device loses its WiFi connection and cannot find any of its saved SSIDs. It reopens the captive portal while preserving all existing configuration and data. The user can update WiFi settings without losing their existing setup.

**Why this priority**: Devices will inevitably lose connectivity (router change, network outage). Graceful recovery is critical for operational resilience.

**Independent Test**: Can be tested by configuring a device, then making its saved SSID unavailable, and verifying the portal reopens with existing config preserved.

**Acceptance Scenarios**:

1. **Given** a configured device connected to WiFi, **When** the network becomes unavailable and no saved SSIDs are found, **Then** the device reopens the captive portal.
2. **Given** the captive portal reopens after connection loss, **When** the form is displayed, **Then** previously configured SSIDs and hostname are shown (pre-filled), but passwords and the API key are NOT revealed.
3. **Given** the reconnection portal is active, **When** the user submits the form without changing the API key field, **Then** the existing API key is preserved.
4. **Given** the reconnection portal is active, **When** the user enters a new API key value or selects "Regenerate", **Then** the API key is updated accordingly.

---

### User Story 3 - Multiple WiFi Access Points (Priority: P2)

A user configures multiple WiFi access points (up to a configurable maximum, default 3) so the device can failover between networks. The device tries each saved SSID before falling back to the captive portal.

**Why this priority**: Multi-AP support improves reliability by allowing fallback networks, but the device is functional with a single AP.

**Independent Test**: Can be tested by configuring two SSIDs, connecting to the first, making it unavailable, and verifying the device connects to the second.

**Acceptance Scenarios**:

1. **Given** the provisioning form, **When** the user adds multiple SSID/password pairs (up to the configured maximum), **Then** all are saved.
2. **Given** a device with multiple saved SSIDs, **When** the current network is lost, **Then** the device attempts to connect to each saved SSID in order before opening the captive portal.
3. **Given** the maximum number of APs is configured via a build variable, **When** the form is rendered, **Then** it shows the correct number of AP configuration slots.

---

### User Story 4 - API Access During Provisioning Mode (Priority: P2)

While the captive portal is active, the device's OMI APIs remain accessible over the portal's AP network. This allows management tools to interact with the device during setup. A build flag can disable this behavior for restricted deployments.

**Why this priority**: API access during provisioning enables advanced workflows and diagnostics, but is not required for basic setup.

**Independent Test**: Can be tested by connecting to the captive portal AP and making API calls to the device's OMI endpoints.

**Acceptance Scenarios**:

1. **Given** the captive portal is active and API-during-provisioning is enabled (default), **When** a client connected to the portal AP makes an OMI API request, **Then** the device responds normally.
2. **Given** the build flag to deny API access during provisioning is enabled, **When** a client attempts an OMI API request during provisioning, **Then** the request is rejected.
3. **Given** the captive portal is active with API access enabled, **When** the portal redirects browsers, **Then** OMI API URLs are excluded from captive portal redirection so they function normally.

---

### Edge Cases

- What happens when the user submits the form with an invalid SSID or wrong password? The device should attempt connection, detect failure, and reopen the portal with an error message.
- What happens when the device has saved SSIDs but none are currently reachable? The device should cycle through all saved SSIDs with appropriate retry logic before opening the captive portal.
- What happens if the user closes the browser before completing the form? The captive portal remains active until a valid configuration is submitted or the device is power-cycled.
- What happens during a power loss while saving configuration? Configuration writes should be atomic (write-then-swap) to prevent corruption.
- What happens if the maximum AP count build variable is set to 1? The form shows a single AP configuration slot and behaves correctly.
- What happens if the device has a build-time WiFi configuration? It attempts to connect to that network first and only opens the portal if the connection fails and no other saved SSIDs are available.
- What happens if a saved SSID reappears while the captive portal is active? The device auto-reconnects and closes the portal without user intervention (e.g., router reboot recovery).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST start a captive portal when no WiFi has been configured at build time and no saved SSIDs exist.
- **FR-002**: System MUST start a captive portal when it cannot connect to any saved or build-time configured SSID.
- **FR-003**: The captive portal MUST present a single form with fields for: WiFi SSID(s) and password(s), device hostname, and API key management. The SSID field MUST show a scannable list of visible networks for selection, with a manual entry fallback for hidden networks.
- **FR-004**: The provisioning form MUST support configuring up to MAX_WIFI_APS access points (build-configurable, default 3).
- **FR-005**: The API key field MUST support three modes: set to a user-provided value, generate a new random key, or leave unchanged (when re-provisioning). On first-time provisioning, the API key is mandatory — the user must either set or generate one.
- **FR-006**: System MUST NOT reveal the existing API key or saved WiFi passwords when reopening the captive portal for re-provisioning. Unchanged passwords are preserved if the user does not re-enter them.
- **FR-007**: System MUST preserve all existing configuration and data when reopening the captive portal after connection loss.
- **FR-008**: System MUST attempt to connect to all saved SSIDs (in order) before falling back to the captive portal.
- **FR-009**: System MUST allow access to all OMI APIs over the captive portal AP network by default during provisioning mode.
- **FR-010**: A build flag MUST exist to disable OMI API access during provisioning mode.
- **FR-011**: OMI API URLs MUST NOT be subject to captive portal redirection (so API calls work normally when portal is active).
- **FR-012**: System MUST display the generated API key exactly once after generation, for the user to copy.
- **FR-013**: Configuration persistence MUST be atomic to prevent corruption from power loss during writes.
- **FR-014**: The captive portal MUST redirect HTTP requests to the provisioning form (standard captive portal behavior).
- **FR-017**: The captive portal AP MUST broadcast with SSID "setup-{hostname}", where hostname defaults to the build flag value "eOMI".
- **FR-018**: While the captive portal is active after connection loss, the device MUST periodically scan for saved SSIDs in the background and auto-reconnect if one becomes available, closing the portal automatically.
- **FR-015**: System MUST validate that at least one SSID/password pair is provided before accepting the form.
- **FR-016**: System MUST attempt to connect to the configured network after form submission and report success or failure to the user.

### Key Entities

- **WiFi Credential**: An SSID and password pair representing a configured access point. A device stores up to MAX_WIFI_APS credentials.
- **Device Configuration**: The persistent state including WiFi credentials, hostname, and API key hash. Survives power cycles and re-provisioning.
- **Captive Portal Session**: The temporary state of the provisioning AP being active, including the HTTP server serving the form and handling submissions.
- **API Key**: A secret token used to authenticate API requests to the device. Stored securely (hashed), never displayed after initial setup except when regenerated.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A user with no prior knowledge can complete first-time device provisioning in under 2 minutes.
- **SC-002**: Device reopens the captive portal within 30 seconds of determining all saved SSIDs are unreachable.
- **SC-003**: When multiple SSIDs are configured, the device fails over to an available saved network within 30 seconds without user intervention.
- **SC-004**: Existing device configuration and data are fully preserved through 100% of re-provisioning cycles.
- **SC-005**: API key is never exposed in the provisioning form during re-provisioning (zero information leakage).
- **SC-006**: OMI API requests succeed during captive portal mode when the access-deny build flag is not set.
- **SC-007**: Configuration survives unexpected power loss during provisioning (no corruption).

## Clarifications

### Session 2026-03-06

- Q: Should SSID entry be manual text, a scan list, or both? → A: Scan visible networks as selectable list, with manual entry fallback for hidden networks.
- Q: Should saved WiFi passwords be shown pre-filled on re-provisioning? → A: Passwords masked, require re-entry to change (consistent with API key treatment).
- Q: Is API key mandatory on first-time provisioning? → A: Mandatory - user must generate or set an API key before the device accepts the configuration.
- Q: What SSID does the captive portal AP broadcast? → A: "setup-{hostname}", where hostname is user-configurable and defaults to build flag value "eOMI" (e.g., "setup-eOMI").
- Q: Should the device background-scan for saved SSIDs while the portal is active? → A: Yes, auto-reconnect to a saved SSID if found and close the portal automatically.

## Assumptions

- The device has a WiFi radio capable of running in AP (access point) mode simultaneously or by switching from station mode.
- Build-time WiFi configuration is optional and provided via build variables (e.g., environment variables or config files at compile time).
- The captive portal uses an open (unencrypted) AP for initial provisioning, as the device has no shared secret with the user yet.
- The hostname is used for mDNS/network identification and the captive portal AP SSID. It must follow standard hostname conventions (alphanumeric, hyphens, max 63 chars). The default hostname is provided by a build flag with value "eOMI".
- API key generation produces a cryptographically random token of sufficient length (at least 32 characters).
- The device stores the API key as a hash (not plaintext) for security.
- "All OMI APIs" refers to the device's existing HTTP API endpoints, whatever they may be at the time of implementation.
- The SSID scan/retry logic uses standard WiFi scanning with reasonable timeouts before declaring all SSIDs unreachable.
