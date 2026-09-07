use crate::{
    config::{Acceleration, Provider},
    model::{Action, BackendSettings},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::{Color, Style, Stylize},
    text::Line,
    widgets::{Block, Clear, Paragraph},
};

#[derive(Clone, Copy)]
enum Field {
    Provider,
    Acceleration,
    Binary,
    LocalModel,
    OpenAiModel,
    ApiKey,
    Microphone,
    Save,
}

pub struct SettingsView {
    settings: BackendSettings,
    selected: usize,
    editing: bool,
    key: String,
    key_changed: bool,
}
impl SettingsView {
    pub fn new(settings: BackendSettings) -> Self {
        Self {
            settings,
            selected: 0,
            editing: false,
            key: String::new(),
            key_changed: false,
        }
    }
    fn fields(&self) -> &'static [Field] {
        match self.settings.provider {
            Provider::Local => &[
                Field::Provider,
                Field::Acceleration,
                Field::Binary,
                Field::LocalModel,
                Field::Microphone,
                Field::Save,
            ],
            Provider::OpenAi => &[
                Field::Provider,
                Field::OpenAiModel,
                Field::ApiKey,
                Field::Microphone,
                Field::Save,
            ],
        }
    }
    fn text(&mut self) -> Option<&mut String> {
        match self.fields()[self.selected] {
            Field::Binary => Some(&mut self.settings.binary),
            Field::LocalModel => Some(&mut self.settings.model),
            Field::OpenAiModel => Some(&mut self.settings.openai_model),
            Field::ApiKey => {
                self.key_changed = true;
                Some(&mut self.key)
            }
            _ => None,
        }
    }
    pub fn paste(&mut self, text: &str) {
        if self.editing
            && let Some(field) = self.text()
        {
            field.extend(
                text.chars()
                    .filter(|c| !c.is_control())
                    .take(2048usize.saturating_sub(field.len())),
            );
        }
    }
    /// Returns (close dialog, optional command). Secrets never enter public state.
    pub fn key(&mut self, key: KeyEvent) -> (bool, Option<Action>) {
        if self.editing {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => self.editing = false,
                KeyCode::Backspace => {
                    if let Some(field) = self.text() {
                        field.pop();
                    }
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(field) = self.text() {
                        field.clear();
                    }
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.paste(&c.to_string());
                }
                _ => {}
            }
            return (false, None);
        }
        match key.code {
            KeyCode::Esc => return (true, None),
            KeyCode::Up | KeyCode::BackTab => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => {
                self.selected = (self.selected + 1).min(self.fields().len() - 1)
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') => {
                match self.fields()[self.selected] {
                    Field::Provider => {
                        self.settings.provider = match self.settings.provider {
                            Provider::Local => Provider::OpenAi,
                            Provider::OpenAi => Provider::Local,
                        }
                    }
                    Field::Acceleration => {
                        self.settings.acceleration = match self.settings.acceleration {
                            Acceleration::Auto => Acceleration::Cpu,
                            Acceleration::Cpu => Acceleration::Vulkan,
                            Acceleration::Vulkan => Acceleration::Auto,
                        }
                    }
                    Field::Microphone => return (false, Some(Action::Devices)),
                    Field::Save => {
                        return (
                            true,
                            Some(Action::ConfigureBackend {
                                settings: self.settings.clone(),
                                api_key: self.key_changed.then(|| std::mem::take(&mut self.key)),
                            }),
                        );
                    }
                    _ => self.editing = true,
                }
            }
            _ => {}
        }
        (false, None)
    }
    fn area(&self, screen: Rect) -> Rect {
        let width = screen.width.saturating_sub(2).min(90);
        let height = screen.height.min(self.fields().len() as u16 + 5);
        Rect::new(
            screen.x + (screen.width - width) / 2,
            screen.y + (screen.height - height) / 2,
            width,
            height,
        )
    }
    pub fn mouse(&mut self, event: MouseEvent, screen: Rect) -> (bool, Option<Action>) {
        let area = self.area(screen);
        let inside = area.contains((event.column, event.row).into());
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if !inside
                    || (event.row == area.y && event.column >= area.right().saturating_sub(9))
                {
                    return (true, None);
                }
                if event.row == area.y || event.row == area.bottom() - 1 {
                    return (false, None);
                }
                let index = (event.row - area.y - 1) as usize;
                if index < self.fields().len() {
                    self.editing = false;
                    self.selected = index;
                    return self.key(KeyEvent::from(KeyCode::Enter));
                }
            }
            MouseEventKind::ScrollUp if inside && !self.editing => {
                self.selected = self.selected.saturating_sub(1);
            }
            MouseEventKind::ScrollDown if inside && !self.editing => {
                self.selected = (self.selected + 1).min(self.fields().len() - 1);
            }
            _ => {}
        }
        (false, None)
    }
    pub fn draw(&self, frame: &mut ratatui::Frame, device: &str) {
        let area = self.area(frame.area());
        frame.render_widget(Clear, area);
        let block = Block::bordered()
            .title(" Settings ")
            .title(Line::from("[Close]").right_aligned())
            .title_bottom(if self.editing {
                " Enter done · Ctrl-U erase · paste supported "
            } else {
                " ↑↓ select · Enter change · Esc close "
            })
            .style(
                Style::default()
                    .bg(Color::Rgb(18, 23, 30))
                    .fg(Color::Rgb(185, 239, 214)),
            );
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let key = if self.key_changed {
            if self.key.is_empty() {
                "(remove saved key)".into()
            } else {
                "•".repeat(self.key.len().min(24))
            }
        } else if self.settings.api_key_configured {
            "(configured — enter to replace)".into()
        } else {
            "(not configured)".into()
        };
        let mut lines = self
            .fields()
            .iter()
            .map(|field| match field {
                Field::Provider => format!(
                    "Provider       ‹ {} ›",
                    match self.settings.provider {
                        Provider::Local => "Local Whisper",
                        Provider::OpenAi => "OpenAI",
                    }
                ),
                Field::Acceleration => {
                    format!("Acceleration   ‹ {:?} ›", self.settings.acceleration)
                }
                Field::Binary => format!("Whisper binary {}", self.settings.binary),
                Field::LocalModel => format!("Whisper model  {}", self.settings.model),
                Field::OpenAiModel => format!("Model          {}", self.settings.openai_model),
                Field::ApiKey => format!("API key        {key}"),
                Field::Microphone => format!("Microphone     {device}"),
                Field::Save => "Save settings".into(),
            })
            .enumerate()
            .map(|(i, row)| {
                let prefix = if i == self.selected {
                    if self.editing { "✎ " } else { "› " }
                } else {
                    "  "
                };
                let line = Line::from(format!("{prefix}{row}"));
                if i == self.selected {
                    line.bg(Color::Rgb(38, 66, 61)).bold()
                } else {
                    line
                }
            })
            .collect::<Vec<_>>();
        lines.push(Line::from(""));
        lines.push(Line::from(match self.settings.provider {
            Provider::Local => "Local processing. Vulkan requires a Vulkan-enabled Whisper binary.",
            Provider::OpenAi => "OpenAI uploads recorded chunks using your API account.",
        }));
        lines.push(Line::from(
            "Changes apply to the next transcription; active work continues.",
        ));
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn mouse_switches_provider_and_opens_microphone_picker() {
        let mut view = SettingsView::new(BackendSettings::default());
        let screen = Rect::new(0, 0, 100, 30);
        let click = |area: Rect, row: u16| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x + 3,
            row: area.y + row,
            modifiers: KeyModifiers::NONE,
        };
        let local = view.area(screen);
        view.mouse(click(local, 1), screen);
        assert_eq!(view.settings.provider, Provider::OpenAi);
        assert!(
            !view
                .fields()
                .iter()
                .any(|f| matches!(f, Field::Acceleration | Field::Binary))
        );
        let cloud = view.area(screen);
        let (closed, action) = view.mouse(click(cloud, 4), screen);
        assert!(!closed);
        assert!(matches!(action, Some(Action::Devices)));
    }
    #[test]
    fn key_entry_is_masked_and_only_sent_on_save() {
        let mut view = SettingsView::new(BackendSettings {
            provider: Provider::OpenAi,
            ..Default::default()
        });
        view.selected = 2;
        view.key(KeyEvent::from(KeyCode::Enter));
        view.paste("sk-test-secret");
        // Normal application shortcuts must remain text inside the editor.
        view.key(KeyEvent::from(KeyCode::Char('q')));
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| view.draw(f, "System default")).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(!text.contains("sk-test-secret"));
        assert!(text.contains("••••"));
        view.key(KeyEvent::from(KeyCode::Enter));
        view.selected = view.fields().len() - 1;
        let (closed, action) = view.key(KeyEvent::from(KeyCode::Enter));
        assert!(closed);
        let Some(Action::ConfigureBackend { api_key, settings }) = action else {
            panic!("Expected save")
        };
        assert_eq!(api_key.as_deref(), Some("sk-test-secretq"));
        assert!(
            !serde_json::to_string(&settings)
                .unwrap()
                .contains("sk-test")
        );
    }
}
