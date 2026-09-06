#!/usr/bin/env bash
set -euo pipefail

export LC_ALL=C

.tekton/scripts/check-secret-argv.py
.tekton/scripts/test-release-state.sh
cargo fmt --all -- --check
cargo +stable test --locked -p ota-core --target x86_64-unknown-linux-gnu --config 'unstable.build-std=[]'
cargo +stable test --locked -p release-tool --target x86_64-unknown-linux-gnu --config 'unstable.build-std=[]'
cargo +stable test --locked -p ov3660 --target x86_64-unknown-linux-gnu --config 'unstable.build-std=[]'
cargo check --locked --release
