#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
npm --prefix "$repo_root/web" ci
npm --prefix "$repo_root/web" run build
cargo run --manifest-path "$repo_root/server/Cargo.toml" --release
