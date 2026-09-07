use crate::model::{Action, State};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::{Color, Style, Stylize},
    text::Line,
    widgets::{Block, Clear, Paragraph},
};

pub struct TabMenu {
    id: Option<String>,
    name: String,
    position: (u16, u16),
    renaming: bool,
    replace: bool,
    menu_index: usize,
}
impl TabMenu {
    pub fn new(state: &State, tab: usize, position: (u16, u16), renaming: bool) -> Self {
        Self {
            id: state.tabs.get(tab).map(|t| t.id.clone()),
            name: title(state, tab),
            position,
            renaming,
            replace: true,
            menu_index: 0,
        }
    }
    pub fn create() -> Self {
        Self {
            id: None,
            name: String::new(),
            position: (0, 0),
            renaming: true,
            replace: false,
            menu_index: 0,
        }
    }
    fn area(&self, screen: Rect) -> Rect {
        if self.renaming {
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
                self.position.1.min(screen.bottom().saturating_sub(4)),
                width,
                4.min(screen.height),
            )
        }
    }
    fn choose(&mut self) -> (bool, Option<Action>) {
        if self.menu_index == 1 {
            return (true, self.id.clone().map(Action::CloseTab));
        }
        self.renaming = true;
        (false, None)
    }
    fn save(&self) -> Option<Action> {
        let name = self.name.trim();
        if name.is_empty() {
            return None;
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
            if self.name.chars().count() >= 48 {
                break;
            }
            self.name.push(c);
        }
    }
    pub fn event(&mut self, event: Event, screen: Rect) -> (bool, Option<Action>) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Esc => return (true, None),
                KeyCode::Enter if self.renaming => {
                    let action = self.save();
                    return (action.is_some(), action);
                }
                KeyCode::Enter => return self.choose(),
                KeyCode::Up if !self.renaming => self.menu_index = 0,
                KeyCode::Down if !self.renaming => self.menu_index = 1,
                KeyCode::Backspace if self.renaming => {
                    if self.replace {
                        self.name.clear();
                        self.replace = false;
                    } else {
                        self.name.pop();
                    }
                }
                KeyCode::Char('u' | 'c')
                    if self.renaming && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    self.name.clear();
                    self.replace = false;
                }
                KeyCode::Char(c)
                    if self.renaming
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.insert(&c.to_string())
                }
                _ => {}
            },
            Event::Paste(text) if self.renaming => self.insert(&text),
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                let area = self.area(screen);
                if !area.contains((mouse.column, mouse.row).into()) {
                    return (true, None);
                }
                if !self.renaming {
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
        if self.renaming {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(if self.id.is_some() {
                        "rename tab"
                    } else {
                        "new tab"
                    })
                    .bold(),
                    Line::from(""),
                    Line::from(if self.name.is_empty() {
                        " "
                    } else {
                        &self.name
                    })
                    .bg(Color::Rgb(38, 43, 64)),
                    Line::from(""),
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
            let lines = ["Rename…", "Close tab"]
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
