#!/usr/bin/env bash
# Show USB serial devices and their lock status via the HTTP lock server.
#
# Usage:
#   ./scripts/list-devices.sh
#
# Environment:
#   DEVICE_LOCK_URL — lock server URL (default: http://localhost:7357)

set -euo pipefail

LOCK_URL="${DEVICE_LOCK_URL:-http://localhost:7357}"

response=$(curl -sf "$LOCK_URL/devices" 2>/dev/null) || {
    echo "ERROR: cannot reach lock server at $LOCK_URL" >&2
    echo "Start it with: ./scripts/start-lock-server.sh" >&2
    exit 1
}

# Format the JSON response
python3 -c "
import sys, json, time

data = json.load(sys.stdin)
devices = data.get('devices', [])

if not devices:
    print('No USB serial devices found.')
    sys.exit(0)

for d in devices:
    dev = d['device']
    if d['status'] == 'free':
        print(f'  {dev:<20}  FREE')
    else:
        h = d.get('holder', {})
        parts = []
        if h.get('pid'): parts.append(f'PID={h[\"pid\"]}')
        if h.get('host'): parts.append(f'HOST={h[\"host\"]}')
        if h.get('container'): parts.append(f'CONTAINER={h[\"container\"]}')
        if h.get('time'): parts.append(f'TIME={h[\"time\"]}')
        ttl = d.get('expires_at', 0) - time.time()
        parts.append(f'TTL={int(ttl)}s')
        info = '  '.join(parts)
        print(f'  {dev:<20}  LOCKED  {info}')

total = data.get('total', 0)
locked = data.get('locked', 0)
free = data.get('free', 0)
print()
print(f'Total: {total}  Locked: {locked}  Free: {free}')
" <<< "$response"
