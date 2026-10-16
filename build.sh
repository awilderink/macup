#!/bin/zsh
set -euo pipefail
cd "${0:A:h}"
nix --extra-experimental-features 'nix-command flakes' shell nixpkgs#go -c sh -c '
  go vet ./... &&
  go test -count=1 ./... &&
  CGO_ENABLED=0 GOOS=darwin GOARCH=arm64 go build -trimpath -ldflags="-s -w" -o ../../macup .
'
print "built ${0:A:h:h:h}/macup"
