# Feature Specification: mDNS Hostname and DNS-SD Service Advertisement

**Feature Branch**: `004-mdns-dns-sd`
**Created**: 2026-03-06
**Status**: Draft
**Input**: User description: "mDNS and DNS-SD like described in the thesis"

## Clarifications

### Session 2026-03-06

- Q: Should all devices share a common mDNS hostname ("eomi.local") as the thesis proposes, or use a different mechanism? → A: Drop the common hostname; rely on DNS-SD service browsing to find any device (standard-compliant).
- Q: Where in the O-DF tree should discovery results be stored? → A: `/Objects/System/discovery/<hostname>` as an InfoItem, value is `<ip>:<port>`, timestamp serves as lastSeen.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Device Reachable by Hostname (Priority: P1)

A user connects to the same LAN as an eOMI device and opens a browser. Instead of needing to know the device's IP address, the user types the device's hostname with the ".local" suffix (e.g., `http://eomi.local`) and reaches the device's web interface. The device advertises its hostname via mDNS so that operating systems with mDNS support (Windows, macOS, Linux, iOS) can resolve it without any special configuration.

**Why this priority**: Hostname resolution is the foundational capability. Without it, users must know the device IP, which changes across networks and DHCP leases. This is the single most impactful discovery feature and was identified in the thesis as essential for the configurator app entry point.

**Independent Test**: Can be tested by powering on a device on a LAN, then from another device on the same network, resolving `<hostname>.local` (e.g., via `ping eomi.local` or opening `http://eomi.local` in a browser) and verifying it resolves to the device's IP address.

**Acceptance Scenarios**:

1. **Given** a device is connected to a WiFi network with hostname "eomi", **When** another device on the same network queries "eomi.local", **Then** the query resolves to the device's current LAN IP address.
2. **Given** a device has a user-configured hostname "living-room" set during provisioning, **When** another device queries "living-room.local", **Then** it resolves to the device's IP address.
3. **Given** a device's IP address changes (e.g., DHCP renewal), **When** the new IP is assigned, **Then** the device updates its mDNS advertisement so that the hostname resolves to the new IP within a reasonable time.
4. **Given** a device is in captive portal mode (AP mode), **When** a client connects to the device's AP, **Then** hostname resolution via mDNS is NOT active (the captive portal DNS server takes precedence on the AP interface).

---

### User Story 2 - DNS-SD Service Advertisement (Priority: P2)

An eOMI device advertises its HTTP service via DNS-SD so that service discovery tools and other eOMI devices on the LAN can find it. Using standard mDNS service browsing (e.g., `dns-sd -B _omi._tcp` on macOS, or Avahi on Linux), a user or application can discover all eOMI devices on the network along with their hostnames, IP addresses, and port.

**Why this priority**: Service discovery enables the "find all devices" use case described in the thesis. While hostname resolution (P1) lets users reach a device they already know about, DNS-SD lets the configurator app and other devices enumerate all available eOMI devices automatically.

**Independent Test**: Can be tested by running an mDNS service browser on a computer on the same LAN as the device, browsing for the registered service type, and verifying the device appears in the results with correct hostname, port, and any TXT record metadata.

**Acceptance Scenarios**:

1. **Given** a device is connected to a WiFi network, **When** a service browser queries for the device's service type, **Then** the device appears in the results with its hostname, port 80, and service type.
2. **Given** multiple eOMI devices are on the same network, **When** a service browser queries the service type, **Then** all devices appear as separate service instances with unique instance names.
3. **Given** a device is advertising its service, **When** the device disconnects from the network, **Then** it sends a goodbye packet (TTL=0) so that other devices remove it from their caches promptly.

---

### User Story 3 - Find Any Device via DNS-SD Service Browsing (Priority: P2)

A user or application that does not know any device hostname uses a DNS-SD service browser (or a native app using the OS mDNS API) to discover all eOMI devices on the LAN. This provides the zero-knowledge entry point: without knowing any device name or IP, the user can browse for the eOMI service type and pick any device from the results to open the configurator app.

**Why this priority**: This provides the zero-knowledge discovery entry point. A user who knows nothing about device names or IP addresses can browse for the service type to find and reach any device. This complements the captive portal fallback (already implemented) for cases where mDNS is available.

**Independent Test**: Can be tested by having two or more eOMI devices on a network, browsing for the service type, and verifying that all devices appear in the results with their hostnames, IPs, and ports.

**Acceptance Scenarios**:

1. **Given** two eOMI devices with unique hostnames "kitchen" and "bedroom", **When** a service browser queries the eOMI service type, **Then** both devices appear in the results with their hostnames and IP addresses.
2. **Given** a user selects one of the discovered devices from the service browser results, **When** they open its address in a browser, **Then** they reach the device's web interface.

---

### User Story 4 - Discovery Results Published as O-DF InfoItems (Priority: P3)

A device periodically performs mDNS/DNS-SD discovery to find other eOMI devices on the network and publishes the results into its local O-DF data tree. The configurator web app (which cannot use UDP directly from a browser) reads these discovery results from the device's O-DF tree via the existing HTTP/WebSocket OMI API to display a list of all known devices on the network.

