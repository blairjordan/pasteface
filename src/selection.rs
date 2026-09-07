use ratatui::{buffer::Buffer, layout::Rect, style::Color};

#[derive(Clone, Copy)]
pub struct Selection {
    anchor: (u16, u16),
    head: (u16, u16),
}
impl Selection {
    pub fn new(column: u16, row: u16) -> Self {
        Self {
            anchor: (row, column),
            head: (row, column),
        }
    }
    pub fn drag(&mut self, column: u16, row: u16, area: Rect) {
        self.head = (
            row.clamp(area.y, area.bottom().saturating_sub(1)),
            column.clamp(area.x, area.right().saturating_sub(1)),
        );
    }
    fn contains(&self, x: u16, y: u16) -> bool {
        let point = (y, x);
        self.anchor.min(self.head) <= point && point <= self.anchor.max(self.head)
    }
    pub fn paint(&self, buffer: &mut Buffer, area: Rect) {
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                if self.contains(x, y) {
                    buffer[(x, y)].set_bg(Color::Rgb(46, 74, 91));
                }
            }
        }
    }
    pub fn text(&self, cells: &[Vec<String>], area: Rect) -> String {
        if self.anchor == self.head {
            return String::new();
        }
        cells
            .iter()
            .enumerate()
            .filter_map(|(row, cells)| {
                let y = area.y + row as u16;
                let text: String = cells
                    .iter()
                    .enumerate()
                    .filter(|(column, _)| self.contains(area.x + *column as u16, y))
                    .map(|(_, text)| text.as_str())
                    .collect();
                (!text.is_empty()).then(|| text.trim_end().to_owned())
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reverse_drag_copies_only_text_without_padding() {
        let cells = ["first words  ", "second line  "]
            .iter()
            .map(|line| line.chars().map(|c| c.to_string()).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let area = Rect::new(2, 3, 13, 2);
        let mut selection = Selection::new(7, 4);
        selection.drag(8, 3, area);
        assert_eq!(selection.text(&cells, area), "words\nsecond");
    }
}
