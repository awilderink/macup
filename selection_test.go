package main

import (
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

func fixtureTree(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	for _, f := range []string{
		".zshrc",
		".ssh/id_ed25519",
		".ssh/config",
		"Documents/a.txt",
		"Documents/b.txt",
		"Documents/sub/c.txt",
	} {
		p := filepath.Join(root, f)
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(f), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return root
}

func TestSelectingADirectoryCoversItsSubtree(t *testing.T) {
	s := newSelection(fixtureTree(t))
	s.toggle("Documents")
	if got := s.state("Documents/sub/c.txt"); got != checked {
		t.Fatalf("descendant state = %v, want checked", got)
	}
	if got := s.sorted(); !reflect.DeepEqual(got, []string{"Documents"}) {
		t.Fatalf("paths = %v", got)
	}
}

func TestSelectingADirectoryDropsRedundantDescendants(t *testing.T) {
	s := newSelection(fixtureTree(t))
	s.toggle("Documents/sub/c.txt")
	s.toggle("Documents")
	if got := s.sorted(); !reflect.DeepEqual(got, []string{"Documents"}) {
		t.Fatalf("paths = %v", got)
	}
}

// Unchecking one file inside a checked folder must keep its siblings (and the
// siblings of every folder on the way down) selected, so nothing else is lost.
func TestUncheckingInsideACheckedDirectoryKeepsSiblings(t *testing.T) {
	s := newSelection(fixtureTree(t))
	s.toggle("Documents")
	s.toggle("Documents/sub/c.txt")
	want := []string{"Documents/a.txt", "Documents/b.txt"}
	if got := s.sorted(); !reflect.DeepEqual(got, want) {
		t.Fatalf("paths = %v, want %v", got, want)
	}
	if got := s.state("Documents"); got != partial {
		t.Fatalf("parent state = %v, want partial", got)
	}
	if got := s.state("Documents/sub"); got != unchecked {
		t.Fatalf("emptied dir state = %v, want unchecked", got)
	}
}

// Selecting every child one by one should collapse to the parent so the rsync
// list stays minimal and new files in that folder are included too.
func TestSelectingAllChildrenCollapsesToParent(t *testing.T) {
	s := newSelection(fixtureTree(t))
	s.toggle(".ssh/id_ed25519")
	s.toggle(".ssh/config")
	if got := s.sorted(); !reflect.DeepEqual(got, []string{".ssh"}) {
		t.Fatalf("paths = %v", got)
	}
}

func TestTopLevelNeverCollapsesIntoRoot(t *testing.T) {
	s := newSelection(fixtureTree(t))
	for _, p := range []string{".zshrc", ".ssh", "Documents"} {
		s.toggle(p)
	}
	if got := s.sorted(); len(got) != 3 {
		t.Fatalf("paths = %v, want the three top-level entries", got)
	}
}

func TestToggleTwiceRestoresEmptySelection(t *testing.T) {
	s := newSelection(fixtureTree(t))
	s.toggle("Documents/a.txt")
	s.toggle("Documents/a.txt")
	if got := s.sorted(); len(got) != 0 {
		t.Fatalf("paths = %v, want none", got)
	}
}
