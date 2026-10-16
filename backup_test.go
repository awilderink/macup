package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

// Runs the real rsync with the embedded rules against a fake home folder, so a
// rule edit that accidentally starts copying caches or a Photos library (the
// backup drive cannot hold the current one next to the old one) fails here.
func TestEmbeddedRulesSelectTheRightFiles(t *testing.T) {
	home := t.TempDir()
	e := env{home: home, user: filepath.Base(home)}
	bin, err := findRsync(e)
	if err != nil {
		t.Skip("GNU rsync not available:", err)
	}
	for _, f := range []string{
		"Documents/a.txt",
		"Pictures/Photos Library.photoslibrary/database/Photos.sqlite",
		"Pictures/holiday.jpg",
		"Library/Caches/x",
		"Library/Application Support/App/data",
		"code/proj/node_modules/m/index.js",
		".claude/projects/p/memory/m.md",
		".claude/file-history/h",
		".claude/settings.json",
	} {
		p := filepath.Join(home, f)
		os.MkdirAll(filepath.Dir(p), 0o755)
		os.WriteFile(p, []byte("x"), 0o644)
	}
	rules, err := writeRulesFile()
	if err != nil {
		t.Fatal(err)
	}
	defer os.Remove(rules)
	out, err := exec.Command(bin, "-a", "-n", "--out-format=%n", "--filter=merge "+rules, home+"/", t.TempDir()+"/").Output()
	if err != nil {
		t.Fatal(err)
	}
	var files []string
	for _, l := range strings.Split(string(out), "\n") {
		if l != "" && !strings.HasSuffix(l, "/") {
			files = append(files, l)
		}
	}
	slices.Sort(files)
	want := []string{
		".claude/projects/p/memory/m.md",
		".claude/settings.json",
		"Documents/a.txt",
		"Library/Application Support/App/data",
		"Pictures/holiday.jpg",
	}
	if !slices.Equal(files, want) {
		t.Fatalf("backed up %v\nwant %v", files, want)
	}
}
