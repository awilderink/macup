package main

import (
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"golang.org/x/sys/unix"
)

type photoLibrary struct {
	name string
	src  string
	dst  string
}

func (l photoLibrary) staging() string {
	return filepath.Join(filepath.Dir(l.dst), "."+l.name+".staging")
}

func librariesIn(dir string) []string {
	paths, _ := filepath.Glob(filepath.Join(dir, "*.photoslibrary"))
	return paths
}

func photoLibraries(e env) []photoLibrary {
	var libs []photoLibrary
	for _, p := range librariesIn(filepath.Join(e.home, "Pictures")) {
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

func (l photoLibrary) shrinkGuard() (*guard, error) {
	if _, err := os.Stat(photosDB(l.dst)); err != nil {
		return nil, nil
	}
	srcItems, err := libraryItems(l.src)
	if err != nil {
		return nil, fmt.Errorf("cannot read %s: %w", l.name, err)
	}
	dstItems, err := libraryItems(l.dst)
	if err != nil {
		return nil, nil
	}
	var srcOriginals, dstOriginals int
	var wg sync.WaitGroup
	wg.Go(func() { srcOriginals = countFiles(filepath.Join(l.src, "originals")) })
	wg.Go(func() { dstOriginals = countFiles(filepath.Join(l.dst, "originals")) })
	wg.Wait()
	if srcItems >= dstItems && srcOriginals >= dstOriginals {
		return nil, nil
	}
	return &guard{
		problem: fmt.Sprintf("%s on this Mac has %d items and %d original files, the backup has %d items and %d original files. "+
			"Backing up would remove the difference from the backup. If Photos → Settings → iCloud is set to \"Optimize Mac Storage\", "+
			"originals live only in iCloud; switch to \"Download Originals\" first.", l.name, srcItems, srcOriginals, dstItems, dstOriginals),
		question: "Back up the smaller library anyway?",
	}, nil
}

func photoArgs(src, dst string) []string {
	return rsyncArgs("--delete",
		"--filter=- /database/Photos.sqlite*",
		"--filter=H /resources/cpl/",
		src+"/", dst+"/")
}

func backupPhotoLibrary(e env, o backupOptions, bin string, l photoLibrary) error {
	srcItems, err := libraryItems(l.src)
	if err != nil {
		return fmt.Errorf("cannot read %s: %w", l.name, err)
	}
	if o.dryRun {
		return previewPhotoBackup(bin, l, srcItems)
	}
	staging := l.staging()
	if err := os.RemoveAll(staging); err != nil {
		return err
	}
	if err := os.MkdirAll(e.photosDir(), 0o755); err != nil {
		return err
	}
	_, statErr := os.Stat(l.dst)
	replacing := statErr == nil
	if replacing {
		if err := unix.Clonefile(l.dst, staging, unix.CLONE_NOFOLLOW); err != nil {
			return fmt.Errorf("could not stage the Photos backup: %w", err)
		}
	}
	if err := stagePhotoLibrary(e, bin, l, staging, srcItems); err != nil {
		os.RemoveAll(staging)
		return err
	}
	if !replacing {
		return os.Rename(staging, l.dst)
	}
	if err := unix.RenamexNp(staging, l.dst, unix.RENAME_SWAP); err != nil {
		os.RemoveAll(staging)
		return fmt.Errorf("could not swap in the new Photos backup: %w", err)
	}
	return os.RemoveAll(staging)
}

func stagePhotoLibrary(e env, bin string, l photoLibrary, staging string, srcItems int) error {
	db := photosDB(staging)
	if err := os.MkdirAll(filepath.Dir(db), 0o755); err != nil {
		return err
	}
	for _, f := range []string{db, db + "-wal", db + "-shm"} {
		os.Remove(f)
	}
	if err := withSpinner("Taking a consistent snapshot of the Photos database…", func() error {
		_, err := sqliteQuery(photosDB(l.src), "vacuum into '"+strings.ReplaceAll(db, "'", "''")+"';")
		return err
	}); err != nil {
		return err
	}
	res, err := runJob("Backing up "+l.name+" → "+l.dst, rsyncJob{bin: bin, args: photoArgs(l.src, staging), logPath: logPath(e, "photos"), strict: true})
	if err != nil {
		return err
	}
	if err := reportResult("Photos copy", res); err != nil {
		return err
	}
	if err := verifyPhotoBackup(staging, srcItems); err != nil {
		return err
	}
	ok("Photos library %s backed up and verified (%d items) in %s", l.name, srcItems, res.duration.Round(time.Second))
	return nil
}

func verifyPhotoBackup(lib string, wantItems int) error {
	check, err := sqliteQuery(photosDB(lib), "pragma quick_check;")
	if err != nil {
		return err
	}
	if check != "ok" {
		return fmt.Errorf("the copied Photos database failed its integrity check: %s", truncate(check, 200))
	}
	got, err := libraryItems(lib)
	if err != nil {
		return err
	}
	if got != wantItems {
		return fmt.Errorf("the copied Photos database has %d items, expected %d (was Photos still syncing? try again)", got, wantItems)
	}
	return nil
}

func previewPhotoBackup(bin string, l photoLibrary, srcItems int) error {
	var files, deleted int
	var bytes int64
	err := dryRun(bin, photoArgs(l.src, l.dst), func(it rsyncItem) {
		switch {
		case it.deletion():
			deleted++
		case it.file():
			files++
			bytes += it.size
		}
	})
	if err != nil {
		return err
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

func restorePhotos(e env, target string) error {
	libs := librariesIn(e.photosDir())
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
		res, err := runJob("Restoring "+filepath.Base(src)+" → "+dst, rsyncJob{bin: bin, args: rsyncArgs(src+"/", dst+"/"), logPath: logPath(e, "restore")})
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
