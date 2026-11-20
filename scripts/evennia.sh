#!/usr/bin/env bash
# Manage a local Evennia server used as the integration-test MUD.
#
# Usage: scripts/evennia.sh {setup|start|stop|status|reset}
#
# Everything lives in .evennia/ (git-ignored). The server listens for telnet
# on localhost:4000. A superuser "struppi" / "struppi-test" is created.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/.evennia"
VENV="$DIR/venv"
GAME="$DIR/game"

export EVENNIA_SUPERUSER_USERNAME="struppi"
export EVENNIA_SUPERUSER_PASSWORD="struppi-test"
export EVENNIA_SUPERUSER_EMAIL="struppi@example.invalid"

activate() {
    [ -d "$VENV" ] || { echo "run '$0 setup' first" >&2; exit 1; }
    # shellcheck disable=SC1091
    source "$VENV/bin/activate"
}

case "${1:-}" in
    setup)
        mkdir -p "$DIR"
        [ -d "$VENV" ] || python3 -m venv "$VENV"
        activate
        pip install --quiet --upgrade pip
        pip install --quiet evennia
        if [ ! -d "$GAME" ]; then
            (cd "$DIR" && evennia --init game)
            (cd "$GAME" && evennia migrate)
        fi
        echo "Evennia is set up in $GAME"
        ;;
    start)
        activate
        (cd "$GAME" && evennia start)
        ;;
    stop)
        activate
        (cd "$GAME" && evennia stop)
        ;;
    status)
        activate
        (cd "$GAME" && evennia status)
        ;;
    reset)
        if [ -d "$GAME" ]; then
            activate
            (cd "$GAME" && evennia stop) || true
        fi
        rm -rf "$GAME"
        echo "Removed $GAME; run '$0 setup' to recreate it"
        ;;
    *)
        echo "usage: $0 {setup|start|stop|status|reset}" >&2
        exit 2
        ;;
esac
