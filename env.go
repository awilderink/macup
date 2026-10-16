package main

import (
	"fmt"
	"os"
	"path/filepath"
)

type env struct {
	home   string
	user   string
	volume string
	host   string
}

func loadEnv() (env, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return env{}, fmt.Errorf("cannot determine home directory: %w", err)
	}
	home = envOr("MACUP_HOME", home)
	return env{
		home:   home,
		user:   filepath.Base(home),
		volume: envOr("MACUP_VOLUME", "/Volumes/Backup"),
		host:   envOr("MACUP_HOST", "Arvins-MacBook-Pro"),
	}, nil
}

func envOr(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

func (e env) backupsDir() string   { return filepath.Join(e.volume, "Users") }
func (e env) backupHome() string   { return filepath.Join(e.backupsDir(), e.user) }
func (e env) bootstrapDir() string { return filepath.Join(e.volume, "MacBootstrap") }
func (e env) driveNixDir() string  { return filepath.Join(e.bootstrapDir(), "nix-darwin") }
func (e env) nixDir() string       { return filepath.Join(e.home, ".config", "nix-darwin") }
func (e env) logDir() string       { return filepath.Join(e.home, ".backup-logs") }
func (e env) lockDir() string      { return filepath.Join(e.volume, ".backup.lock") }
