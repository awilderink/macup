package main

import (
	"errors"
	"os/exec"
)

type guard struct {
	problem  string
	question string
	fixTitle string
	fix      func()
}

type check func() (*guard, error)

func precomputed(g *guard) check { return func() (*guard, error) { return g, nil } }

func resolveGuards(checks []check, yes bool) error {
	for _, c := range checks {
		g, err := c()
		if err != nil {
			return err
		}
		if g == nil {
			continue
		}
		warn("%s", g.problem)
		if yes {
			continue
		}
		if !interactive() {
			return errors.New("stopped for the reason above; add --yes to continue anyway")
		}
		if g.fix != nil {
			do, err := confirm(g.fixTitle, "", true)
			if err != nil {
				return err
			}
			if do {
				g.fix()
				return errCancelled
			}
		}
		if err := confirmOrCancel(g.question, "", false); err != nil {
			return err
		}
	}
	return nil
}

func openSettings(pane string) func() {
	return func() { exec.Command("open", pane).Run() }
}
