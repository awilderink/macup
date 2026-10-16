#!/bin/zsh
set -euo pipefail
cd "${0:A:h}"
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
out="${MACUP_OUT:-$HOME/.local/bin/macup}"
mkdir -p "${out:h}"
cp target/release/macup "$out"
print "built $out"
if [[ -d /Volumes/Backup/home ]]; then
  cp target/release/macup /Volumes/Backup/macup
  print "copied to /Volumes/Backup/macup"
fi
