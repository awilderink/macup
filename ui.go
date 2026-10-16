package main

import (
	"context"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/charmbracelet/bubbles/progress"
	"github.com/charmbracelet/bubbles/spinner"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/huh"
	hspinner "github.com/charmbracelet/huh/spinner"
	"github.com/charmbracelet/lipgloss"
	"github.com/mattn/go-isatty"
)

var (
	accent      = lipgloss.AdaptiveColor{Light: "#5A3FC0", Dark: "#B4A0FF"}
	styleTitle  = lipgloss.NewStyle().Bold(true).Foreground(accent)
	styleDim    = lipgloss.NewStyle().Foreground(lipgloss.AdaptiveColor{Light: "#777777", Dark: "#8A8A8A"})
	styleOK     = lipgloss.NewStyle().Foreground(lipgloss.AdaptiveColor{Light: "#1F7A3A", Dark: "#6BD68B"})
	styleWarn   = lipgloss.NewStyle().Foreground(lipgloss.AdaptiveColor{Light: "#9A6200", Dark: "#F2C14E"})
	styleError  = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.AdaptiveColor{Light: "#B3261E", Dark: "#FF8A80"})
	styleCursor = lipgloss.NewStyle().Background(lipgloss.AdaptiveColor{Light: "#E6E0FF", Dark: "#3A3160"})
)

func interactive() bool {
	return isatty.IsTerminal(os.Stdin.Fd()) && isatty.IsTerminal(os.Stdout.Fd())
}

func step(format string, a ...any) { fmt.Println(styleTitle.Render("==> ") + fmt.Sprintf(format, a...)) }
func ok(format string, a ...any)   { fmt.Println(styleOK.Render("✓ ") + fmt.Sprintf(format, a...)) }
func warn(format string, a ...any) { fmt.Println(styleWarn.Render("! ") + fmt.Sprintf(format, a...)) }
func note(format string, a ...any) { fmt.Println(styleDim.Render("  " + fmt.Sprintf(format, a...))) }

func withSpinner(title string, action func() error) error {
	if !interactive() {
		return action()
	}
	var err error
	if serr := hspinner.New().Title(" " + title).Action(func() { err = action() }).Run(); serr != nil {
		return serr
	}
	return err
}

func confirm(title, description string, def bool) (bool, error) {
	if !interactive() {
		return def, nil
	}
	answer := def
	err := huh.NewConfirm().Title(title).Description(description).Value(&answer).Run()
	return answer, err
}

func confirmOrCancel(title, description string, def bool) error {
	yes, err := confirm(title, description, def)
	if err != nil {
		return err
	}
	if !yes {
		return errCancelled
	}
	return nil
}

type progressMsg rsyncProgress
type lineMsg string
type jobDoneMsg struct {
	res rsyncResult
	err error
}

type jobModel struct {
	title      string
	bar        progress.Model
	spin       spinner.Model
	prog       rsyncProgress
	warnings   []string
	warnCount  int
	cancel     context.CancelFunc
	cancelling bool
	start      time.Time
	done       *jobDoneMsg
}

func (m jobModel) Init() tea.Cmd { return m.spin.Tick }

func (m jobModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		if msg.String() == "ctrl+c" || msg.String() == "q" || msg.String() == "esc" {
			m.cancelling = true
			m.cancel()
		}
	case tea.WindowSizeMsg:
		m.bar.Width = min(60, max(20, msg.Width-30))
	case progressMsg:
		m.prog = rsyncProgress(msg)
	case lineMsg:
		m.warnCount++
		m.warnings = append(m.warnings, string(msg))
		if len(m.warnings) > 4 {
			m.warnings = m.warnings[len(m.warnings)-4:]
		}
	case jobDoneMsg:
		m.done = &msg
		return m, tea.Quit
	case spinner.TickMsg:
		var cmd tea.Cmd
		m.spin, cmd = m.spin.Update(msg)
		return m, cmd
	}
	return m, nil
}

