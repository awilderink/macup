package main

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"golang.org/x/sys/unix"
)

type photoLibrary struct {
	name string
	src  string
	dst  string
}

func (l photoLibrary) previous() string {
	return filepath.Join(filepath.Dir(l.dst), "."+l.name+".previous")
}

func (e env) photosDir() string { return filepath.Join(e.volume, "photos") }

func photoLibraries(e env) []photoLibrary {
	paths, _ := filepath.Glob(filepath.Join(e.home, "Pictures", "*.photoslibrary"))
	libs := make([]photoLibrary, 0, len(paths))
	for _, p := range paths {
		name := filepath.Base(p)
		libs = append(libs, photoLibrary{name: name, src: p, dst: filepath.Join(e.photosDir(), name)})
	}
	return libs
}

func photosAppRunning() bool { return exec.Command("pgrep", "-x", "Photos").Run() == nil }

func photosDB(lib string) string { return filepath.Join(lib, "database", "Photos.sqlite") }

func sqliteQuery(db, query string) (string, error) {
	out, err := exec.Command("/usr/bin/sqlite3", "-bail", "file:"+db+"?mode=ro", query).CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("sqlite3 %s: %v: %s", db, err, strings.TrimSpace(string(out)))
	}
	return strings.TrimSpace(string(out)), nil
}

func libraryItems(lib string) (int, error) {
	out, err := sqliteQuery(photosDB(lib), "select count(*) from ZASSET where ZTRASHEDSTATE = 0;")
	if err != nil {
		return 0, err
	}
	return strconv.Atoi(out)
}

func countFiles(dir string) int {
	n := 0
	filepath.WalkDir(dir, func(_ string, d fs.DirEntry, err error) error {
		if err == nil && d.Type().IsRegular() {
			n++
		}
		return nil
	})
	return n
}

func checkLibraryNotShrunk(l photoLibrary, srcItems int) error {
	if _, err := os.Stat(photosDB(l.dst)); err != nil {
		return nil
	}
	dstItems, err := libraryItems(l.dst)
	if err != nil {
		return nil
	}
	srcOriginals := countFiles(filepath.Join(l.src, "originals"))
	dstOriginals := countFiles(filepath.Join(l.dst, "originals"))
	if srcItems >= dstItems && srcOriginals >= dstOriginals {
		return nil
	}
	return fmt.Errorf("%s on this Mac has %d items and %d original files, the backup has %d items and %d original files. "+
		"Backing up would remove the difference from the backup. If Photos → Settings → iCloud is set to \"Optimize Mac Storage\", "+
		"originals live only in iCloud; switch to \"Download Originals\" first. Add --yes to back up anyway",
		l.name, srcItems, srcOriginals, dstItems, dstOriginals)
}

func photoArgs(l photoLibrary, extra ...string) []string {
	args := []string{"-ahXN", "--delete", unreadableXattrs,
		"--filter=- /database/Photos.sqlite*",
		"--filter=H /resources/cpl/",
	}
	args = append(args, extra...)
	return append(args, l.src+"/", l.dst+"/")
}

func recoverInterruptedPhotoBackup(l photoLibrary) error {
	if _, err := os.Stat(l.previous()); err != nil {
		return nil
	}
	warn("a previous Photos backup of %s was interrupted; putting the last good copy back", l.name)
	if err := os.RemoveAll(l.dst); err != nil {
		return err
	}
	return os.Rename(l.previous(), l.dst)
}

func backupPhotos(e env, o backupOptions, bin string) error {
	libs := photoLibraries(e)
	if len(libs) == 0 {
		return nil
	}
	if photosAppRunning() && !o.dryRun {
		return errors.New("quit the Photos app first; its library is a live database that cannot be copied safely while it is open")
	}
	for _, l := range libs {
		if err := backupPhotoLibrary(e, o, bin, l); err != nil {
			return err
		}
	}
	return nil
}

