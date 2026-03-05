<!--
Sync Impact Report
===================
Version change: N/A → 1.0.0 (initial ratification)
Modified principles: N/A (all new)
Added sections:
  - Core Principles (5 principles)
  - Hardware & Platform Constraints
  - Development Workflow
  - Governance
Removed sections: N/A
Templates requiring updates:
  - .specify/templates/plan-template.md — ✅ no update needed
    (Constitution Check section is generic; gates derived at plan time)
  - .specify/templates/spec-template.md — ✅ no update needed
    (spec template is feature-agnostic; no constitution-specific fields)
  - .specify/templates/tasks-template.md — ✅ no update needed
    (task phases are generic; no principle-specific task types required)
  - .specify/templates/commands/*.md — N/A (directory does not exist)
Follow-up TODOs: none
-->

# Embedded O-MI v2 Constitution

## Core Principles

### I. Resource Efficiency

All code MUST target the cheapest viable hardware. Every feature,
dependency, and abstraction MUST justify its memory and CPU cost.

- Memory allocation MUST be minimized; prefer stack over heap,
  fixed buffers over dynamic allocation.
- Computation MUST be kept to the minimum required for correctness.
- New dependencies MUST be evaluated for binary size and RAM impact
  before adoption.
- Rationale: The project targets low-cost ESP microcontrollers where
  RAM is measured in hundreds of kilobytes.

### II. Reliability

The system MUST operate without errors under all supported
conditions and recover gracefully from faults.

- All error paths MUST be handled explicitly; panics and unwraps
  in production code are forbidden unless mathematically provable.
- Fault tolerance MUST be designed in: the device MUST recover from
  transient failures (network drops, malformed input, power glitches)
  without requiring a manual restart.
- Rationale: IoT devices operate unattended; a crash or hang has no
  human nearby to intervene.

### III. Platform Separation

Platform-specific code MUST be isolated from platform-independent
logic at the file and module level.

- Hardware abstractions (GPIO, Wi-Fi, timers, flash) MUST live in
  dedicated platform modules, not mixed into business logic.
- Platform-independent code MUST compile and run on the host
  (x86_64 Linux) without any ESP toolchain.
- Rationale: Enables thorough host-side testing and keeps the
  codebase portable across ESP variants and future targets.

### IV. Test Discipline

All platform-independent code MUST be tested on the host before
any device-level testing occurs.

- Host tests MUST pass before attempting a
  device build or flash.
- End-to-end tests MUST be the last validation
  step, run only after all host tests succeed.
- New logic MUST include corresponding host-level tests unless it
  is inherently hardware-dependent.
- Device locking protocol MUST be followed for all hardware
  interactions (see Development Workflow).
- Rationale: Host tests are fast, deterministic, and free of
  hardware contention; catching bugs early saves device cycles.

### V. Simplicity

Code MUST be as simple as the requirements allow. Complexity MUST
be justified against a simpler alternative.

- YAGNI: features, abstractions, and configurability MUST NOT be
  added for hypothetical future needs.
- Prefer three similar lines over a premature abstraction.
- Error handling and validation MUST only be added at system
  boundaries (user input, network, external APIs); internal
  invariants may rely on type-system guarantees.
- Rationale: Simpler code is smaller (Principle I), more reliable
  (Principle II), and easier to test (Principle IV).

## Governance

- This constitution is the highest-authority document for the
  project. All code, reviews, and design decisions MUST comply.
- **Amendments** require:
  1. A written proposal describing the change and its rationale.
  2. Update to this file with an incremented version number.
  3. A sync impact report (HTML comment at top of this file)
     listing affected templates and artifacts.
- **Versioning** follows semantic versioning:
  - MAJOR: principle removed or redefined incompatibly.
  - MINOR: new principle or section added, or material expansion.
  - PATCH: clarifications, wording, or typo fixes.
- **Compliance review**: every PR and design document SHOULD be
  checked against the Core Principles. The plan template's
  "Constitution Check" gate enforces this at planning time.
- **Runtime guidance**: see `CLAUDE.md` for development-time
  conventions that implement these principles.

**Version**: 1.0.0 | **Ratified**: 2026-03-04 | **Last Amended**: 2026-03-04
