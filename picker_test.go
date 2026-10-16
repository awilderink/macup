package main

import (
	"reflect"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

func press(p *picker, keys ...string) {
	for _, k := range keys {
		var msg tea.KeyMsg
		switch k {
		case "space":
			msg = tea.KeyMsg{Type: tea.KeySpace, Runes: []rune(" ")}
		case "right":
			msg = tea.KeyMsg{Type: tea.KeyRight}
		case "left":
			msg = tea.KeyMsg{Type: tea.KeyLeft}
		case "down":
			msg = tea.KeyMsg{Type: tea.KeyDown}
		case "enter":
			msg = tea.KeyMsg{Type: tea.KeyEnter}
		default:
			msg = tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune(k)}
		}
		p.Update(msg)
	}
}

func cursorName(p *picker) string { return p.rows[p.cursor].rel }

// Folders sort before files and dotfolders are shown, because a home backup's
// most valuable restore targets (.ssh, .claude) are hidden directories.
func TestPickerListsFoldersFirstIncludingDotfolders(t *testing.T) {
	p, err := newPicker(fixtureTree(t), &selection{paths: map[string]bool{}})
	if err != nil {
		t.Fatal(err)
	}
	defer p.sizes.close()
	var names []string
	for _, r := range p.rows {
		names = append(names, r.rel)
	}
	if want := []string{".ssh", "Documents", ".zshrc"}; !reflect.DeepEqual(names, want) {
		t.Fatalf("rows = %v, want %v", names, want)
	}
}

func TestPickerExpandSelectAndConfirm(t *testing.T) {
	root := fixtureTree(t)
	sel := newSelection(root)
	p, err := newPicker(root, sel)
	if err != nil {
		t.Fatal(err)
	}
	press(p, "down", "right")
	if !p.rows[1].expanded {
		t.Fatal("Documents did not expand")
	}
	press(p, "down")
	if got := cursorName(p); got != "Documents/sub" {
		t.Fatalf("cursor on %s, want Documents/sub", got)
	}
	press(p, "space")
	if got := sel.sorted(); !reflect.DeepEqual(got, []string{"Documents/sub"}) {
		t.Fatalf("selection = %v", got)
	}
	view := p.View()
	if !strings.Contains(view, "[-]") || !strings.Contains(view, "[x]") {
		t.Fatalf("expected partial parent and checked child in view:\n%s", view)
	}
	press(p, "enter")
	if !p.confirmed {
		t.Fatal("enter did not confirm a non-empty selection")
	}
}

func TestPickerRefusesToContinueWithNothingSelected(t *testing.T) {
	p, err := newPicker(fixtureTree(t), &selection{paths: map[string]bool{}})
	if err != nil {
		t.Fatal(err)
	}
	defer p.sizes.close()
	press(p, "enter")
	if p.confirmed || !strings.Contains(p.status, "nothing selected") {
		t.Fatalf("confirmed=%v status=%q", p.confirmed, p.status)
	}
}

func TestPickerPresetSelectsOnlyPathsPresentInBackup(t *testing.T) {
	root := fixtureTree(t)
	sel := newSelection(root)
	p, err := newPicker(root, sel)
	if err != nil {
		t.Fatal(err)
	}
	defer p.sizes.close()
	press(p, "p", "1")
	if got := sel.sorted(); !reflect.DeepEqual(got, []string{".zshrc"}) {
		t.Fatalf("selection = %v, want only .zshrc from the shell preset", got)
	}
}

func TestSizerMeasuresNestedDirectories(t *testing.T) {
	root := fixtureTree(t)
	s := newSizer(2)
	defer s.close()
	if got := s.walk(root+"/Documents", 0); got != int64(len("Documents/a.txt")+len("Documents/b.txt")+len("Documents/sub/c.txt")) {
		t.Fatalf("size = %d", got)
	}
	if _, ok := s.get(root + "/Documents/sub"); !ok {
		t.Fatal("subdirectory size was not cached during the parent walk")
	}
}
