use crate::config::Preset;
use crate::errors::cancelled;
use crate::selection::{CheckState, Selection, existing_paths, join_rel};
use crate::sizer::Sizer;
use crate::ui::{human_bytes, truncate};
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

struct Node {
    name: String,
    rel: String,
    dir: bool,
    size: u64,
    depth: usize,
    expanded: bool,
    loaded: bool,
    children: Vec<usize>,
}

pub struct Picker {
    root: PathBuf,
    nodes: Vec<Node>,
    rows: Vec<usize>,
    cursor: usize,
    offset: usize,
    width: usize,
    height: usize,
    pub sel: Selection,
    sizes: Sizer,
    presets: Vec<Preset>,
    showing_presets: bool,
    pub status: String,
    pub confirmed: bool,
    pub done: bool,
    sel_size: u64,
    sel_complete: bool,
}

impl Picker {
    pub fn new(root: &Path, presets: Vec<Preset>) -> Result<Picker> {
        let top = Node {
            name: String::new(),
            rel: String::new(),
            dir: true,
            size: 0,
            depth: 0,
            expanded: true,
            loaded: false,
            children: vec![],
        };
        let mut p = Picker {
            root: root.to_path_buf(),
            nodes: vec![top],
            rows: vec![],
            cursor: 0,
            offset: 0,
            width: 80,
            height: 24,
            sel: Selection::new(root),
            sizes: Sizer::new(4),
            presets,
            showing_presets: false,
            status: String::new(),
            confirmed: false,
            done: false,
            sel_size: 0,
            sel_complete: true,
        };
        p.load(0)?;
        p.flatten();
        p.refresh_selected_size();
        Ok(p)
    }

    #[cfg(test)]
    pub fn row_names(&self) -> Vec<&str> {
        self.rows
            .iter()
            .map(|&i| self.nodes[i].rel.as_str())
            .collect()
    }

    #[cfg(test)]
    pub fn cursor_rel(&self) -> &str {
        &self.nodes[self.rows[self.cursor]].rel
    }

    #[cfg(test)]
    pub fn is_expanded(&self, rel: &str) -> bool {
        self.nodes.iter().any(|n| n.rel == rel && n.expanded)
    }

    fn abs(&self, i: usize) -> PathBuf {
        self.root.join(&self.nodes[i].rel)
    }

