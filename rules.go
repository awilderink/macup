package main

import (
	_ "embed"
	"os"
)

//go:embed backup.rules
var backupRules []byte

func writeRulesFile() (string, error) {
	f, err := os.CreateTemp("", "macup-backup-*.rules")
	if err != nil {
		return "", err
	}
	defer f.Close()
	if _, err := f.Write(backupRules); err != nil {
		os.Remove(f.Name())
		return "", err
	}
	return f.Name(), nil
}