func (m jobModel) View() string {
	if m.done != nil {
		return ""
	}
	var b strings.Builder
	b.WriteString(m.spin.View() + " " + styleTitle.Render(m.title) + "\n\n")
	if m.prog.transferred != "" {
		b.WriteString("  " + m.bar.ViewAs(float64(m.prog.percent)/100) + "\n")
		info := fmt.Sprintf("  %s copied · %s", m.prog.transferred, m.prog.rate)
		if m.prog.files != "" {
			info += fmt.Sprintf(" · %s files", m.prog.files)
		}
		info += fmt.Sprintf(" · %s elapsed", time.Since(m.start).Round(time.Second))
		b.WriteString(styleDim.Render(info) + "\n")
	} else {
		b.WriteString(styleDim.Render("  comparing files…") + "\n")
	}
	if m.warnCount > 0 {
		b.WriteString("\n" + styleWarn.Render(fmt.Sprintf("  %d messages from rsync (full list in the log):", m.warnCount)) + "\n")
		for _, w := range m.warnings {
			b.WriteString(styleDim.Render("  "+truncate(w, 100)) + "\n")
		}
	}
	if m.cancelling {
		b.WriteString("\n" + styleWarn.Render("  stopping…") + "\n")
	} else {
		b.WriteString("\n" + styleDim.Render("  q / ctrl+c to stop safely") + "\n")
	}
	return b.String()
}

func runJob(title string, job rsyncJob) (rsyncResult, error) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if !interactive() {
		step("%s", title)
		last := time.Time{}
		return job.run(ctx, func(p rsyncProgress) {
			if time.Since(last) > 5*time.Second {
				last = time.Now()
				fmt.Printf("  %s  %d%%  %s\n", p.transferred, p.percent, p.rate)
			}
		}, func(l string) { fmt.Println("  " + l) })
	}
	sp := spinner.New()
	sp.Spinner = spinner.MiniDot
	sp.Style = lipgloss.NewStyle().Foreground(accent)
	m := jobModel{title: title, bar: progress.New(progress.WithDefaultGradient(), progress.WithWidth(50)), spin: sp, cancel: cancel, start: time.Now()}
	p := tea.NewProgram(m)
	go func() {
		last := time.Time{}
		res, err := job.run(ctx, func(pr rsyncProgress) {
			if time.Since(last) >= 100*time.Millisecond {
				last = time.Now()
				p.Send(progressMsg(pr))
			}
		}, func(l string) { p.Send(lineMsg(l)) })
		p.Send(jobDoneMsg{res: res, err: err})
	}()
	final, err := p.Run()
	if err != nil {
		return rsyncResult{}, err
	}
	done := final.(jobModel).done
	if done == nil {
		return rsyncResult{exitCode: 20}, nil
	}
	return done.res, done.err
}

func reportResult(what string, res rsyncResult) error {
	line := fmt.Sprintf("%s %s in %s", what, res.summary(), res.duration.Round(time.Second))
	switch {
	case res.exitCode == 0:
		ok("%s", line)
	case res.ok():
		warn("%s", line)
	default:
		fmt.Println(styleError.Render("✗ ") + line)
	}
	note("log: %s", res.logPath)
	switch {
	case res.exitCode == 20:
		return errCancelled
	case !res.ok():
		return errSilent
	}
	return nil
}

func truncate(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n-1]) + "…"
}

func humanBytes(n int64) string {
	const unit = 1000
	if n < unit {
		return fmt.Sprintf("%d B", n)
	}
	div, exp := int64(unit), 0
	for m := n / unit; m >= unit; m /= unit {
		div *= unit
		exp++
	}
	return fmt.Sprintf("%.1f %cB", float64(n)/float64(div), "kMGTPE"[exp])
}

func plural(n int, one string, many ...string) string {
	if n == 1 {
		return "1 " + one
	}
	if len(many) > 0 {
		return fmt.Sprintf("%d %s", n, many[0])
	}
	return fmt.Sprintf("%d %ss", n, one)
}
