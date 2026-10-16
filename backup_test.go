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

// A freshly installed Mac must not silently overwrite an existing backup with
// default settings: without a previous successful backup it needs --yes.
func TestFirstBackupFromThisMacNeedsConfirmation(t *testing.T) {
	e := env{home: t.TempDir(), volume: t.TempDir()}
	os.MkdirAll(filepath.Join(e.backupHome(), "Documents"), 0o755)
	if err := confirmFirstBackup(e, backupOptions{}); err == nil {
		t.Fatal("first backup over an existing backup was allowed without confirmation")
	}
	if err := confirmFirstBackup(e, backupOptions{dryRun: true}); err != nil {
		t.Fatalf("a dry run should not need confirmation: %v", err)
	}
	if err := confirmFirstBackup(e, backupOptions{yes: true}); err != nil {
		t.Fatalf("--yes should confirm: %v", err)
	}
	os.MkdirAll(e.logDir(), 0o755)
	os.WriteFile(filepath.Join(e.logDir(), "backup-20261001-120000.log"),
		[]byte("rsync -ahXN src dst\n2026-10-01T12:00:00+01:00: finished after 5m0s\n"), 0o644)
	if err := confirmFirstBackup(e, backupOptions{}); err != nil {
		t.Fatalf("a Mac with a previous successful backup should not be asked again: %v", err)
	}
}

func TestFailedOrDryRunBackupsDoNotCountAsBackedUp(t *testing.T) {
	e := env{home: t.TempDir()}
	os.MkdirAll(e.logDir(), 0o755)
	os.WriteFile(filepath.Join(e.logDir(), "backup-20261001-120000.log"),
		[]byte("rsync -ahXN --dry-run src dst\n2026-10-01T12:00:00+01:00: finished after 1s\n"), 0o644)
	os.WriteFile(filepath.Join(e.logDir(), "backup-20261002-120000.log"),
		[]byte("rsync -ahXN src dst\n2026-10-02T12:00:00+01:00: cancelled after 1s\n"), 0o644)
	if hasBackedUpBefore(e) {
		t.Fatal("dry runs and cancelled runs were counted as real backups")
	}
}

func TestMirrorPreviewCountsWhatWouldBeDeleted(t *testing.T) {
	e := env{home: t.TempDir(), volume: t.TempDir()}
	bin, err := findRsync(e)
	if err != nil {
		t.Skip("GNU rsync not available:", err)
	}
	os.WriteFile(filepath.Join(e.home, "kept.txt"), []byte("x"), 0o644)
	for _, f := range []string{"kept.txt", "gone.txt", "old/a.txt"} {
		p := filepath.Join(e.backupHome(), f)
		os.MkdirAll(filepath.Dir(p), 0o755)
		os.WriteFile(p, []byte("x"), 0o644)
	}
	rules, _ := writeRulesFile()
	defer os.Remove(rules)
	d, err := previewDeletions(e, backupOptions{mirror: true}, bin, rules)
	if err != nil {
		t.Fatal(err)
	}
	if d.count != 3 {
		t.Fatalf("count = %d (%v), want gone.txt, old/ and old/a.txt", d.count, d.samples)
	}
	if _, err := os.Stat(filepath.Join(e.backupHome(), "gone.txt")); err != nil {
		t.Fatal("the preview deleted files")
	}
}
