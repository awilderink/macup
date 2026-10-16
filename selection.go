package main

import (
	"os"
	"path"
	"path/filepath"
	"sort"
	"strings"
)

type checkState int

const (
	unchecked checkState = iota
	partial
	checked
)

type selection struct {
	paths map[string]bool
	list  func(rel string) ([]string, error)
}

func newSelection(root string) *selection {
	return &selection{paths: map[string]bool{}, list: dirLister(root)}
}

func dirLister(root string) func(string) ([]string, error) {
	return func(rel string) ([]string, error) {
		entries, err := os.ReadDir(filepath.Join(root, rel))
		if err != nil {
			return nil, err
		}
		names := make([]string, 0, len(entries))
		for _, e := range entries {
			names = append(names, joinRel(rel, e.Name()))
		}
		return names, nil
	}
}

func joinRel(dir, name string) string {
	if dir == "" {
		return name
	}
	return dir + "/" + name
}

func parentRel(rel string) string {
	d := path.Dir(rel)
	if d == "." {
		return ""
	}
	return d
}

func isUnder(p, dir string) bool {
	if dir == "" {
		return p != ""
	}
	return strings.HasPrefix(p, dir+"/")
}

func (s *selection) coveringAncestor(rel string) (string, bool) {
	for p := rel; p != ""; p = parentRel(p) {
		if s.paths[p] {
			return p, true
		}
	}
	return "", false
}

func (s *selection) state(rel string) checkState {
	if _, ok := s.coveringAncestor(rel); ok {
		return checked
	}
	for p := range s.paths {
		if isUnder(p, rel) {
			return partial
		}
	}
	return unchecked
}

func (s *selection) toggle(rel string) error {
	if _, ok := s.coveringAncestor(rel); ok {
		return s.remove(rel)
	}
	return s.add(rel)
}

func (s *selection) add(rel string) error {
	if _, ok := s.coveringAncestor(rel); ok {
		return nil
	}
	for p := range s.paths {
		if isUnder(p, rel) {
			delete(s.paths, p)
		}
	}
	s.paths[rel] = true
	return s.collapse(parentRel(rel))
}

func (s *selection) collapse(dir string) error {
	if dir == "" {
		return nil
	}
	children, err := s.list(dir)
	if err != nil || len(children) == 0 {
		return err
	}
	for _, c := range children {
		if !s.paths[c] {
			return nil
		}
	}
	for _, c := range children {
		delete(s.paths, c)
	}
	s.paths[dir] = true
	return s.collapse(parentRel(dir))
}

func (s *selection) remove(rel string) error {
	anc, ok := s.coveringAncestor(rel)
	if !ok {
		return nil
	}
	delete(s.paths, anc)
	for cur := anc; cur != rel; {
		next := joinRel(cur, strings.SplitN(strings.TrimPrefix(rel, cur+"/"), "/", 2)[0])
		children, err := s.list(cur)
		if err != nil {
			return err
		}
		for _, c := range children {
			if c != next {
				s.paths[c] = true
			}
		}
		cur = next
	}
	return nil
}

func (s *selection) clear() { s.paths = map[string]bool{} }

func (s *selection) sorted() []string {
	out := make([]string, 0, len(s.paths))
	for p := range s.paths {
		out = append(out, p)
	}
	sort.Strings(out)
	return out
}
