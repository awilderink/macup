package main

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
)

type preset struct {
	name  string
	paths []string
}

var presets = []preset{
	{"Shell: zsh, Oh My Zsh, Cargo env", []string{".zshrc", ".zprofile", ".zshenv", ".oh-my-zsh", ".cargo/env"}},
	{"Claude Code config", []string{".claude"}},
	{"SSH keys and config", []string{".ssh"}},
	{"Nix config", []string{".config/nix-darwin"}},
}

type node struct {
	name     string
	rel      string
	dir      bool
	size     int64
	depth    int
	expanded bool
	loaded   bool
	children []*node
}

type sizeTickMsg struct{}

type picker struct {
	root      string
	top       *node
	rows      []*node
	cursor    int
	offset    int
	width     int
	height    int
	sel       *selection
	sizes     *sizer
	presets   bool
	status    string
	confirmed bool
}

func newPicker(root string, sel *selection) (*picker, error) {
	top := &node{rel: "", dir: true, depth: -1, expanded: true}
	p := &picker{root: root, top: top, sel: sel, sizes: newSizer(4), height: 20, width: 80}
	if err := p.load(top); err != nil {
		return nil, err
	}
	p.flatten()
	return p, nil
}

func (p *picker) abs(n *node) string { return filepath.Join(p.root, n.rel) }

func (p *picker) load(n *node) error {
	if n.loaded {
		return nil
	}
	entries, err := os.ReadDir(p.abs(n))
	if err != nil {
		return err
	}
	n.children = n.children[:0]
	for _, e := range entries {
		c := &node{name: e.Name(), rel: joinRel(n.rel, e.Name()), dir: e.IsDir(), depth: n.depth + 1}
		if !c.dir {
			if info, err := e.Info(); err == nil {
				c.size = info.Size()
			}
		}
		n.children = append(n.children, c)
	}
	sort.SliceStable(n.children, func(i, j int) bool {
		a, b := n.children[i], n.children[j]
		if a.dir != b.dir {
			return a.dir
		}
		return strings.ToLower(a.name) < strings.ToLower(b.name)
	})
	for i := len(n.children) - 1; i >= 0; i-- {
		if n.children[i].dir {
			p.sizes.request(p.abs(n.children[i]))
		}
	}
	n.loaded = true
	return nil
}

func (p *picker) flatten() {
	p.rows = p.rows[:0]
	var walk func(n *node)
	walk = func(n *node) {
		for _, c := range n.children {
			p.rows = append(p.rows, c)
			if c.dir && c.expanded {
				walk(c)
			}
		}
	}
	walk(p.top)
	p.cursor = min(p.cursor, max(0, len(p.rows)-1))
}

func (p *picker) parentIndex(i int) int {
	d := p.rows[i].depth
	for j := i - 1; j >= 0; j-- {
		if p.rows[j].depth < d {
			return j
		}
	}
	return -1
}

func (p *picker) Init() tea.Cmd { return p.waitSizes() }

func (p *picker) waitSizes() tea.Cmd {
	ch := p.sizes.updates
	return func() tea.Msg {
		<-ch
		return sizeTickMsg{}
	}
}

func (p *picker) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		if msg.Width > 0 && msg.Height > 0 {
			p.width, p.height = msg.Width, msg.Height
		}
	case sizeTickMsg:
		return p, p.waitSizes()
	case tea.KeyMsg:
		p.status = ""
		if p.presets {
			return p, p.handlePresetKey(msg.String())
		}
		return p, p.handleKey(msg.String())
	}
	return p, nil
}

func (p *picker) handlePresetKey(key string) tea.Cmd {
	p.presets = false
	if key < "1" || key > "9" {
		return nil
	}
	i := int(key[0] - '1')
	if i >= len(presets) {
		return nil
	}
	added := 0
	for _, rel := range presets[i].paths {
		if _, err := os.Lstat(filepath.Join(p.root, rel)); err == nil {
			p.sel.add(rel)
			added++
		}
	}
	p.status = fmt.Sprintf("%s: %d of %d paths found in the backup", presets[i].name, added, len(presets[i].paths))
	return nil
}

