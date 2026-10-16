package main

import (
	"fmt"
	"os"
	"path/filepath"
)

type env struct {
	home   string
	volume string
}

func loadEnv() (env, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return env{}, fmt.Errorf("cannot determine home directory: %w", err)
	}
	return env{
		home:   envOr("MACUP_HOME", home),
		volume: envOr("MACUP_VOLUME", "/Volumes/Backup"),
	}, nil
}

func envOr(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

func (e env) user() string        { return filepath.Base(e.home) }
func (e env) backupHome() string  { return filepath.Join(e.volume, "home") }
func (e env) photosDir() string   { return filepath.Join(e.volume, "photos") }
func (e env) driveNixDir() string { return filepath.Join(e.volume, "nix-darwin") }
func (e env) nixDir() string      { return filepath.Join(e.home, ".config", "nix-darwin") }
func (e env) logDir() string      { return filepath.Join(e.home, ".backup-logs") }
func (e env) lockDir() string     { return filepath.Join(e.volume, ".backup.lock") }
func (e env) lastBackupFile() string {
	return filepath.Join(e.logDir(), "last-backup")
}

func (e env) hasNixConfig() bool { return hasFlake(e.nixDir()) }

func hasFlake(dir string) bool {
	_, err := os.Stat(filepath.Join(dir, "flake.nix"))
	return err == nil
}