**Why this priority**: This closes the browser-discovery gap identified in the thesis. Browsers cannot perform mDNS queries directly, but the device can query on their behalf and expose results as standard O-DF data. This is a higher-level integration that builds on the advertisement capabilities from P1 and P2.

**Independent Test**: Can be tested by having two devices on the same network, waiting for the discovery interval to elapse, then reading the first device's O-DF tree to verify the second device appears as a discovered peer with its hostname and IP.

**Acceptance Scenarios**:

1. **Given** two eOMI devices are on the same network (device B hostname "bedroom"), **When** device A performs periodic discovery, **Then** an InfoItem at `/Objects/System/discovery/bedroom` appears in device A's O-DF tree with value `<ip>:<port>` and a timestamp reflecting when the peer was last seen.
2. **Given** device B goes offline, **When** device A's next discovery cycle runs, **Then** device B's InfoItem is removed from the discovery path in the O-DF tree.
3. **Given** a web app reads `/Objects/System/discovery` from any device's O-DF tree, **Then** it receives all currently known peers as InfoItems, each named by hostname with value `<ip>:<port>`.

---

### Edge Cases

- What happens when two devices have the same unique hostname? mDNS conflict resolution (probing) should detect the collision and one device should select an alternative name.
- How does the system handle a network with no other mDNS-capable devices? Discovery results are simply empty; the device continues to advertise normally.
- What happens when the device transitions between AP mode and station mode? mDNS advertisement should start when station mode connects and stop when entering AP mode (where captive portal DNS takes over).
- What if the network blocks multicast traffic? mDNS will not function; the captive portal fallback (already implemented) remains available as an alternative discovery method.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Device MUST advertise its configured hostname via mDNS on the ".local" domain when connected to a WiFi network in station mode.
- **FR-002**: Device MUST respond to mDNS queries for its hostname with its current IP address.
- **FR-003**: Device MUST advertise an HTTP service via DNS-SD with the appropriate service type, port 80, and a TXT record containing at minimum the device's O-DF Object path.
- **FR-004**: Device MUST use the hostname configured during WiFi provisioning; if no hostname was set, a sensible default MUST be used (e.g., "eomi").
- **FR-005**: Each device MUST have a unique DNS-SD service instance name to distinguish it from other devices of the same service type.
- **FR-006**: When the device disconnects from the network, it MUST send mDNS goodbye announcements (TTL=0) if possible.
- **FR-007**: Device MUST NOT run mDNS advertisement while in captive portal (AP) mode; the captive portal DNS responder handles all DNS in that mode.
- **FR-008**: Device MUST periodically discover other eOMI devices on the LAN via DNS-SD service browsing.
- **FR-009**: Discovery results MUST be published as InfoItems under `/Objects/System/discovery/<hostname>` where the value is `<ip>:<port>` and the value timestamp represents when the peer was last seen.
- **FR-010**: mDNS hostname conflict detection MUST be handled; if a conflict is detected, the device MUST select an alternative hostname (e.g., by appending a suffix).
- **FR-011**: The mDNS and DNS-SD functionality MUST operate within the memory and CPU constraints of the target embedded device.

### Key Entities

- **mDNS Hostname**: The ".local" domain name the device responds to. Composed of the user-configured unique hostname plus the ".local" suffix.
- **DNS-SD Service Instance**: A named service advertisement containing the service type, protocol, port, hostname, and optional TXT record metadata. Uniquely identifies one device's service on the network.
- **Discovery Result**: An InfoItem at `/Objects/System/discovery/<hostname>` representing a peer device found via DNS-SD browsing. Value is `<ip>:<port>`, timestamp is when the peer was last seen.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A device is reachable by its ".local" hostname from another device on the same LAN within 5 seconds of connecting to WiFi.
- **SC-002**: A DNS-SD service browser discovers the device's service within 10 seconds of the device connecting to the network.
- **SC-003**: When multiple devices are on the same LAN, a DNS-SD service browse returns all devices within 10 seconds.
- **SC-004**: Discovery results for peer devices appear in the O-DF tree within 60 seconds of a new peer joining the network.
- **SC-005**: mDNS/DNS-SD functionality adds no more than 4 KB of additional RAM usage to the running system.
- **SC-006**: Hostname conflict between two devices with the same name is resolved automatically within 30 seconds, with both devices remaining reachable under distinct hostnames.

## Assumptions

- The target platform provides an mDNS library or sufficient low-level UDP multicast support to implement mDNS.
- The WiFi provisioning flow (already implemented in spec 002) sets the hostname that mDNS will use.
- The existing captive portal DNS server (already implemented) exclusively handles DNS on the AP interface; mDNS operates only on the station (STA) interface.
- DNS-SD service type will follow a convention to identify eOMI services specifically. The exact service type is a design decision for the planning phase.
- Periodic discovery interval is a design decision for the planning phase; a reasonable default is 30-60 seconds.