func backupPhotoLibrary(e env, o backupOptions, bin string, l photoLibrary) error {
	if err := recoverInterruptedPhotoBackup(l); err != nil {
		return err
	}
	srcItems, err := libraryItems(l.src)
	if err != nil {
		return fmt.Errorf("cannot read %s: %w", l.name, err)
	}
	if !o.yes {
		if err := checkLibraryNotShrunk(l, srcItems); err != nil {
			return err
		}
	}
	if o.dryRun {
		return previewPhotoBackup(bin, l, srcItems)
	}
	if err := os.MkdirAll(e.photosDir(), 0o755); err != nil {
		return err
	}
	hadPrevious := false
	if _, err := os.Stat(l.dst); err == nil {
		if err := unix.Clonefile(l.dst, l.previous(), unix.CLONE_NOFOLLOW); err != nil {
			return fmt.Errorf("could not keep the previous Photos backup aside: %w", err)
		}
		hadPrevious = true
	}
	rollback := func(cause error) error {
		if !hadPrevious {
			return fmt.Errorf("%w; the incomplete copy at %s is not usable", cause, l.dst)
		}
		if err := os.RemoveAll(l.dst); err == nil {
			os.Rename(l.previous(), l.dst)
		}
		return fmt.Errorf("%w; the previous Photos backup was kept", cause)
	}

	if err := os.MkdirAll(filepath.Join(l.dst, "database"), 0o755); err != nil {
		return rollback(err)
	}
	snapshot := photosDB(l.dst) + ".macup-new"
	os.Remove(snapshot)
	if err := withSpinner("Taking a consistent snapshot of the Photos database…", func() {
		_, err = sqliteQuery(photosDB(l.src), "vacuum into '"+strings.ReplaceAll(snapshot, "'", "''")+"';")
	}); err != nil {
		return rollback(err)
	}
	if err != nil {
		return rollback(err)
	}
	res, err := runJob("Backing up "+l.name+" → "+l.dst, rsyncJob{bin: bin, args: photoArgs(l, "--info=progress2"), logPath: logPath(e, "photos")})
	if err != nil {
		return rollback(err)
	}
	if res.exitCode != 0 && res.exitCode != 24 {
		reportResult("Photos backup", res)
		return rollback(errors.New("copying the Photos library did not finish"))
	}
	db := photosDB(l.dst)
	if err := os.Rename(snapshot, db); err != nil {
		return rollback(err)
	}
	os.Remove(db + "-wal")
	os.Remove(db + "-shm")
	if err := verifyPhotoBackup(l, srcItems); err != nil {
		return rollback(err)
	}
	if hadPrevious {
		discard := l.previous() + ".discard"
		if os.Rename(l.previous(), discard) == nil {
			os.RemoveAll(discard)
		}
	}
	ok("Photos library %s backed up and verified (%d items) in %s", l.name, srcItems, res.duration.Round(time.Second))
	return nil
}

func verifyPhotoBackup(l photoLibrary, wantItems int) error {
	check, err := sqliteQuery(photosDB(l.dst), "pragma quick_check;")
	if err != nil {
		return err
	}
	if check != "ok" {
		return fmt.Errorf("the copied Photos database failed its integrity check: %s", truncate(check, 200))
	}
	got, err := libraryItems(l.dst)
	if err != nil {
		return err
	}
	if got != wantItems {
		return fmt.Errorf("the copied Photos database has %d items, expected %d (was Photos still syncing? try again)", got, wantItems)
	}
	return nil
}

func previewPhotoBackup(bin string, l photoLibrary, srcItems int) error {
	out, err := exec.Command(bin, photoArgs(l, "--dry-run", "--no-h", "--out-format=%i %l %n")...).Output()
	var exit *exec.ExitError
	if err != nil && (!errors.As(err, &exit) || exit.ExitCode() != 24) {
		return fmt.Errorf("previewing the Photos backup failed: %w", err)
	}
	var files, deleted int
	var bytes int64
	for _, line := range strings.Split(string(out), "\n") {
		if strings.HasPrefix(line, "*deleting") {
			deleted++
			continue
		}
		fields := strings.SplitN(line, " ", 3)
		if len(fields) == 3 && strings.HasPrefix(fields[0], ">f") {
			files++
			n, _ := strconv.ParseInt(fields[1], 10, 64)
			bytes += n
		}
	}
	step("Photos library %s (%d items)", l.name, srcItems)
	note("%s to copy (%s), %s to remove from the backup, plus the database", plural(files, "file"), humanBytes(bytes), plural(deleted, "file or folder", "files and folders"))
	return nil
}

func lastPhotoBackup(e env) string {
	var parts []string
	for _, l := range photoLibraries(e) {
		info, err := os.Stat(photosDB(l.dst))
		if err != nil {
			parts = append(parts, l.name+": never")
			continue
		}
		parts = append(parts, l.name+": "+info.ModTime().Format("2006-01-02 15:04"))
	}
	if len(parts) == 0 {
		return "no library on this Mac"
	}
	return strings.Join(parts, " · ")
}

func backedUpLibraries(e env) []string {
	paths, _ := filepath.Glob(filepath.Join(e.photosDir(), "*.photoslibrary"))
	return paths
}

func restorePhotos(e env, target string) error {
	libs := backedUpLibraries(e)
	if len(libs) == 0 {
		return fmt.Errorf("no Photos library backed up in %s", e.photosDir())
	}
	bin, err := findRsync(e)
	if err != nil {
		return err
	}
	for _, src := range libs {
		dst := filepath.Join(target, filepath.Base(src))
		if _, err := os.Stat(dst); err == nil {
			warn("%s already exists; move it aside first if you want the backup copy instead", dst)
			continue
		}
		if err := os.MkdirAll(target, 0o755); err != nil {
			return err
		}
		res, err := runJob("Restoring "+filepath.Base(src)+" → "+dst, rsyncJob{bin: bin, args: []string{"-ahXN", "--info=progress2", src + "/", dst + "/"}, logPath: logPath(e, "restore")})
		if err != nil {
			return err
		}
		if err := reportResult("Photos restore", res); err != nil {
			return err
		}
		note("open Photos while holding Option to pick this library, then make it the System Photo Library in Photos → Settings")
	}
	return nil
}
