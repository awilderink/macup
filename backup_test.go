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
	e := env{home: home}
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
		"Library/Application Support/AddressBook/AddressBook-v22.abcddb",
		"Library/Application Support/FileProvider/x/wharf/tombstone/a",
		"Library/Application Support/CloudDocs/session/db/client.db",
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
	out, err := exec.Command(bin, rsyncArgs("-n", "--out-format=%n", "--filter=merge "+rules, home+"/", t.TempDir()+"/")...).Output()
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
		"Library/Application Support/AddressBook/AddressBook-v22.abcddb",
		"Library/Application Support/App/data",
		"Pictures/holiday.jpg",
	}
	if !slices.Equal(files, want) {
		t.Fatalf("backed up %v\nwant %v", files, want)
	}
}

// A freshly installed Mac must not silently overwrite an existing backup with
// default settings: until one real backup has finished, it asks first.
func TestFirstBackupFromThisMacIsGuarded(t *testing.T) {
	e := env{home: t.TempDir(), volume: t.TempDir()}
	if firstBackupGuard(e) != nil {
		t.Fatal("an empty backup drive should not need confirmation")
	}
	os.MkdirAll(filepath.Join(e.backupHome(), "Documents"), 0o755)
	if firstBackupGuard(e) == nil {
		t.Fatal("first backup over an existing backup was not guarded")
	}
	os.MkdirAll(e.logDir(), 0o755)
	os.WriteFile(e.lastBackupFile(), []byte("2026-10-01 12:00 · finished\n"), 0o644)
	if firstBackupGuard(e) != nil {
		t.Fatal("a Mac with a previous successful backup should not be asked again")
	}
	if got := lastBackup(e); got != "2026-10-01 12:00 · finished" {
		t.Fatalf("lastBackup = %q", got)
	}
}

// Without a terminal nobody can answer, so a guard must stop the run unless
// --yes (or a dry run) says to continue.
func TestGuardsStopNonInteractiveRunsUnlessYes(t *testing.T) {
	g := &guard{problem: "something is off", question: "continue?"}
	if err := resolveGuards([]check{precomputed(g)}, false); err == nil {
		t.Fatal("a guard did not stop a non-interactive run")
	}
	if err := resolveGuards([]check{precomputed(g)}, true); err != nil {
		t.Fatalf("--yes should continue: %v", err)
	}
	if err := resolveGuards([]check{precomputed(nil)}, false); err != nil {
		t.Fatalf("no problem should mean no stop: %v", err)
	}
}

func TestMirrorGuardCountsWhatWouldBeDeleted(t *testing.T) {
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
	g, err := mirrorGuard(e, backupOptions{mirror: true}, bin, rules)
	if err != nil {
		t.Fatal(err)
	}
	if g == nil || !strings.Contains(g.problem, "3 files and folders") {
		t.Fatalf("guard = %+v, want gone.txt, old/ and old/a.txt counted", g)
	}
	if _, err := os.Stat(filepath.Join(e.backupHome(), "gone.txt")); err != nil {
		t.Fatal("the preview deleted files")
	}
}
