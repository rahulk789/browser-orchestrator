#!/usr/bin/env bash
set -euo pipefail

# Start Restate
restate-server &
RESTATE_PID=$!

# Wait until Restate is ready
until restate whoami >/dev/null 2>&1; do
    sleep 1
done

cargo run &
APP_PID=$!

sleep 1

restate dp register http://localhost:4000 --force

# Cleanup on exit
trap 'kill $RESTATE_PID $APP_PID' EXIT

wait