func (p *picker) handleKey(key string) tea.Cmd {
	if len(p.rows) == 0 && key != "q" && key != "esc" && key != "ctrl+c" {
		return nil
	}
	page := max(1, p.listHeight()-1)
	switch key {
	case "ctrl+c", "q", "esc":
		p.sizes.close()
		return tea.Quit
	case "enter":
		if len(p.sel.paths) == 0 {
			p.status = "nothing selected — press space to select a file or folder"
			return nil
		}
		p.confirmed = true
		p.sizes.close()
		return tea.Quit
	case "up", "k":
		p.cursor = max(0, p.cursor-1)
	case "down", "j":
		p.cursor = min(len(p.rows)-1, p.cursor+1)
	case "pgup", "ctrl+u":
		p.cursor = max(0, p.cursor-page)
	case "pgdown", "ctrl+d":
		p.cursor = min(len(p.rows)-1, p.cursor+page)
	case "home", "g":
		p.cursor = 0
	case "end", "G":
		p.cursor = len(p.rows) - 1
	case "right", "l":
		n := p.rows[p.cursor]
		if !n.dir {
			return nil
		}
		if n.expanded && len(n.children) > 0 {
			p.cursor++
			return nil
		}
		if err := p.load(n); err != nil {
			p.status = err.Error()
			return nil
		}
		n.expanded = true
		p.flatten()
	case "left", "h":
		n := p.rows[p.cursor]
		if n.dir && n.expanded {
			n.expanded = false
			p.flatten()
		} else if i := p.parentIndex(p.cursor); i >= 0 {
			p.cursor = i
		}
	case " ", "x":
		if err := p.sel.toggle(p.rows[p.cursor].rel); err != nil {
			p.status = err.Error()
		}
		if key == " " {
			p.cursor = min(len(p.rows)-1, p.cursor+1)
		}
	case "a":
		parent := p.top
		if i := p.parentIndex(p.cursor); i >= 0 {
			parent = p.rows[i]
		}
		all := true
		for _, c := range parent.children {
			if p.sel.state(c.rel) != checked {
				all = false
			}
		}
		for _, c := range parent.children {
			if all {
				p.sel.remove(c.rel)
			} else {
				p.sel.add(c.rel)
			}
		}
	case "c":
		p.sel.clear()
	case "p":
		p.presets = true
	}
	return nil
}

func (p *picker) listHeight() int { return max(3, p.height-6) }

func (p *picker) sizeLabel(n *node) string {
	if !n.dir {
		return humanBytes(n.size)
	}
	if s, ok := p.sizes.get(p.abs(n)); ok {
		return humanBytes(s)
	}
	return "…"
}

func (p *picker) selectedSize() (int64, bool) {
	var total int64
	complete := true
	for rel := range p.sel.paths {
		abs := filepath.Join(p.root, rel)
		info, err := os.Lstat(abs)
		if err != nil {
			continue
		}
		if !info.IsDir() {
			total += info.Size()
			continue
		}
		if s, ok := p.sizes.get(abs); ok {
			total += s
		} else {
			complete = false
			p.sizes.request(abs)
		}
	}
	return total, complete
}

func (p *picker) View() string {
	var b strings.Builder
	b.WriteString(styleTitle.Render("Restore from "+p.root) + "\n")
	b.WriteString(styleDim.Render("space select · → open · ← close · a all here · p presets · c clear · enter continue · q quit") + "\n\n")
	if p.presets {
		b.WriteString("Add a preset to the selection:\n\n")
		for i, pr := range presets {
			b.WriteString(fmt.Sprintf("  %d  %s  %s\n", i+1, pr.name, styleDim.Render(strings.Join(pr.paths, " "))))
		}
		b.WriteString(styleDim.Render("\n  any other key to go back") + "\n")
		return b.String()
	}
	h := p.listHeight()
	if p.cursor < p.offset {
		p.offset = p.cursor
	}
	if p.cursor >= p.offset+h {
		p.offset = p.cursor - h + 1
	}
	sizeW := 9
	for i := p.offset; i < min(len(p.rows), p.offset+h); i++ {
		n := p.rows[i]
		box := "[ ]"
		switch p.sel.state(n.rel) {
		case checked:
			box = styleOK.Render("[x]")
		case partial:
			box = styleWarn.Render("[-]")
		}
		arrow := "  "
		name := n.name
		if n.dir {
			arrow = "▸ "
			if n.expanded {
				arrow = "▾ "
			}
			name += "/"
		}
		left := strings.Repeat("  ", n.depth) + arrow
		nameW := max(10, p.width-len(left)-sizeW-8)
		line := fmt.Sprintf("%s%s %-*s %*s", left, box, nameW, truncate(name, nameW), sizeW, p.sizeLabel(n))
		if i == p.cursor {
			line = styleCursor.Render(line)
		}
		b.WriteString(line + "\n")
	}
	if len(p.rows) == 0 {
		b.WriteString(styleDim.Render("  (empty)") + "\n")
	}
	b.WriteString("\n")
	if p.status != "" {
		b.WriteString(styleWarn.Render(p.status) + "\n")
	} else {
		size, complete := p.selectedSize()
		label := humanBytes(size)
		if !complete {
			label = "≥ " + label + " (still measuring)"
		}
		b.WriteString(styleDim.Render(fmt.Sprintf("%d selected · %s", len(p.sel.paths), label)) + "\n")
	}
	return b.String()
}

func pickPaths(root string) ([]string, error) {
	sel := newSelection(root)
	p, err := newPicker(root, sel)
	if err != nil {
		return nil, err
	}
	if _, err := tea.NewProgram(p, tea.WithAltScreen()).Run(); err != nil {
		return nil, err
	}
	if !p.confirmed {
		return nil, errCancelled
	}
	return sel.sorted(), nil
}
