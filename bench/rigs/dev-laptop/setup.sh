#!/usr/bin/env bash
set -euo pipefail

echo "marg bench dev-laptop rig setup"
echo "-------------------------------"

missing=0

check_cmd() {
    if command -v "$1" >/dev/null 2>&1; then
        echo "ok      $1 found"
    else
        echo "missing $1 not found ($2)"
        missing=$((missing + 1))
    fi
}

check_cmd cargo "install Rust via rustup.rs"
check_cmd curl  "install curl via your package manager"
check_cmd k6    "install via 'brew install k6' or https://k6.io/docs/get-started/installation/"

if [ "$missing" -gt 0 ]; then
    echo
    echo "$missing tool(s) missing. Install and re-run."
    exit 1
fi

echo
echo "All tools present. Run ./run.sh to execute the dev-laptop scenarios."
