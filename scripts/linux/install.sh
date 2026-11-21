#!/bin/sh
set -eu
APP_ROOT=$(dirname -- "$(readlink -f -- "$0")")
if [ "${1:-}" = --uninstall ]; then
    exec "$APP_ROOT/AppRun" --uninstall-desktop
fi
if [ "$#" -ne 0 ]; then
    echo "Usage: $0 [--uninstall]" >&2
    exit 1
fi
exec "$APP_ROOT/AppRun" --install-desktop
