#!/bin/sh
# Runs the fill benchmarks in a fixed container: 2 cores, 4 GB memory, no swap, pinned image.
# Arguments go to the bench, e.g. `benches/run.sh --compare benches/baseline.json`.
set -eu

# rust:1.95-bookworm
IMAGE="rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

docker run --rm \
  --cpuset-cpus 0-1 --cpus 2 \
  --memory 4g --memory-swap 4g \
  -v "$ROOT":/docx-fill \
  -v docx-fill-bench-cargo:/usr/local/cargo/registry \
  -v docx-fill-bench-target:/target \
  -e CARGO_TARGET_DIR=/target \
  -w /docx-fill \
  "$IMAGE" \
  cargo bench --quiet --all-features --bench fill -- "$@"
