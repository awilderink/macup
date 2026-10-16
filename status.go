package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

func cmdStatus(e env, _ []string) error {
	row := func(label, value string) { fmt.Printf("%s %s\n", styleTitle.Render(fmt.Sprintf("%-14s", label)), value) }
	fmt.Println(styleTitle.Render("macup status"))
	mounted := checkMounted(e.volume) == nil
	if !mounted {
		row("Backup drive", styleWarn.Render("not connected"))
	} else {
		v := e.volume + " · connected"
		if free, total, err := freeSpace(e.volume); err == nil && total > 0 {
			v += fmt.Sprintf(" · %s free of %s", humanBytes(int64(free)), humanBytes(int64(total)))
			if nearlyFull(free, total) {
				v = styleWarn.Render(v + " · nearly full")
			}
		}
		row("Backup drive", v)
	}
	row("Last backup", lastBackup(e))
	row("Photos", lastPhotoBackup(e))
	switch granted, known := hasFullDiskAccess(e.home); {
	case !known:
		row("Disk access", "unknown")
	case granted:
		row("Disk access", terminalName()+" has Full Disk Access")
	default:
		row("Disk access", styleWarn.Render(terminalName()+" lacks Full Disk Access — backups will miss protected folders"))
	}
	if nix := findNix(); nix != "" {
		row("Nix", nixVersion(nix))
	} else {
		row("Nix", styleWarn.Render("not installed — run macup apply"))
	}
	if gen, err := os.Readlink("/run/current-system"); err == nil {
		row("System", filepath.Base(gen))
	} else {
		row("System", styleWarn.Render("nix-darwin not applied yet"))
	}
	switch {
	case !e.hasNixConfig():
		row("Config", styleWarn.Render("missing — macup apply copies it from the drive"))
	case !mounted:
		row("Config", e.nixDir())
	case nixConfigInSync(e):
		row("Config", e.nixDir()+" · same as drive copy")
	default:
		row("Config", e.nixDir()+styleWarn.Render(" · differs from drive copy (saved on next apply or backup)"))
	}
	return nil
}

func firstLine(b []byte) string {
	line, _, _ := strings.Cut(strings.TrimSpace(string(b)), "\n")
	return line
}
