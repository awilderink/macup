package main

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
)

func checkMounted(volume string) error {
	var vol, parent syscall.Stat_t
	if err := syscall.Stat(volume, &vol); err != nil {
		return fmt.Errorf("%s not found — is the backup drive connected?", volume)
	}
	if err := syscall.Stat(filepath.Dir(volume), &parent); err != nil {
		return err
	}
	if vol.Dev == parent.Dev {
		return fmt.Errorf("%s is an empty folder, not the mounted drive — connect and unlock the backup disk", volume)
	}
	return nil
}

func freeSpace(path string) (free, total uint64, err error) {
	var st syscall.Statfs_t
	if err := syscall.Statfs(path, &st); err != nil {
		return 0, 0, err
	}
	return st.Bavail * uint64(st.Bsize), st.Blocks * uint64(st.Bsize), nil
}

type lock struct{ dir string }

func acquireLock(dir string) (*lock, error) {
	if err := os.Mkdir(dir, 0o755); err != nil {
		if !errors.Is(err, fs.ErrExist) {
			return nil, err
		}
		data, _ := os.ReadFile(filepath.Join(dir, "pid"))
		pid, _ := strconv.Atoi(strings.TrimSpace(string(data)))
		if pid > 0 && processAlive(pid) {
			return nil, fmt.Errorf("another backup or restore is running (pid %d)", pid)
		}
		if err := os.RemoveAll(dir); err != nil {
			return nil, err
		}
		if err := os.Mkdir(dir, 0o755); err != nil {
			return nil, err
		}
	}
	if err := os.WriteFile(filepath.Join(dir, "pid"), []byte(strconv.Itoa(os.Getpid())+"\n"), 0o644); err != nil {
		os.RemoveAll(dir)
		return nil, err
	}
	return &lock{dir: dir}, nil
}

func (l *lock) release() { os.RemoveAll(l.dir) }

func processAlive(pid int) bool {
	err := syscall.Kill(pid, 0)
	return err == nil || errors.Is(err, syscall.EPERM)
}

func nearlyFull(free, total uint64) bool { return total > 0 && free*10 < total }
