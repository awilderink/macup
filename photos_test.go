package main

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func makeLibrary(t *testing.T, dir string, items int, originals ...string) {
	t.Helper()
	os.MkdirAll(filepath.Join(dir, "database"), 0o755)
	db := photosDB(dir)
	os.Remove(db)
	var sql strings.Builder
	sql.WriteString("pragma journal_mode=wal; create table ZASSET(Z_PK integer primary key, ZTRASHEDSTATE integer);")
	for i := range items {
		fmt.Fprintf(&sql, "insert into ZASSET values (%d, 0);", i+1)
	}
	sql.WriteString("insert into ZASSET values (100000, 1);")
	if out, err := exec.Command("/usr/bin/sqlite3", db, sql.String()).CombinedOutput(); err != nil {
		t.Fatalf("%v: %s", err, out)
	}
	for _, f := range append(originals, "resources/derivatives/thumb.jpg", "resources/cpl/cloudsync.noindex/upload.bin") {
		p := filepath.Join(dir, f)
		os.MkdirAll(filepath.Dir(p), 0o755)
		os.WriteFile(p, []byte(f), 0o644)
	}
}

func photoFixture(t *testing.T) (env, photoLibrary, string) {
	t.Helper()
	e := env{home: t.TempDir(), volume: t.TempDir()}
	bin, err := findRsync(e)
	if err != nil {
		t.Skip("GNU rsync not available:", err)
	}
	src := filepath.Join(e.home, "Pictures", "Photos Library.photoslibrary")
	makeLibrary(t, src, 2, "originals/A/a.jpg", "originals/B/b.mov")
	libs := photoLibraries(e)
	if len(libs) != 1 {
		t.Fatalf("libraries = %v", libs)
	}
	return e, libs[0], bin
}

func TestPhotoBackupCopiesLibraryWithoutICloudCache(t *testing.T) {
	e, l, bin := photoFixture(t)
	if err := backupPhotoLibrary(e, backupOptions{}, bin, l); err != nil {
		t.Fatal(err)
	}
	for _, f := range []string{"originals/A/a.jpg", "originals/B/b.mov", "resources/derivatives/thumb.jpg"} {
		if _, err := os.Stat(filepath.Join(l.dst, f)); err != nil {
			t.Errorf("%s missing from backup", f)
		}
	}
	if _, err := os.Stat(filepath.Join(l.dst, "resources/cpl")); err == nil {
		t.Error("iCloud upload cache was copied")
	}
	if n, err := libraryItems(l.dst); err != nil || n != 2 {
		t.Fatalf("backup items = %d, %v", n, err)
	}
	if _, err := os.Stat(photosDB(l.dst) + "-wal"); err == nil {
		t.Error("a stale WAL file was left next to the snapshot")
	}
	if _, err := os.Stat(l.previous()); err == nil {
		t.Error("the previous copy was not cleaned up")
	}
}

func TestPhotoBackupUpdatesAnExistingCopy(t *testing.T) {
	e, l, bin := photoFixture(t)
	if err := backupPhotoLibrary(e, backupOptions{}, bin, l); err != nil {
		t.Fatal(err)
	}
	makeLibrary(t, l.src, 3, "originals/C/c.jpg")
	os.MkdirAll(filepath.Join(l.dst, "resources/cpl"), 0o755)
	if err := backupPhotoLibrary(e, backupOptions{}, bin, l); err != nil {
		t.Fatal(err)
	}
	if n, _ := libraryItems(l.dst); n != 3 {
		t.Fatalf("items = %d, want 3", n)
	}
	if _, err := os.Stat(filepath.Join(l.dst, "originals/C/c.jpg")); err != nil {
		t.Error("new original not copied")
	}
	if _, err := os.Stat(filepath.Join(l.dst, "resources/cpl")); err == nil {
		t.Error("a stale iCloud cache in the backup should be removed")
	}
}

// A library that lost originals (for example after "Optimize Mac Storage"
// moved them to iCloud) must not wipe them from the backup by default.
func TestPhotoBackupRefusesWhenLibraryShrank(t *testing.T) {
	e, l, bin := photoFixture(t)
	if err := backupPhotoLibrary(e, backupOptions{}, bin, l); err != nil {
		t.Fatal(err)
	}
	os.Remove(filepath.Join(l.src, "originals/B/b.mov"))
	err := backupPhotoLibrary(e, backupOptions{}, bin, l)
	if err == nil || !strings.Contains(err.Error(), "Optimize Mac Storage") {
		t.Fatalf("err = %v, want a refusal", err)
	}
	if _, err := os.Stat(filepath.Join(l.dst, "originals/B/b.mov")); err != nil {
		t.Fatal("the refused run still removed an original from the backup")
	}
	if err := backupPhotoLibrary(e, backupOptions{yes: true}, bin, l); err != nil {
		t.Fatalf("--yes should allow it: %v", err)
	}
}

func TestPhotoBackupRestoresLastGoodCopyAfterInterruption(t *testing.T) {
	e, l, bin := photoFixture(t)
	if err := backupPhotoLibrary(e, backupOptions{}, bin, l); err != nil {
		t.Fatal(err)
	}
	os.Rename(l.dst, l.previous())
	os.MkdirAll(filepath.Join(l.dst, "originals"), 0o755)
	if err := recoverInterruptedPhotoBackup(l); err != nil {
		t.Fatal(err)
	}
	if n, err := libraryItems(l.dst); err != nil || n != 2 {
		t.Fatalf("recovered copy items = %d, %v", n, err)
	}
	if _, err := os.Stat(l.previous()); err == nil {
		t.Fatal("previous copy still present after recovery")
	}
}

func TestPhotoVerificationCatchesItemMismatch(t *testing.T) {
	e, l, bin := photoFixture(t)
	if err := backupPhotoLibrary(e, backupOptions{}, bin, l); err != nil {
		t.Fatal(err)
	}
	if err := verifyPhotoBackup(l, 5); err == nil {
		t.Fatal("a copy with fewer items than the source passed verification")
	}
}

func TestPhotoRestoreNeverOverwritesAnExistingLibrary(t *testing.T) {
	e, l, bin := photoFixture(t)
	if err := backupPhotoLibrary(e, backupOptions{}, bin, l); err != nil {
		t.Fatal(err)
	}
	target := filepath.Join(t.TempDir(), "Pictures")
	if err := restorePhotos(e, target); err != nil {
		t.Fatal(err)
	}
	restored := filepath.Join(target, l.name)
	if n, err := libraryItems(restored); err != nil || n != 2 {
		t.Fatalf("restored items = %d, %v", n, err)
	}
	os.WriteFile(filepath.Join(restored, "marker"), []byte("mine"), 0o644)
	if err := restorePhotos(e, target); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(restored, "marker")); err != nil {
		t.Fatal("an existing library was overwritten")
	}
}
