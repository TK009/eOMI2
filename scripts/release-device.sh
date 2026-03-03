#!/usr/bin/env bash
# Release a previously claimed device by closing the flock fd.
#
# This script must be SOURCED, not executed.
#
# Usage:
#   . ./scripts/release-device.sh
#
# Expects DEVICE_FD to be set by claim-device.sh.
# Also works automatically if the process exits (kernel closes all fds).

if [[ -n "${DEVICE_FD:-}" ]]; then
    exec {DEVICE_FD}>&- 2>/dev/null || true
fi
unset DEVICE_PORT DEVICE_FD
