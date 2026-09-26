#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ ! -d "$repo_root/web/node_modules" ]]; then
  npm --prefix "$repo_root/web" ci
fi

cargo run --manifest-path "$repo_root/server/Cargo.toml" &
server_pid=$!
(cd "$repo_root/web" && npm run dev -- --host 127.0.0.1) &
web_pid=$!

cleanup() {
  kill "$web_pid" "$server_pid" 2>/dev/null || true
  wait "$web_pid" "$server_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM
wait "$web_pid"
