package main

import (
	"bufio"
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"time"
)

var errNoRsync = errors.New("GNU rsync not found — run `macup apply` first so it can be fetched through Nix (macOS's built-in openrsync is not reliable enough for this)")

func findRsync(e env) (string, error) {
	candidates := []string{
		"/opt/homebrew/bin/rsync",
		"/usr/local/bin/rsync",
		filepath.Join("/etc/profiles/per-user", e.user, "bin/rsync"),
		filepath.Join(e.home, ".nix-profile/bin/rsync"),
		"/run/current-system/sw/bin/rsync",
	}
	if p, err := exec.LookPath("rsync"); err == nil {
		candidates = append([]string{p}, candidates...)
	}
	for _, c := range candidates {
		if isGNURsync(c) {
			return c, nil
		}
	}
	nix := findNix()
	if nix == "" {
		return "", errNoRsync
	}
	out, err := exec.Command(nix, "--extra-experimental-features", "nix-command flakes",
		"build", "nixpkgs#rsync", "--no-link", "--print-out-paths").Output()
	if err != nil {
		return "", fmt.Errorf("fetching rsync through Nix failed: %w", err)
	}
	lines := strings.Fields(string(out))
	if len(lines) == 0 {
		return "", errNoRsync
	}
	bin := filepath.Join(lines[len(lines)-1], "bin", "rsync")
	if !isGNURsync(bin) {
		return "", errNoRsync
	}
	return bin, nil
}

func isGNURsync(path string) bool {
	out, err := exec.Command(path, "--version").Output()
	return err == nil && strings.HasPrefix(string(out), "rsync  version")
}

type rsyncProgress struct {
	transferred string
	percent     int
	rate        string
	eta         string
	files       string
}

var progressLine = regexp.MustCompile(`^\s*([\d.,]+[KMGTP]?)\s+(\d+)%\s+(\S+/s)\s+(\d+:\d\d:\d\d)(?:\s+\(xfr#(\d+))?`)

func parseProgress(line string) (rsyncProgress, bool) {
	m := progressLine.FindStringSubmatch(line)
	if m == nil {
		return rsyncProgress{}, false
	}
	var pct int
	fmt.Sscanf(m[2], "%d", &pct)
	return rsyncProgress{transferred: m[1], percent: pct, rate: m[3], eta: m[4], files: m[5]}, true
}

type rsyncJob struct {
	bin     string
	args    []string
	logPath string
}

type rsyncResult struct {
	exitCode int
	duration time.Duration
	logPath  string
}

func (r rsyncResult) summary() string {
	switch r.exitCode {
	case 0:
		return "finished"
	case 23:
		return "finished, but some files could not be copied (permissions or locked files) — see the log"
	case 24:
		return "finished (some files vanished while copying, usually harmless)"
	case 20:
		return "cancelled"
	default:
		return fmt.Sprintf("failed (rsync exit %d) — see the log", r.exitCode)
	}
}

func (r rsyncResult) ok() bool { return r.exitCode == 0 || r.exitCode == 23 || r.exitCode == 24 }

func (j rsyncJob) run(ctx context.Context, onProgress func(rsyncProgress), onLine func(string)) (rsyncResult, error) {
	start := time.Now()
	var log io.Writer = io.Discard
	if j.logPath != "" {
		if err := os.MkdirAll(filepath.Dir(j.logPath), 0o755); err != nil {
			return rsyncResult{}, err
		}
		f, err := os.Create(j.logPath)
		if err != nil {
			return rsyncResult{}, err
		}
		defer f.Close()
		fmt.Fprintf(f, "%s %s\n", j.bin, strings.Join(j.args, " "))
		log = f
	}
	cmd := exec.CommandContext(ctx, j.bin, j.args...)
	cmd.Cancel = func() error { return cmd.Process.Signal(os.Interrupt) }
	cmd.WaitDelay = 10 * time.Second
	pr, pw := io.Pipe()
	cmd.Stdout = pw
	cmd.Stderr = pw
	if err := cmd.Start(); err != nil {
		return rsyncResult{}, err
	}
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		sc := bufio.NewScanner(pr)
		sc.Buffer(make([]byte, 64*1024), 1024*1024)
		sc.Split(splitCRLF)
		for sc.Scan() {
			line := sc.Text()
			if p, ok := parseProgress(line); ok {
				if onProgress != nil {
					onProgress(p)
				}
				continue
			}
			fmt.Fprintln(log, line)
			if strings.TrimSpace(line) != "" && onLine != nil {
				onLine(line)
			}
		}
		io.Copy(io.Discard, pr)
	}()
	err := cmd.Wait()
	pw.Close()
	wg.Wait()
	res := rsyncResult{duration: time.Since(start), logPath: j.logPath}
	if err != nil {
		var exit *exec.ExitError
		if !errors.As(err, &exit) {
			return res, err
		}
		res.exitCode = exit.ExitCode()
		if ctx.Err() != nil {
			res.exitCode = 20
		}
	}
	fmt.Fprintf(log, "%s: %s after %s\n", time.Now().Format(time.RFC3339), res.summary(), res.duration.Round(time.Second))
	return res, nil
}

func splitCRLF(data []byte, atEOF bool) (int, []byte, error) {
	if i := bytes.IndexAny(data, "\r\n"); i >= 0 {
		return i + 1, data[:i], nil
	}
	if atEOF && len(data) > 0 {
		return len(data), data, nil
	}
	return 0, nil, nil
}

func logPath(e env, kind string) string {
	return filepath.Join(e.logDir(), fmt.Sprintf("%s-%s.log", kind, time.Now().Format("20060102-150405")))
}
