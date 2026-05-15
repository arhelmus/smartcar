#!/usr/bin/env sh
# entrypoint.sh – start the openauto autoapp head unit emulator.
#
# Environment variables (all optional, sane defaults provided):
#   DISPLAY        X11 display to use (default: :0).  Must be reachable from
#                  inside the container – mount /tmp/.X11-unix and run
#                  `xhost +local:` on the host before starting the container.
#   OPENAUTO_ARGS  Extra arguments forwarded verbatim to autoapp.
#
# Signal handling: SIGTERM is caught and forwarded to autoapp so that
# `docker stop` gives the process a chance to shut down cleanly.

set -eu

# ---------------------------------------------------------------------------
# X11 / DISPLAY setup
# ---------------------------------------------------------------------------
DISPLAY="${DISPLAY:-:0}"
export DISPLAY

# Verify the X server is reachable; warn but do not abort so the container
# can still be used in headless / testing scenarios.
if command -v xdpyinfo >/dev/null 2>&1; then
    if ! xdpyinfo >/dev/null 2>&1; then
        echo "WARNING: Cannot reach X display '${DISPLAY}'. autoapp may fail to start." >&2
        echo "         On the host run: xhost +local:docker" >&2
    fi
fi

# ---------------------------------------------------------------------------
# Launch autoapp
# ---------------------------------------------------------------------------
AUTOAPP_BIN="${AUTOAPP_BIN:-/usr/local/bin/autoapp}"

if [ ! -x "${AUTOAPP_BIN}" ]; then
    echo "ERROR: autoapp binary not found at '${AUTOAPP_BIN}'" >&2
    exit 1
fi

echo "Starting openauto autoapp (DISPLAY=${DISPLAY}) ..." >&2

# Run autoapp in the background so the shell can handle signals.
# shellcheck disable=SC2086
"${AUTOAPP_BIN}" ${OPENAUTO_ARGS:-} &
AUTOAPP_PID=$!

# Forward SIGTERM / SIGINT to autoapp and wait for it to exit.
_shutdown() {
    echo "Received stop signal – shutting down autoapp (pid ${AUTOAPP_PID}) ..." >&2
    kill -TERM "${AUTOAPP_PID}" 2>/dev/null || true
    wait "${AUTOAPP_PID}" 2>/dev/null || true
    exit 0
}

trap _shutdown TERM INT

# Wait for autoapp to finish; propagate its exit code.
wait "${AUTOAPP_PID}"
EXIT_CODE=$?
echo "autoapp exited with code ${EXIT_CODE}" >&2
exit "${EXIT_CODE}"