    fn load(&mut self, i: usize) -> Result<()> {
        if self.nodes[i].loaded {
            return Ok(());
        }
        let mut children = Vec::new();
        for entry in std::fs::read_dir(self.abs(i))? {
            let entry = entry?;
            let dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let size = if dir {
                0
            } else {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            children.push(Node {
                rel: join_rel(&self.nodes[i].rel, &name),
                name,
                dir,
                size,
                depth: if i == 0 { 0 } else { self.nodes[i].depth + 1 },
                expanded: false,
                loaded: false,
                children: vec![],
            });
        }
        children.sort_by(|a, b| {
            b.dir
                .cmp(&a.dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let mut ids = Vec::new();
        for child in children {
            if child.dir {
                self.sizes.request(&self.root.join(&child.rel));
            }
            self.nodes.push(child);
            ids.push(self.nodes.len() - 1);
        }
        self.nodes[i].children = ids;
        self.nodes[i].loaded = true;
        Ok(())
    }

    fn flatten(&mut self) {
        let mut rows = Vec::new();
        let mut stack: Vec<usize> = self.nodes[0].children.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            rows.push(i);
            if self.nodes[i].dir && self.nodes[i].expanded {
                stack.extend(self.nodes[i].children.iter().rev());
            }
        }
        self.rows = rows;
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }

    fn parent_row(&self, row: usize) -> Option<usize> {
        let depth = self.nodes[self.rows[row]].depth;
        (0..row)
            .rev()
            .find(|&j| self.nodes[self.rows[j]].depth < depth)
    }

    fn list_height(&self) -> usize {
        self.height.saturating_sub(6).max(3)
    }

    pub fn handle_key(&mut self, key: &str) {
        self.status.clear();
        if self.showing_presets {
            self.handle_preset_key(key);
        } else {
            self.handle_list_key(key);
        }
        self.refresh_selected_size();
    }

    fn handle_preset_key(&mut self, key: &str) {
        self.showing_presets = false;
        let Some(preset) = key
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| self.presets.get(i).cloned())
        else {
            return;
        };
        let found = existing_paths(&self.root, &preset.paths);
        for rel in &found {
            let _ = self.sel.add(rel);
        }
        self.status = format!(
            "{}: {} of {} paths found in the backup",
            preset.name,
            found.len(),
            preset.paths.len()
        );
    }

    fn handle_list_key(&mut self, key: &str) {
        if matches!(key, "q" | "esc" | "ctrl+c") {
            self.done = true;
            return;
        }
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let page = self.list_height().saturating_sub(1).max(1);
        let current = self.rows[self.cursor];
        match key {
            "enter" => {
                if self.sel.paths.is_empty() {
                    self.status =
                        "nothing selected — press space to select a file or folder".into();
                } else {
                    self.confirmed = true;
                    self.done = true;
                }
            }
            "up" | "k" => self.cursor = self.cursor.saturating_sub(1),
            "down" | "j" => self.cursor = (self.cursor + 1).min(last),
            "pgup" | "ctrl+u" => self.cursor = self.cursor.saturating_sub(page),
            "pgdown" | "ctrl+d" => self.cursor = (self.cursor + page).min(last),
            "home" | "g" => self.cursor = 0,
            "end" | "G" => self.cursor = last,
            "right" | "l" => {
                if !self.nodes[current].dir {
                    return;
                }
                if self.nodes[current].expanded && !self.nodes[current].children.is_empty() {
                    self.cursor += 1;
                    return;
                }
                if let Err(err) = self.load(current) {
                    self.status = err.to_string();
                    return;
                }
                self.nodes[current].expanded = true;
                self.flatten();
            }
            "left" | "h" => {
                if self.nodes[current].dir && self.nodes[current].expanded {
                    self.nodes[current].expanded = false;
                    self.flatten();
                } else if let Some(parent) = self.parent_row(self.cursor) {
                    self.cursor = parent;
                }
            }
            " " | "x" => {
                let rel = self.nodes[current].rel.clone();
                if let Err(err) = self.sel.toggle(&rel) {
                    self.status = err.to_string();
                }
                if key == " " {
                    self.cursor = (self.cursor + 1).min(last);
                }
            }
            "a" => {
                let parent = self
                    .parent_row(self.cursor)
                    .map(|r| self.rows[r])
                    .unwrap_or(0);
                let children: Vec<String> = self.nodes[parent]
                    .children
                    .iter()
                    .map(|&c| self.nodes[c].rel.clone())
                    .collect();
                let all = children
                    .iter()
                    .all(|c| self.sel.state(c) == CheckState::Checked);
                for c in &children {
                    let _ = if all {
                        self.sel.remove(c)
                    } else {
                        self.sel.add(c)
                    };
                }
            }
            "c" => self.sel.clear(),
            "p" if self.presets.is_empty() => {
                self.status =
                    "no presets yet; add them as [[restore.presets]] in macup.toml".into();
            }
            "p" => self.showing_presets = true,
            _ => {}
        }
    }

    pub fn refresh_selected_size(&mut self) {
        let mut total = 0;
        let mut complete = true;
        for rel in &self.sel.paths {
            let abs = self.root.join(rel);
            let Ok(meta) = std::fs::symlink_metadata(&abs) else {
                continue;
            };
            if !meta.is_dir() {
                total += meta.len();
            } else if let Some(s) = self.sizes.get(&abs) {
                total += s;
            } else {
                complete = false;
                self.sizes.request(&abs);
            }
        }
        self.sel_size = total;
        self.sel_complete = complete;
    }

    fn size_label(&self, i: usize) -> String {
        let n = &self.nodes[i];
        if !n.dir {
            return human_bytes(n.size);
        }
        self.sizes
            .get(&self.abs(i))
            .map(human_bytes)
            .unwrap_or_else(|| "…".into())
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if area.width > 0 && area.height > 0 {
            self.width = area.width as usize;
            self.height = area.height as usize;
        }
        let accent = Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD);
        let dim = Style::new().add_modifier(Modifier::DIM);
        let mut lines = vec![
            Line::styled(format!("Restore from {}", self.root.display()), accent),
            Line::styled(
                "space select · → open · ← close · a all here · p presets · c clear · enter continue · q quit",
                dim,
            ),
            Line::raw(""),
        ];
        if self.showing_presets {
            lines.push(Line::raw("Add a preset to the selection:"));
            lines.push(Line::raw(""));
            for (i, p) in self.presets.iter().enumerate() {
                lines.push(Line::from(vec![
                    Span::raw(format!("  {}  {}  ", i + 1, p.name)),
                    Span::styled(p.paths.join(" "), dim),
                ]));
            }
            lines.push(Line::styled("  any other key to go back", dim));
            frame.render_widget(Paragraph::new(lines), area);
            return;
        }
        let h = self.list_height();
        if self.cursor < self.offset {
            self.offset = self.cursor;
        }
        if self.cursor >= self.offset + h {
            self.offset = self.cursor + 1 - h;
        }
        let size_w = 9;
        for row in self.offset..self.rows.len().min(self.offset + h) {
            let i = self.rows[row];
            let n = &self.nodes[i];
            let (box_text, box_style) = match self.sel.state(&n.rel) {
                CheckState::Checked => ("[x]", Style::new().fg(Color::Green)),
                CheckState::Partial => ("[-]", Style::new().fg(Color::Yellow)),
                CheckState::Unchecked => ("[ ]", Style::new()),
            };
            let arrow = match (n.dir, n.expanded) {
                (true, true) => "▾ ",
                (true, false) => "▸ ",
                _ => "  ",
            };
            let name = if n.dir {
                format!("{}/", n.name)
            } else {
                n.name.clone()
            };
            let left = format!("{}{}", "  ".repeat(n.depth), arrow);
            let name_w = self
                .width
                .saturating_sub(left.chars().count() + size_w + 8)
                .max(10);
            let mut line = Line::from(vec![
                Span::raw(left),
                Span::styled(box_text, box_style),
                Span::raw(format!(
                    " {:<name_w$} {:>size_w$}",
                    truncate(&name, name_w),
                    self.size_label(i)
                )),
            ]);
            if row == self.cursor {
                line = line.style(Style::new().add_modifier(Modifier::REVERSED));
            }
            lines.push(line);
        }
        if self.rows.is_empty() {
            lines.push(Line::styled("  (empty)", dim));
        }
        lines.push(Line::raw(""));
        if !self.status.is_empty() {
            lines.push(Line::styled(
                self.status.clone(),
                Style::new().fg(Color::Yellow),
            ));
        } else {
            let mut label = human_bytes(self.sel_size);
            if !self.sel_complete {
                label = format!("≥ {label} (still measuring)");
            }
            lines.push(Line::styled(
                format!("{} selected · {label}", self.sel.paths.len()),
                dim,
            ));
        }
        frame.render_widget(Paragraph::new(lines), area);
    }
}

fn key_name(code: KeyCode, mods: KeyModifiers) -> String {
    match code {
        KeyCode::Char(c) if mods.contains(KeyModifiers::CONTROL) => format!("ctrl+{c}"),
        KeyCode::Char(' ') => " ".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::PageUp => "pgup".into(),
        KeyCode::PageDown => "pgdown".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        _ => String::new(),
    }
}

pub fn pick_paths(root: &Path, presets: Vec<Preset>) -> Result<Vec<String>> {
    let mut picker = Picker::new(root, presets)?;
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        while !picker.done {
            terminal.draw(|f| picker.render(f))?;
            if event::poll(Duration::from_millis(100))?
                && let Event::Key(k) = event::read()?
                && k.kind == KeyEventKind::Press
            {
                picker.handle_key(&key_name(k.code, k.modifiers));
            }
            if picker.sizes.dirty.swap(false, Ordering::Relaxed) {
                picker.refresh_selected_size();
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result?;
    if !picker.confirmed {
        return Err(cancelled());
    }
    Ok(picker.sel.sorted())
}

#[cfg(test)]
#[path = "picker_tests.rs"]
mod tests;
