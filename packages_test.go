package main

import (
	"os"
	"strings"
	"testing"
)

const homeNix = `{ lib, pkgs, ... }:

{
  home.packages = with pkgs; [
    bat
    curl
    zoxide
  ];

  programs.home-manager.enable = true;
}
`

func TestAddPackageKeepsListSortedAndIndented(t *testing.T) {
	out, changed, err := editPackages(homeNix, []string{"go", "bat"}, true)
	if err != nil {
		t.Fatal(err)
	}
	if len(changed) != 1 || changed[0] != "go" {
		t.Fatalf("changed = %v, want only the new package", changed)
	}
	want := "    bat\n    curl\n    go\n    zoxide\n  ];"
	if !strings.Contains(out, want) {
		t.Fatalf("output:\n%s", out)
	}
	if !strings.Contains(out, "programs.home-manager.enable = true;") {
		t.Fatal("rest of the file was lost")
	}
}

func TestRemovePackage(t *testing.T) {
	out, changed, err := editPackages(homeNix, []string{"curl", "nope"}, false)
	if err != nil {
		t.Fatal(err)
	}
	if len(changed) != 1 || strings.Contains(out, "curl") {
		t.Fatalf("changed = %v, output:\n%s", changed, out)
	}
}

func TestEditCasksRewritesTheSingleLineList(t *testing.T) {
	src := `casks = [ "brave-browser" "ghostty" ];`
	out, _, err := editCasks(src, []string{"firefox"}, true)
	if err != nil {
		t.Fatal(err)
	}
	if out != `casks = [ "brave-browser" "firefox" "ghostty" ];` {
		t.Fatalf("got %s", out)
	}
}

// The real home.nix must stay parseable by this editor; otherwise `macup add`
// would fail on the user's actual config even though the unit cases pass.
func TestRealConfigIsEditable(t *testing.T) {
	for _, tc := range []struct {
		file string
		edit func(string, []string, bool) (string, []string, error)
	}{
		{"../nix-darwin/home.nix", editPackages},
		{"../nix-darwin/flake.nix", editCasks},
	} {
		src, err := readIfExists(tc.file)
		if err != nil {
			t.Skip("drive copy of the config not present")
		}
		if _, changed, err := tc.edit(src, []string{"macup-test-package"}, true); err != nil || len(changed) != 1 {
			t.Fatalf("%s: changed=%v err=%v", tc.file, changed, err)
		}
	}
}

func readIfExists(path string) (string, error) {
	data, err := os.ReadFile(path)
	return string(data), err
}
