use crate::{
    config::Config,
    ipc::Client,
    logo,
    model::{Action, ChunkStatus, Phase, State, timestamp},
    selection::Selection,
    settings_ui::SettingsView,
    tab_menu::{self, TabMenu},
};
use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Bar, BarChart, BarGroup, Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};
const BG: Color = Color::Rgb(18, 23, 30);
const MUTED: Color = Color::Rgb(134, 150, 165);
const MINT: Color = Color::Rgb(185, 239, 214);

pub fn run(config: Config) -> Result<()> {
    // The interface owns its dark palette, including under inherited NO_COLOR.
    crossterm::style::force_color_output(true);
    let client = Client::new(config);
    let mut terminal = ratatui::init();
    crossterm::execute!(
        std::io::stdout(),
        event::EnableBracketedPaste,
        event::EnableMouseCapture
    )?;
    let result = interact(&mut terminal, &client);
    let _ = crossterm::execute!(
        std::io::stdout(),
        event::DisableBracketedPaste,
        event::DisableMouseCapture
    );
    ratatui::restore();
    result
}
#[derive(Clone)]
enum Hit {
    Key(KeyEvent),
    Tab(usize),
    SelectTab(String),
    Microphone(usize),
}
#[derive(Default)]
struct View {
    settings: Option<SettingsView>,
    tab_menu: Option<TabMenu>,
    hits: Vec<(Rect, Hit)>,
    tab: usize,
    transcript_area: Rect,
    selection: Option<Selection>,
    text_cells: Vec<Vec<String>>,
    copies: u64,
    copied_until: Option<Instant>,
    scroll: u16,
    confirm: bool,
    microphone: Option<usize>,
    levels: VecDeque<u64>,
    welcome_started: Option<Instant>,
    completed: HashMap<String, Option<Instant>>,
}

fn queue_items(
    state: &State,
    completed: &mut HashMap<String, Option<Instant>>,
    now: Instant,
) -> Vec<(usize, f32)> {
    completed.retain(|id, _| state.chunks.iter().any(|c| c.id == *id));
    state
        .chunks
        .iter()
        .enumerate()
        .filter_map(|(index, chunk)| {
            let opacity = if chunk.status == ChunkStatus::Ready {
                let elapsed = now
                    .duration_since(
                        *completed
                            .entry(chunk.id.clone())
                            .or_insert(Some(now - Duration::from_secs(4)))
                            .get_or_insert(now),
                    )
                    .as_secs_f32();
                (1. - (elapsed - 3.5) / 0.5).clamp(0., 1.)
            } else {
                completed.insert(chunk.id.clone(), None);
                1.
            };
            (opacity > 0.).then_some((index, opacity))
        })
        .collect()
}

fn fade(color: Color, opacity: f32) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    let blend = |value: u8, background: u8| {
        (background as f32 + (value as f32 - background as f32) * opacity) as u8
    };
    Color::Rgb(blend(r, 18), blend(g, 23), blend(b, 30))
}

