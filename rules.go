package main

import _ "embed"

//go:embed backup.rules
var backupRules []byte

func writeRulesFile() (string, error) { return writeTemp("macup-backup-*.rules", backupRules) }
