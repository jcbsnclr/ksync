# Source this from inside the ksync repository:
#
#   . ./env.sh
#
# It points the client and server at the checked-in development configs.
# This file is deliberately not named .env and is not loaded automatically;
# sourcing it changes only the current shell.

KSYNC_ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || {
    echo "env.sh: source this from inside the ksync repository" >&2
    return 1 2>/dev/null || exit 1
}

export KSYNC_CLIENT_CONFIG="$KSYNC_ROOT/examples/config/client.toml"
export KSYNC_SERVER_CONFIG="$KSYNC_ROOT/examples/config/server.toml"

unset KSYNC_ROOT
