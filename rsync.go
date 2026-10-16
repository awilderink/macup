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
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"
)

func rsyncArgs(extra ...string) []string {
	return append([]string{"-ahXN", "--filter=-x com.apple.macl"}, extra...)
}

var errNoRsync = errors.New("GNU rsync not found — run `macup apply` first so it can be fetched through Nix (macOS's built-in openrsync is not reliable enough for this)")

var (
	rsyncMu  sync.Mutex
	rsyncBin string
)

func findRsync(e env) (string, error) {
	rsyncMu.Lock()
	defer rsyncMu.Unlock()
	if rsyncBin == "" {
		bin, err := locateRsync(e)
		if err != nil {
			return "", err
		}
		rsyncBin = bin
	}
	return rsyncBin, nil
}

func locateRsync(e env) (string, error) {
	candidates := []string{
		"/opt/homebrew/bin/rsync",
		"/usr/local/bin/rsync",
		filepath.Join("/etc/profiles/per-user", e.user(), "bin/rsync"),
		filepath.Join(e.home, ".nix-profile/bin/rsync"),
		"/run/current-system/sw/bin/rsync",
	}
	if p, err := exec.LookPath("rsync"); err == nil && !slices.Contains(candidates, p) {
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
	args := []string{"build", "nixpkgs#rsync", "--no-link", "--print-out-paths"}
	if e.hasNixConfig() {
		args = append(args, "--inputs-from", e.nixDir())
	}
	out, err := exec.Command(nix, nixArgs(args...)...).Output()
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

func tolerableExit(err error) bool {
	var exit *exec.ExitError
	return errors.As(err, &exit) && (exit.ExitCode() == 23 || exit.ExitCode() == 24)
}

type rsyncItem struct {
	code string
	size int64
	name string
}

func (i rsyncItem) deletion() bool { return strings.HasPrefix(i.code, "*deleting") }
func (i rsyncItem) file() bool     { return strings.HasPrefix(i.code, ">f") }
func (i rsyncItem) isNew() bool    { return strings.HasPrefix(i.code, ">f+++") }

func parseItem(line string) (rsyncItem, bool) {
	if len(line) < 12 {
		return rsyncItem{}, false
	}
	size, name, found := strings.Cut(strings.TrimLeft(line[11:], " "), " ")
	if !found {
		return rsyncItem{}, false
	}
	n, err := strconv.ParseInt(size, 10, 64)
	if err != nil {
		return rsyncItem{}, false
	}
	return rsyncItem{code: line[:11], size: n, name: name}, true
}

func dryRun(bin string, args []string, each func(rsyncItem)) error {
	cmd := exec.Command(bin, append(slices.Clone(args), "--dry-run", "--no-h", "--out-format=%i %l %n")...)
	out, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	var stderr strings.Builder
	cmd.Stderr = &stderr
	if err := cmd.Start(); err != nil {
		return err
	}
	sc := bufio.NewScanner(out)
	sc.Buffer(make([]byte, 64*1024), 1024*1024)
	for sc.Scan() {
		if it, ok := parseItem(sc.Text()); ok {
			each(it)
		}
	}
	io.Copy(io.Discard, out)
	if err := cmd.Wait(); err != nil && !tolerableExit(err) {
		return fmt.Errorf("rsync preview failed: %v %s", err, truncate(strings.TrimSpace(stderr.String()), 300))
	}
	return nil
}

type rsyncProgress struct {
	transferred string
	percent     int
	rate        string
	files       string
}

var progressLine = regexp.MustCompile(`^\s*([\d.,]+[KMGTP]?)\s+(\d+)%\s+(\S+/s)\s+\d+:\d\d:\d\d(?:\s+\(xfr#(\d+))?`)

func parseProgress(line string) (rsyncProgress, bool) {
	m := progressLine.FindStringSubmatch(line)
	if m == nil {
		return rsyncProgress{}, false
	}
	pct, _ := strconv.Atoi(m[2])
	return rsyncProgress{transferred: m[1], percent: pct, rate: m[3], files: m[4]}, true
}

type rsyncJob struct {
	bin     string
	args    []string
	logPath string
	strict  bool
}

type rsyncResult struct {
	exitCode int
	duration time.Duration
	logPath  string
	strict   bool
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

func (r rsyncResult) ok() bool {
	return r.exitCode == 0 || r.exitCode == 24 || (r.exitCode == 23 && !r.strict)
}

func (j rsyncJob) run(ctx context.Context, onProgress func(rsyncProgress), onLine func(string)) (rsyncResult, error) {
	start := time.Now()
	if err := os.MkdirAll(filepath.Dir(j.logPath), 0o755); err != nil {
		return rsyncResult{}, err
	}
	log, err := os.Create(j.logPath)
	if err != nil {
		return rsyncResult{}, err
	}
	defer log.Close()
	args := append(slices.Clone(j.args), "--info=progress2")
	fmt.Fprintf(log, "%s %s\n", j.bin, strings.Join(args, " "))
	cmd := exec.CommandContext(ctx, j.bin, args...)
	cmd.Cancel = func() error { return cmd.Process.Signal(os.Interrupt) }
	cmd.WaitDelay = 10 * time.Second
	pr, pw := io.Pipe()
	cmd.Stdout = pw
	cmd.Stderr = pw
	if err := cmd.Start(); err != nil {
		return rsyncResult{}, err
	}
	var wg sync.WaitGroup
	wg.Go(func() {
		sc := bufio.NewScanner(pr)
		sc.Buffer(make([]byte, 64*1024), 1024*1024)
		sc.Split(splitCRLF)
		for sc.Scan() {
			line := sc.Text()
			if p, ok := parseProgress(line); ok {
				onProgress(p)
				continue
			}
			fmt.Fprintln(log, line)
			if strings.TrimSpace(line) != "" {
				onLine(line)
			}
		}
		io.Copy(io.Discard, pr)
	})
	err = cmd.Wait()
	pw.Close()
	wg.Wait()
	res := rsyncResult{duration: time.Since(start), logPath: j.logPath, strict: j.strict}
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

func writeTemp(pattern string, data []byte) (string, error) {
	f, err := os.CreateTemp("", pattern)
	if err != nil {
		return "", err
	}
	defer f.Close()
	if _, err := f.Write(data); err != nil {
		os.Remove(f.Name())
		return "", err
	}
	return f.Name(), nil
}
