#!/usr/bin/env bash
# Release a previously claimed device.
# Usage: ./scripts/release-device.sh "$DEVICE_LOCK"

set -euo pipefail

lock="${1:?usage: release-device.sh <lockfile>}"
rm -f "$lock"
