#!/bin/zsh
set -euo pipefail
cd "${0:A:h}"
out="${MACUP_OUT:-$HOME/.local/bin/macup}"
nix --extra-experimental-features 'nix-command flakes' shell nixpkgs#go -c sh -c "
  go vet ./... &&
  go test -count=1 ./... &&
  CGO_ENABLED=0 GOOS=darwin GOARCH=arm64 go build -trimpath -ldflags='-s -w' -o '$out' .
"
print "built $out"
if [[ -d /Volumes/Backup/Users ]]; then
  cp "$out" /Volumes/Backup/macup
  print "copied to /Volumes/Backup/macup"
fi
