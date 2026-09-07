use crate::model::{Action, State};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::{Color, Style, Stylize},
    text::Line,
    widgets::{Block, Clear, Paragraph},
};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Menu,
    Name,
    Export,
}

pub struct TabMenu {
    id: Option<String>,
    name: String,
    position: (u16, u16),
    mode: Mode,
    error: Option<String>,
    replace: bool,
    menu_index: usize,
}
impl TabMenu {
    pub fn new(state: &State, tab: usize, position: (u16, u16), renaming: bool) -> Self {
        Self {
            id: state.tabs.get(tab).map(|t| t.id.clone()),
            name: title(state, tab),
            position,
            mode: if renaming { Mode::Name } else { Mode::Menu },
            error: None,
            replace: true,
            menu_index: 0,
        }
    }
    pub fn create() -> Self {
        Self {
            id: None,
            name: String::new(),
            position: (0, 0),
            mode: Mode::Name,
            error: None,
            replace: false,
            menu_index: 0,
        }
    }
    fn area(&self, screen: Rect) -> Rect {
        if self.mode != Mode::Menu {
            let width = screen.width.saturating_sub(2).min(60);
            Rect::new(
                screen.x + (screen.width - width) / 2,
                screen.y + screen.height.saturating_sub(7) / 2,
                width,
                7.min(screen.height),
            )
        } else {
            let width = 18.min(screen.width);
            Rect::new(
                self.position.0.min(screen.right().saturating_sub(width)),
                self.position.1.min(screen.bottom().saturating_sub(5)),
                width,
                5.min(screen.height),
            )
        }
    }
    pub fn export(state: &State, tab: usize) -> Self {
        let mut dialog = Self::new(state, tab, (0, 0), false);
        dialog.begin_export();
        dialog
    }
    fn begin_export(&mut self) {
        self.mode = Mode::Export;
        self.name = crate::export::suggested_path(&self.name);
        self.replace = true;
    }
    fn choose(&mut self) -> (bool, Option<Action>) {
        match self.menu_index {
            1 => self.begin_export(),
            2 => return (true, self.id.clone().map(Action::CloseTab)),
            _ => self.mode = Mode::Name,
        }
        (false, None)
    }
    fn save(&mut self) -> Option<Action> {
        let name = self.name.trim();
        if name.is_empty() {
            return None;
        }
        if self.mode == Mode::Export {
            return match crate::export::resolve_path(name) {
                Ok(path) => self.id.clone().map(|id| Action::ExportTab { id, path }),
                Err(error) => {
                    self.error = Some(error.to_string());
                    None
                }
            };
        }
        Some(match &self.id {
            Some(id) => Action::RenameTab {
                id: id.clone(),
                name: name.into(),
            },
            None => Action::CreateTab(name.into()),
        })
    }
    fn insert(&mut self, text: &str) {
        if self.replace {
            self.name.clear();
            self.replace = false;
        }
        for c in text.chars().filter(|c| !c.is_control()) {
            if self.name.chars().count() >= if self.mode == Mode::Export { 2048 } else { 48 } {
                break;
            }
            self.name.push(c);
        }
    }
    pub fn event(&mut self, event: Event, screen: Rect) -> (bool, Option<Action>) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Esc => return (true, None),
                KeyCode::Enter if self.mode != Mode::Menu => {
                    let action = self.save();
                    return (action.is_some(), action);
                }
                KeyCode::Enter => return self.choose(),
                KeyCode::Up if self.mode == Mode::Menu => {
                    self.menu_index = self.menu_index.saturating_sub(1)
                }
                KeyCode::Down if self.mode == Mode::Menu => {
                    self.menu_index = (self.menu_index + 1).min(2)
                }
                KeyCode::Backspace if self.mode != Mode::Menu => {
                    if self.replace {
                        self.name.clear();
                        self.replace = false;
                    } else {
                        self.name.pop();
                    }
                }
                KeyCode::Char('u' | 'c')
                    if self.mode != Mode::Menu && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    self.name.clear();
                    self.replace = false;
                }
                KeyCode::Char(c)
                    if self.mode != Mode::Menu
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.insert(&c.to_string())
                }
                _ => {}
            },
            Event::Paste(text) if self.mode != Mode::Menu => self.insert(&text),
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                let area = self.area(screen);
                if !area.contains((mouse.column, mouse.row).into()) {
                    return (true, None);
                }
                if self.mode == Mode::Menu {
                    if mouse.row > area.y && mouse.row < area.bottom() - 1 {
                        self.menu_index = (mouse.row - area.y - 1) as usize;
                        return self.choose();
                    }
                } else if mouse.row == area.y + 5 {
                    if mouse.column < area.x + 12 {
                        let action = self.save();
                        return (action.is_some(), action);
                    }
                    if mouse.column < area.x + 24 {
                        self.name.clear();
                        self.replace = false;
                    } else {
                        return (true, None);
                    }
                }
            }
            _ => {}
        }
        (false, None)
    }
    pub fn draw(&self, frame: &mut ratatui::Frame) {
        let area = self.area(frame.area());
        frame.render_widget(Clear, area);
        let style = Style::default()
            .bg(Color::Rgb(18, 23, 30))
            .fg(Color::Rgb(185, 239, 214));
        let block = Block::bordered().style(style);
        if self.mode != Mode::Menu {
            let visible = self
                .name
                .chars()
                .rev()
                .take(area.width.saturating_sub(3) as usize)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<String>();
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(if self.mode == Mode::Export {
                        "export transcript · .txt"
                    } else if self.id.is_some() {
                        "rename tab"
                    } else {
                        "new tab"
                    })
                    .bold(),
                    Line::from(""),
                    Line::from(if self.name.is_empty() { " " } else { &visible })
                        .bg(Color::Rgb(38, 43, 64)),
                    Line::from(self.error.as_deref().unwrap_or("")),
                    Line::from(vec![
                        ratatui::text::Span::styled(
                            " ↵ save ",
                            Style::default()
                                .bg(Color::Rgb(124, 160, 247))
                                .fg(Color::Rgb(18, 23, 30))
                                .bold(),
                        ),
                        ratatui::text::Span::raw("    ^C clear    esc cancel"),
                    ]),
                ])
                .block(block.border_style(Style::default().fg(Color::Rgb(124, 160, 247)))),
                area,
            );
            frame.set_cursor_position((
                (area.x + 1 + self.name.chars().count() as u16).min(area.right().saturating_sub(2)),
                area.y + 3,
            ));
        } else {
            let lines = ["Rename…", "Export…", "Close tab"]
                .iter()
                .enumerate()
                .map(|(i, label)| {
                    let line = Line::from(*label);
                    if i == self.menu_index {
                        line.bg(Color::Rgb(38, 66, 61)).bold()
                    } else {
                        line
                    }
                })
                .collect::<Vec<_>>();
            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
    }
}
pub fn title(state: &State, tab: usize) -> String {
    state
        .tabs
        .get(tab)
        .map_or_else(|| "Transcript 1".into(), |tab| tab.name.clone())
}
