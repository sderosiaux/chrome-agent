#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
cargo test -- --test-threads=1 2>&1 | tail -3
