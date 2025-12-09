#!/bin/bash
set -e
mkdir -p dist
cargo build --release
cp target/release/gh-prism "dist/gh-prism"