fn interact(terminal: &mut ratatui::DefaultTerminal, client: &Client) -> Result<()> {
    let mut view = View::default();
    let mut sampled_at = Instant::now();
    loop {
        let (state, error) = client.snapshot();
        if state.transcript.is_empty() && !state.phase.busy() {
            view.welcome_started.get_or_insert_with(Instant::now);
        } else {
            view.welcome_started = None;
        }
        let copies = client.copies();
        if copies != view.copies {
            view.copies = copies;
            view.copied_until = Some(Instant::now() + Duration::from_secs(2));
        }
        let level = if state.phase == Phase::Recording {
            (state.level.sqrt().clamp(0., 1.) * 100.) as u64
        } else {
            0
        };
        if sampled_at.elapsed() >= Duration::from_millis(80) {
            view.levels.push_back(level);
            if view.levels.len() > 160 {
                view.levels.pop_front();
            }
            sampled_at = Instant::now();
        }
        view.tab = state
            .tabs
            .iter()
            .position(|tab| tab.id == state.selected_tab)
            .unwrap_or(0);
        terminal.draw(|f| draw(f, &state, error.as_deref(), &mut view))?;
        let revealing = view
            .welcome_started
            .is_some_and(|start| start.elapsed() < Duration::from_secs(2));
        if event::poll(Duration::from_millis(if revealing { 32 } else { 80 }))? {
            let event = event::read()?;
            if let Some(menu) = &mut view.tab_menu {
                let (close, action) = menu.event(event, terminal.get_frame().area());
                if let Some(action) = action {
                    client.send(action);
                }
                if close {
                    view.tab_menu = None;
                }
                continue;
            }
            if let Event::Paste(text) = &event {
                if let Some(settings) = &mut view.settings {
                    settings.paste(text);
                }
                continue;
            }
            let key = match event {
                Event::Key(key) => key,
                Event::Mouse(mouse) => {
                    if view.microphone.is_none()
                        && let Some(settings) = &mut view.settings
                    {
                        let (close, action) = settings.mouse(mouse, terminal.get_frame().area());
                        if let Some(action) = action {
                            if matches!(action, Action::Devices) {
                                view.microphone = Some(microphone_index(&state));
                            }
                            client.send(action);
                        }
                        if close {
                            view.settings = None;
                        }
                        continue;
                    }
                    match mouse.kind {
                        MouseEventKind::Drag(MouseButton::Left) if view.microphone.is_none() => {
                            if let Some(selection) = &mut view.selection {
                                selection.drag(mouse.column, mouse.row, view.transcript_area);
                            }
                            continue;
                        }
                        MouseEventKind::Up(MouseButton::Left) if view.microphone.is_none() => {
                            if let Some(selection) = &view.selection {
                                let text = selection.text(&view.text_cells, view.transcript_area);
                                if !text.is_empty() {
                                    client.send(Action::CopyText(text));
                                }
                            }
                            continue;
                        }
                        MouseEventKind::Down(MouseButton::Right) if view.microphone.is_none() => {
                            if let Some((_, Hit::Tab(tab))) =
                                view.hits.iter().rev().find(|(rect, _)| {
                                    rect.contains((mouse.column, mouse.row).into())
                                })
                            {
                                view.tab_menu = Some(TabMenu::new(
                                    &state,
                                    *tab,
                                    (mouse.column, mouse.row),
                                    false,
                                ));
                            }
                            continue;
                        }
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                            let up = mouse.kind == MouseEventKind::ScrollUp;
                            if view.microphone.is_some() {
                                KeyEvent::from(if up { KeyCode::Up } else { KeyCode::Down })
                            } else if view
                                .transcript_area
                                .contains((mouse.column, mouse.row).into())
                            {
                                view.selection = None;
                                view.scroll = if up {
                                    view.scroll.saturating_sub(3)
                                } else {
                                    view.scroll.saturating_add(3)
                                };
                                continue;
                            } else {
                                continue;
                            }
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            if view.microphone.is_none()
                                && !state.transcript.is_empty()
                                && view
                                    .transcript_area
                                    .contains((mouse.column, mouse.row).into())
                            {
                                view.selection = Some(Selection::new(mouse.column, mouse.row));
                                continue;
                            }
                            view.selection = None;
                            let hit = view
                                .hits
                                .iter()
                                .rev()
                                .find(|(rect, _)| rect.contains((mouse.column, mouse.row).into()))
                                .map(|(_, hit)| hit.clone());
                            match hit {
                                Some(Hit::Key(key)) => key,
                                Some(Hit::Tab(tab)) => {
                                    view.tab = tab;
                                    if let Some(tab) = state.tabs.get(tab) {
                                        client.send(Action::SelectTab(tab.id.clone()));
                                    }
                                    view.scroll = 0;
                                    continue;
                                }
                                Some(Hit::SelectTab(id)) => {
                                    client.send(Action::SelectTab(id));
                                    view.scroll = 0;
                                    continue;
                                }
                                Some(Hit::Microphone(index)) => {
                                    client.send(Action::SelectDevice(
                                        index
                                            .checked_sub(1)
                                            .and_then(|i| state.devices.get(i).cloned()),
                                    ));
                                    view.microphone = None;
                                    continue;
                                }
                                None => {
                                    view.microphone = None;
                                    continue;
                                }
                            }
                        }
                        _ => continue,
                    }
                }
                _ => continue,
            };

            if key.kind != KeyEventKind::Press {
                continue;
            }
            view.selection = None;
            if view.microphone.is_none()
                && let Some(settings) = &mut view.settings
            {
                let (close, action) = settings.key(key);
                if let Some(action) = action {
                    if matches!(action, Action::Devices) {
                        view.microphone = Some(microphone_index(&state));
                    }
                    client.send(action);
                }
                if close {
                    view.settings = None;
                }
                continue;
            }
            if key.code == KeyCode::Char('q') {
                break;
            }
            if let Some(index) = view.microphone {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('m') => view.microphone = None,
                    KeyCode::Up | KeyCode::Char('k') => {
                        view.microphone = Some(index.saturating_sub(1))
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        view.microphone = Some((index + 1).min(state.devices.len()))
                    }
                    KeyCode::Enter => {
                        client.send(Action::SelectDevice(
                            index
                                .checked_sub(1)
                                .and_then(|i| state.devices.get(i).cloned()),
                        ));
                        view.microphone = None;
                    }
                    _ => {}
                }
                continue;
            }
            let action = match key.code {
                KeyCode::Char('e') if !state.tabs.is_empty() => {
                    view.tab_menu = Some(TabMenu::export(&state, view.tab));
                    None
                }
                KeyCode::Char('t') => Some(Action::ReopenTab),
                KeyCode::Char('+') => {
                    view.tab_menu = Some(TabMenu::create());
                    None
                }
                KeyCode::F(2) if !state.tabs.is_empty() => {
                    view.tab_menu = Some(TabMenu::new(&state, view.tab, (0, 0), true));
                    None
                }
                KeyCode::Char(']') | KeyCode::Tab => {
                    view.tab = (view.tab + 1) % state.tabs.len().max(1);
                    view.scroll = 0;
                    state
                        .tabs
                        .get(view.tab)
                        .map(|tab| Action::SelectTab(tab.id.clone()))
                }
                KeyCode::Char('[') | KeyCode::BackTab => {
                    view.tab = if view.tab == 0 {
                        state.tabs.len().saturating_sub(1)
                    } else {
                        view.tab - 1
                    };
                    view.scroll = 0;
                    state
                        .tabs
                        .get(view.tab)
                        .map(|tab| Action::SelectTab(tab.id.clone()))
                }
                KeyCode::Char('s') => {
                    view.settings = Some(SettingsView::new(state.backend.clone()));
                    None
                }
                KeyCode::Char(' ') => Some(Action::Toggle),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Some(Action::CopyLatest)
                }
                KeyCode::Char('c') => Some(Action::Copy),
                KeyCode::Esc if view.confirm => {
                    view.confirm = false;
                    None
                }
                KeyCode::Esc => Some(Action::Cancel),
                KeyCode::Char('x') if view.confirm => {
                    view.scroll = 0;
                    Some(Action::Clear)
                }
                KeyCode::Char('x') => {
                    view.confirm = true;
                    None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    view.scroll = view
                        .scroll
                        .saturating_add(1)
                        .min(state.transcript.len().min(u16::MAX as usize) as u16);
                    None
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    view.scroll = view.scroll.saturating_sub(1);
                    None
                }
                KeyCode::Home => {
                    view.scroll = 0;
                    None
                }
                _ => None,
            };
            if let Some(action) = action {
                client.send(action);
                view.confirm = false;
            } else if key.code != KeyCode::Char('x') {
                view.confirm = false;
            }
        }
    }
    Ok(())
}
fn draw(frame: &mut ratatui::Frame, state: &State, error: Option<&str>, view: &mut View) {
    view.hits.clear();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(Color::Rgb(234, 239, 242))),
        frame.area(),
    );
    if frame.area().width < 44 || frame.area().height < 14 {
        frame.render_widget(
            Paragraph::new(
                "pasteface\nResize to 44 × 14 or larger.\nSpace record/stop · c copy · q close",
            )
            .wrap(Wrap { trim: false }),
            frame.area(),
        );
        return;
    }
    let shortcuts = shortcuts(frame.area().width.saturating_sub(2));
    let queue_items = queue_items(state, &mut view.completed, Instant::now());
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(if state.phase == Phase::Recording {
            7
        } else {
            4
        }),
        Constraint::Length(1),
        Constraint::Length(shortcuts.len() as u16),
    ])
    .horizontal_margin(1)
    .vertical_margin(0)
    .split(frame.area());
    view.transcript_area = Rect::new(
        rows[1].x,
        rows[1].y + 1,
        rows[1].width,
        rows[1].height.saturating_sub(1),
    );
    transcript_tabs(frame, state, view, rows[0]);
    let transcript = state.transcript.as_str();
    if transcript.is_empty() && !state.phase.busy() {
        welcome(
            frame,
            rows[1],
            view.welcome_started
                .map_or(2., |start| start.elapsed().as_secs_f32()),
        );
    } else {
        let text = if transcript.is_empty() {
            "Your words will appear here after you stop recording."
        } else {
            transcript
        };
        frame.render_widget(
            Paragraph::new(text)
                .style(if transcript.is_empty() {
                    Style::default().fg(MUTED)
                } else {
                    Style::default()
                })
                .wrap(Wrap { trim: false })
                .scroll((view.scroll, 0))
                .block(
                    Block::default().title(
                        Line::from(format!(
                            "{} recorded · {} words",
                            timestamp(
                                state
                                    .chunks
                                    .iter()
                                    .filter(|chunk| chunk.tab_id == state.selected_tab)
                                    .map(|chunk| chunk.seconds)
                                    .sum()
                            ),
                            transcript.split_whitespace().count()
                        ))
                        .fg(MUTED),
                    ),
                ),
            rows[1],
        );
    }
    let text_area = view.transcript_area;
    view.text_cells = (text_area.y..text_area.bottom())
        .map(|y| {
            (text_area.x..text_area.right())
                .map(|x| frame.buffer_mut()[(x, y)].symbol().to_owned())
                .collect()
        })
        .collect();
    if let Some(selection) = &view.selection {
        selection.paint(frame.buffer_mut(), text_area);
    }
    let color = match state.phase {
        Phase::Recording | Phase::Error => Color::Rgb(255, 124, 139),
        Phase::Transcribing => Color::Rgb(243, 199, 125),
        _ => MINT,
    };
    let label = format!(
        " {} {}",
        if state.phase == Phase::Recording {
            "●"
        } else {
            "○"
        },
        state.phase.label()
    );
    let mut status = label
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let gain = if i == 1 && state.phase == Phase::Recording {
                crate::animation::pulse()
            } else if state.phase.busy() {
                crate::animation::shimmer(i, label.len())
            } else {
                1.
            };
            let tint = match color {
                Color::Rgb(r, g, b) => Color::Rgb(
                    (r as f32 * gain) as u8,
                    (g as f32 * gain) as u8,
                    (b as f32 * gain) as u8,
                ),
                _ => color,
            };
            Span::styled(c.to_string(), Style::default().fg(tint).bold())
        })
        .collect::<Vec<_>>();
    let duration = match state.phase {
        Phase::Recording => Some(format!("  {}", timestamp(state.seconds))),
        Phase::Transcribing => state
            .chunks
            .iter()
            .find(|chunk| chunk.status == ChunkStatus::Transcribing)
            .map(|chunk| format!("  {} audio", timestamp(chunk.seconds))),
        _ => None,
    };
    if let Some(duration) = duration {
        status.push(Span::styled(duration, Style::default().fg(color)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(status))
            .block(Block::bordered().border_style(Style::default().fg(Color::Rgb(48, 61, 72)))),
        rows[2],
    );
    view.hits.push((
        Rect::new(
            rows[2].x + 1,
            rows[2].y + 1,
            rows[2].width.saturating_sub(2),
            1,
        ),
        Hit::Key(KeyEvent::from(KeyCode::Char(' '))),
    ));
    if rows[2].height >= 5 {
        let inner = Rect::new(
            rows[2].x + 2,
            rows[2].y + 2,
            rows[2].width.saturating_sub(4),
            rows[2].height - 4,
        );
        let count = (inner.width / 3) as usize;
        let values = view
            .levels
            .iter()
            .rev()
            .take(count)
            .copied()
            .collect::<Vec<_>>();
        let bars = values
            .iter()
            .rev()
            .enumerate()
            .map(|(i, v)| {
                Bar::default()
                    .value(*v)
                    .text_value(String::new())
                    .style(Style::default().fg(logo::gradient(i, values.len())))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            BarChart::default()
                .data(BarGroup::default().bars(&bars))
                .max(100)
                .bar_width(2)
                .bar_gap(1)
                .bar_style(Style::default().fg(MINT)),
            inner,
        );
        // Shade each bar vertically; its height and time history remain unchanged.
        for y in inner.y..inner.bottom() {
            let t = (y - inner.y) as f32 / inner.height.saturating_sub(1).max(1) as f32;
            let tint = Color::Rgb(
                (190. - 110. * t) as u8,
                (245. - 115. * t) as u8,
                (209. + 36. * t) as u8,
            );
            for x in inner.x..inner.right() {
                frame.buffer_mut()[(x, y)].set_fg(tint);
            }
        }
    }
    let mic_area = Rect::new(
        rows[2].x + 2,
        rows[2].bottom().saturating_sub(2),
        rows[2].width.saturating_sub(4),
        1,
    );
    frame.render_widget(
        Paragraph::new(format!("MIC  {}  ·  [S] settings", state.device)).fg(MUTED),
        mic_area,
    );
    view.hits
        .push((mic_area, Hit::Key(KeyEvent::from(KeyCode::Char('s')))));
    let copied = view
        .copied_until
        .is_some_and(|until| Instant::now() < until);
    if copied && error.is_none() && rows[1].height > 0 {
        let label = " ✓ Copied to clipboard ";
        let width = (label.chars().count() as u16).min(rows[1].width);
        let toast = Rect::new(
            rows[1].right().saturating_sub(width),
            rows[1].bottom().saturating_sub(1),
            width,
            1,
        );
        frame.render_widget(
            Paragraph::new(label).style(Style::default().bg(MINT).fg(BG).bold()),
            toast,
        );
    }
    let notice_area = Rect::new(
        rows[1].x,
        rows[1].bottom().saturating_sub(1),
        rows[1].width,
        1,
    );
    let notice = if view.confirm {
        "Clear this tab and its audio? [X] confirm · [Esc] dismiss"
    } else {
        error.unwrap_or(
            if copied
                || (state.phase.busy() && !state.message.starts_with("Exported to "))
                || state.message == "Copied to clipboard."
            {
                ""
            } else {
                &state.message
            },
        )
    };
    frame.render_widget(
        Paragraph::new(notice)
            .fg(if error.is_some() {
                Color::LightRed
            } else if copied {
                MINT
            } else {
                MUTED
            })
            .wrap(Wrap { trim: false }),
        notice_area,
    );
    for (row, line) in shortcuts.iter().enumerate() {
        let text = line.to_string();
        for (label, key) in shortcut_controls() {
            if let Some(start) = text.find(label) {
                view.hits.push((
                    Rect::new(
                        rows[4].x + text[..start].chars().count() as u16,
                        rows[4].y + row as u16,
                        label.chars().count() as u16,
                        1,
                    ),
                    Hit::Key(key),
                ));
            }
        }
    }
    if view.confirm {
        let label = notice;
        for (text, key) in [
            ("[X] confirm", KeyCode::Char('x')),
            ("[Esc] dismiss", KeyCode::Esc),
        ] {
            if let Some(start) = label.find(text) {
                view.hits.push((
                    Rect::new(
                        notice_area.x + label[..start].chars().count() as u16,
                        notice_area.y,
                        text.len() as u16,
                        1,
                    ),
                    Hit::Key(KeyEvent::from(key)),
                ));
            }
        }
    }
    frame.render_widget(Paragraph::new(shortcuts).fg(MINT), rows[4]);
    queue(frame, state, rows[3], &mut view.hits, &queue_items);
    if let Some(settings) = &view.settings {
        settings.draw(frame, &state.device);
    }
    if let Some(index) = view.microphone {
        view.hits.clear();
        microphone_picker(frame, state, index, &mut view.hits);
    }
    if let Some(menu) = &view.tab_menu {
        menu.draw(frame);
    }
}
fn transcript_tabs(frame: &mut ratatui::Frame, state: &State, view: &mut View, area: Rect) {
    let labels = (0..state.tabs.len())
        .map(|tab| {
            let name = tab_menu::title(state, tab);
            if name.chars().count() > 18 {
                format!("{}…", name.chars().take(17).collect::<String>())
            } else {
                name
            }
        })
        .collect::<Vec<_>>();
    let plus = Rect::new(area.right().saturating_sub(3), area.y, 3, 1);
    frame.render_widget(Paragraph::new(" + ").fg(MINT).bold(), plus);
    view.hits
        .push((plus, Hit::Key(KeyEvent::from(KeyCode::Char('+')))));
    let mut remaining = area.width.saturating_sub(4);
    if !state.closed_tabs.is_empty() {
        let reopen = Rect::new(
            area.x + remaining.saturating_sub(11),
            area.y,
            11.min(remaining),
            1,
        );
        frame.render_widget(Paragraph::new("[T] reopen").fg(MINT), reopen);
        view.hits
            .push((reopen, Hit::Key(KeyEvent::from(KeyCode::Char('t')))));
        remaining = remaining.saturating_sub(12);
    }
    let area = Rect::new(area.x, area.y, remaining, area.height);
    // Keep the selected tab visible; arrows and Tab reach every chunk.
    let available = area.width.saturating_sub(6);
    let mut start = 0;
    while start < view.tab
        && labels[start..=view.tab]
            .iter()
            .map(|s| s.chars().count() + 2)
            .sum::<usize>()
            > available as usize
    {
        start += 1;
    }
    let mut x = area.x;
    if start > 0 {
        frame.render_widget(Paragraph::new("‹ ").fg(MINT), Rect::new(x, area.y, 2, 1));
        view.hits.push((
            Rect::new(x, area.y, 2, 1),
            Hit::Key(KeyEvent::from(KeyCode::Char('['))),
        ));
        x += 2;
    }
    for (index, label) in labels.iter().enumerate().skip(start) {
        let width = label.chars().count() as u16 + 2;
        if x + width > area.right().saturating_sub(2) {
            let end = Rect::new(area.right().saturating_sub(2), area.y, 2, 1);
            frame.render_widget(Paragraph::new(" ›").fg(MINT), end);
            view.hits
                .push((end, Hit::Key(KeyEvent::from(KeyCode::Char(']')))));
            break;
        }
        let tab = Rect::new(x, area.y, width, 1);
        let style = if index == view.tab {
            Style::default().fg(MINT).bg(Color::Rgb(38, 66, 61)).bold()
        } else {
            Style::default().fg(MUTED)
        };
        frame.render_widget(Paragraph::new(format!(" {label} ")).style(style), tab);
        view.hits.push((tab, Hit::Tab(index)));
        x += width;
    }
}

fn shortcut_controls() -> Vec<(&'static str, KeyEvent)> {
    [
        ("[Space] record/stop", KeyCode::Char(' ')),
        ("[C] copy", KeyCode::Char('c')),
        ("[Ctrl+C] latest", KeyCode::Char('c')),
        ("[E] export", KeyCode::Char('e')),
        ("[X] clear", KeyCode::Char('x')),
        ("[S] settings", KeyCode::Char('s')),
        ("[↑↓] scroll", KeyCode::Down),
        ("[Tab] next", KeyCode::Tab),
        ("[Esc] cancel", KeyCode::Esc),
        ("[Q] close", KeyCode::Char('q')),
    ]
    .into_iter()
    .map(|(label, code)| {
        (
            label,
            KeyEvent::new(
                code,
                if label == "[Ctrl+C] latest" {
                    KeyModifiers::CONTROL
                } else {
                    KeyModifiers::NONE
                },
            ),
        )
    })
    .collect()
}
fn shortcuts(width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut row = String::new();
    for (shortcut, _) in shortcut_controls() {
        if !row.is_empty() && row.chars().count() + 2 + shortcut.chars().count() > width as usize {
            lines.push(Line::from(std::mem::take(&mut row)));
        }
        if !row.is_empty() {
            row.push_str("  ");
        }
        row.push_str(shortcut);
    }
    if !row.is_empty() {
        lines.push(Line::from(row));
    }
    lines
}

fn queue(
    frame: &mut ratatui::Frame,
    state: &State,
    area: Rect,
    hits: &mut Vec<(Rect, Hit)>,
    items: &[(usize, f32)],
) {
    if area.height == 0 || items.is_empty() {
        return;
    }
    // Always show current work before old completed chunks; summarize overflow.
    let mut indices = items.iter().map(|(index, _)| *index).collect::<Vec<_>>();
    indices.sort_by_key(|i| match state.chunks[*i].status {
        ChunkStatus::Recording => (0, *i),
        ChunkStatus::Transcribing => (1, *i),
        ChunkStatus::Queued => (2, *i),
        ChunkStatus::Failed => (3, *i),
        ChunkStatus::Ready => (4, usize::MAX - *i),
        ChunkStatus::Canceled => (5, usize::MAX - *i),
    });
    let chip = |index: usize| {
        let chunk = &state.chunks[index];
        let (label, color) = match chunk.status {
            ChunkStatus::Recording => ("● REC", Color::Rgb(255, 124, 139)),
            ChunkStatus::Transcribing => ("◌ TEXT", Color::Rgb(243, 199, 125)),
            ChunkStatus::Queued => ("WAIT", Color::Rgb(132, 179, 230)),
            ChunkStatus::Ready => ("DONE", MINT),
            ChunkStatus::Failed => ("ERROR", Color::Rgb(255, 124, 139)),
            ChunkStatus::Canceled => ("CANCEL", MUTED),
        };
        Span::styled(
            format!("{:02} {label} {}", index + 1, timestamp(chunk.seconds)),
            {
                let opacity = items
                    .iter()
                    .find(|(i, _)| *i == index)
                    .map_or(1., |(_, opacity)| *opacity);
                Style::default()
                    .fg(fade(color, opacity))
                    .bg(fade(Color::Rgb(32, 41, 51), opacity))
            },
        )
    };
    let total_width: usize = indices.iter().map(|i| chip(*i).width() + 1).sum();
    let overflow_width = if total_width.saturating_sub(1) > area.width as usize {
        format!(" +{} more", items.len()).len()
    } else {
        0
    };
    let mut remaining = (area.width as usize).saturating_sub(overflow_width);
    indices.retain(|index| {
        let width = chip(*index).width();
        if width > remaining {
            return false;
        }
        remaining = remaining.saturating_sub(width + 1);
        true
    });
    let overflow = items.len() - indices.len();
    indices.sort_unstable();
    let mut spans = Vec::new();
    let mut x = area.x;
    for index in indices {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
            x += 1;
        }
        let span = chip(index);
        hits.push((
            Rect::new(x, area.y, span.width() as u16, 1),
            Hit::SelectTab(state.chunks[index].tab_id.clone()),
        ));
        x += span.width() as u16;
        spans.push(span);
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    if overflow > 0 {
        let message = format!(" +{overflow} more");
        let width = message.len() as u16;
        if area.width > width {
            frame.render_widget(
                Paragraph::new(message).fg(MUTED),
                Rect::new(area.right() - width, area.y, width, 1),
            );
        }
    }
}

fn welcome(frame: &mut ratatui::Frame, area: Rect, elapsed: f32) {
    let mut lines = if area.height as usize >= logo::ANSI_FACE.len() + 3 {
        vec![Line::from(""); logo::ANSI_FACE.len()]
    } else {
        vec![Line::from("◉  pasteface").fg(MINT).bold().centered()]
    };
    lines.push(Line::from(""));
    lines.push(
        Line::from(vec![
            Span::raw("A little space to "),
            Span::styled("think out loud.", Style::default().fg(MINT).bold()),
        ])
        .centered(),
    );
    lines.push(Line::from("Press Space to record.").fg(MUTED).centered());
    let height = lines.len() as u16;
    let top = area.y + area.height.saturating_sub(height) / 2;
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(area.x, top, area.width, height.min(area.height)),
    );
    if area.height as usize >= logo::ANSI_FACE.len() + 3 {
        logo::reveal(
            frame,
            Rect::new(area.x, top, area.width, logo::ANSI_FACE.len() as u16),
            elapsed,
        );
    }
}
fn microphone_index(state: &State) -> usize {
    state
        .selected_device
        .as_ref()
        .and_then(|device| state.devices.iter().position(|d| d == device))
        .map_or(0, |index| index + 1)
}
fn microphone_picker(
    frame: &mut ratatui::Frame,
    state: &State,
    index: usize,
    hits: &mut Vec<(Rect, Hit)>,
) {
    let width = frame.area().width.saturating_sub(6).min(70);
    let height = (state.devices.len() as u16 + 5).min(frame.area().height.saturating_sub(4));
    let area = Rect::new(
        (frame.area().width - width) / 2,
        (frame.area().height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    let items = std::iter::once("System default microphone")
        .chain(state.devices.iter().map(String::as_str))
        .map(ListItem::new)
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(" MICROPHONE ")
                .title_bottom(" ↑↓ choose · Enter select · Esc cancel "),
        )
        .style(Style::default().bg(BG).fg(MUTED))
        .highlight_style(Style::default().bg(Color::Rgb(38, 66, 61)).fg(MINT).bold())
        .highlight_symbol("› ");
    let mut selection = ListState::default().with_selected(Some(index.min(state.devices.len())));
    frame.render_stateful_widget(list, area, &mut selection);
    for row in 0..area.height.saturating_sub(2) {
        let item = selection.offset() + row as usize;
        if item <= state.devices.len() {
            hits.push((
                Rect::new(
                    area.x + 1,
                    area.y + 1 + row,
                    area.width.saturating_sub(2),
                    1,
                ),
                Hit::Microphone(item),
            ));
        }
    }
    hits.push((
        Rect::new(area.x, area.bottom() - 1, area.width, 1),
        Hit::Key(KeyEvent::from(KeyCode::Esc)),
    ));
}
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn completed_queue_items_fade_without_removing_transcripts() {
        let mut state = State::default();
        state.chunks.push(crate::model::Chunk {
            id: "chunk".into(),
            tab_id: "default".into(),
            title: String::new(),
            status: ChunkStatus::Ready,
            text: "Keep this transcript".into(),
            error: None,
            seconds: 10.,
        });
        let mut completed = HashMap::new();
        let now = Instant::now();
        // Historical completions stay hidden, even after an initial empty snapshot.
        assert!(queue_items(&State::default(), &mut completed, now).is_empty());
        assert!(queue_items(&state, &mut completed, now).is_empty());
        state.chunks[0].status = ChunkStatus::Transcribing;
        assert_eq!(queue_items(&state, &mut completed, now), vec![(0, 1.)]);
        state.chunks[0].status = ChunkStatus::Ready;
        assert_eq!(queue_items(&state, &mut completed, now), vec![(0, 1.)]);
        assert_eq!(
            queue_items(&state, &mut completed, now + Duration::from_millis(3500)),
            vec![(0, 1.)]
        );
        assert_eq!(
            queue_items(&state, &mut completed, now + Duration::from_millis(3750)),
            vec![(0, 0.5)]
        );
        assert!(queue_items(&state, &mut completed, now + Duration::from_secs(4)).is_empty());
        assert_eq!(state.chunks[0].text, "Keep this transcript");
        state.chunks[0].status = ChunkStatus::Queued;
        assert_eq!(
            queue_items(&state, &mut completed, now + Duration::from_secs(5)),
            vec![(0, 1.)]
        );
        state.chunks[0].status = ChunkStatus::Ready;
        assert_eq!(
            queue_items(&state, &mut completed, now + Duration::from_secs(6)),
            vec![(0, 1.)]
        );
        assert_eq!(fade(MINT, 0.), BG);
    }
    #[test]
    fn queue_chips_fit_one_row_with_duration_and_cancellation() {
        let mut state = State::default();
        for status in [
            ChunkStatus::Queued,
            ChunkStatus::Transcribing,
            ChunkStatus::Canceled,
        ] {
            state.chunks.push(crate::model::Chunk {
                tab_id: "default".into(),
                title: String::new(),
                id: String::new(),
                status,
                text: String::new(),
                error: None,
                seconds: 65.,
            });
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 1)).unwrap();
        terminal
            .draw(|f| {
                queue(
                    f,
                    &state,
                    f.area(),
                    &mut Vec::new(),
                    &[(0, 1.), (1, 1.), (2, 1.)],
                )
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("01 WAIT 01:05"));
        assert!(text.contains("02 ◌ TEXT 01:05"));
        assert!(text.contains("03 CANCEL 01:05"));
    }
    #[test]
    fn recording_has_one_status_line_and_no_duplicate_meter() {
        let state = State {
            phase: Phase::Recording,
            seconds: 12.,
            level: 0.5,
            ..State::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|f| draw(f, &state, None, &mut View::default()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert_eq!(text.matches("Listening").count(), 1);
        assert_eq!(text.matches("00:12").count(), 1);
        assert!(!text.contains("queued"));
        assert!(!text.contains("░"));
        assert!(text.contains("MIC"));
    }
    #[test]
    fn tabs_render_selected_text_and_expose_click_targets() {
        let state = State {
            transcript: "First thought.\nSecond thought.".into(),
            phase: Phase::Ready,
            chunks: ["First thought.", "Second thought."]
                .iter()
                .enumerate()
                .map(|(i, text)| crate::model::Chunk {
                    tab_id: "default".into(),
                    title: String::new(),
                    id: format!("chunk-{i}"),
                    text: (*text).into(),
                    status: ChunkStatus::Ready,
                    error: None,
                    seconds: 12.,
                })
                .collect(),
            ..State::default()
        };
        let mut view = View {
            tab: 0,
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| draw(f, &state, None, &mut view)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("Second thought."));
        assert!(text.contains("First thought."));
        assert!(view.hits.iter().any(|(_, hit)| matches!(hit, Hit::Tab(0))));
        for y in view.transcript_area.y..view.transcript_area.bottom() {
            assert_ne!(
                terminal.backend().buffer()[(view.transcript_area.x, y)].symbol(),
                "│"
            );
        }
    }
    #[test]
    fn welcome_and_transcript_fit() {
        for (width, height) in [(100, 36), (44, 14), (20, 8)] {
            let mut t = Terminal::new(TestBackend::new(width, height)).unwrap();
            t.draw(|f| draw(f, &State::default(), None, &mut View::default()))
                .unwrap();
            let state = State {
                transcript: "A thought worth keeping.".into(),
                phase: Phase::Ready,
                ..State::default()
            };
            t.draw(|f| draw(f, &state, None, &mut View::default()))
                .unwrap();
            if width >= 44 {
                let text: String = t
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|c| c.symbol())
                    .collect();
                assert!(text.contains("A thought worth keeping."));
            }
        }
    }
}
