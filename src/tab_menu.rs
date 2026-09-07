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
}
impl TabMenu {
    pub fn new(state: &State, tab: usize, position: (u16, u16), renaming: bool) -> Self {
        Self {
            id: tab
                .checked_sub(1)
                .and_then(|i| state.chunks.get(i))
                .map(|c| c.id.clone()),
            name: title(state, tab),
            position,
            renaming,
            replace: true,
        }
    }
    fn area(&self, screen: Rect) -> Rect {
        if self.renaming {
            let width = screen.width.saturating_sub(2).min(60);
            Rect::new(
                screen.x + (screen.width - width) / 2,
                screen.y + screen.height.saturating_sub(5) / 2,
                width,
                5.min(screen.height),
            )
        } else {
            let width = 18.min(screen.width);
            Rect::new(
                self.position.0.min(screen.right().saturating_sub(width)),
                self.position.1.min(screen.bottom().saturating_sub(3)),
                width,
                3.min(screen.height),
            )
        }
    }
    fn save(&self) -> Option<Action> {
        let name = self.name.trim();
        (!name.is_empty()).then(|| Action::RenameTab {
            id: self.id.clone(),
            name: name.into(),
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
                KeyCode::Enter => self.renaming = true,
                KeyCode::Backspace if self.renaming => {
                    if self.replace {
                        self.name.clear();
                        self.replace = false;
                    } else {
                        self.name.pop();
                    }
                }
                KeyCode::Char('u')
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
                    if mouse.row == area.y + 1 {
                        self.renaming = true;
                    }
                } else if mouse.row == area.y + 3 {
                    if mouse.column < area.x + 10 {
                        let action = self.save();
                        return (action.is_some(), action);
                    }
                    return (true, None);
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
                    Line::from(self.name.clone()).bg(Color::Rgb(38, 66, 61)),
                    Line::from(""),
                    Line::from("[Save]   [Cancel]"),
                ])
                .block(block.title(" Rename tab · Enter saves ")),
                area,
            );
        } else {
            frame.render_widget(Paragraph::new("Rename…").block(block), area);
        }
    }
}
pub fn title(state: &State, tab: usize) -> String {
    if tab == 0 {
        return if state.transcript_title.is_empty() {
            "All".into()
        } else {
            state.transcript_title.clone()
        };
    }
    state.chunks.get(tab - 1).map_or_else(
        || "All".into(),
        |chunk| {
            if chunk.title.is_empty() {
                format!("Chunk {tab}")
            } else {
                chunk.title.clone()
            }
        },
    )
}
