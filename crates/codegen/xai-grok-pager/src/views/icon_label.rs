//! `IconLabel`: a small picture and the text it names, on one row.

use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use super::provider_brand::ProviderIcon;

/// The cells an icon covers. A cell is about twice as tall as wide, so this is a square.
pub const ICON_CELLS: u16 = 2;
/// The blank cells between an icon and its label.
const ICON_GAP: u16 = 1;

/// Whether this terminal draws an icon. The gate is the same as inline scrollback media.
pub fn icons_supported() -> bool {
    crate::terminal::image::scrollback_inline_overlay_active()
}

/// Where one icon goes on screen this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconSlot {
    pub icon: ProviderIcon,
    pub rect: Rect,
}

/// A label with an optional icon before it.
#[derive(Debug, Clone)]
pub struct IconLabel<'a> {
    pub label: &'a str,
    pub style: Style,
    pub icon: Option<&'a ProviderIcon>,
}

impl IconLabel<'_> {
    fn label_width(&self) -> u16 {
        u16::try_from(self.label.width()).unwrap_or(u16::MAX)
    }

    /// The cells the control takes when it has room for its icon.
    pub fn width(&self) -> u16 {
        let lead = if self.icon.is_some() {
            ICON_CELLS + ICON_GAP
        } else {
            0
        };
        self.label_width().saturating_add(lead)
    }

    /// The cells the control takes inside `max_width`. The icon goes first when the row is short.
    pub fn width_within(&self, max_width: u16) -> u16 {
        if self.width() <= max_width {
            self.width()
        } else {
            self.label_width().min(max_width)
        }
    }

    /// Draw at `(x, y)` in at most `max_width` cells. The answer is where the
    /// icon belongs, when it was given room. `blank` styles the cells under it.
    pub fn render(
        &self,
        buf: &mut Buffer,
        x: u16,
        y: u16,
        max_width: u16,
        blank: Style,
    ) -> Option<IconSlot> {
        let slot = self
            .icon
            .filter(|_| self.width() <= max_width)
            .map(|icon| IconSlot {
                icon: icon.clone(),
                rect: Rect::new(x, y, ICON_CELLS, 1),
            });
        let mut label_x = x;
        let mut room = max_width;
        if slot.is_some() {
            let lead = ICON_CELLS + ICON_GAP;
            buf.set_span(x, y, &Span::styled("   ", blank), lead);
            label_x += lead;
            room -= lead;
        }
        buf.set_span(label_x, y, &Span::styled(self.label, self.style), room);
        slot
    }
}

/// The icons the terminal shows now.
#[derive(Debug, Default)]
pub struct IconLayer {
    transmitted: HashSet<u32>,
    placed: Vec<(u32, Rect)>,
}

impl IconLayer {
    /// The escapes that make the terminal show exactly `slots`. Empty when nothing is drawn and nothing was.
    pub fn sync(&mut self, slots: &[IconSlot]) -> String {
        use crate::terminal::image;
        let mut out = String::new();
        for (id, _) in &self.placed {
            if !slots.iter().any(|slot| slot.icon.id == *id) {
                out.push_str(&image::clear_kitty_image(*id));
                // A terminal may free an image that has no placement left.
                self.transmitted.remove(id);
            }
        }
        for slot in slots {
            let id = slot.icon.id;
            let moved = !self.placed.contains(&(id, slot.rect));
            // A moved icon is sent again: a screen clear between frames drops the image with its placement.
            if moved || !self.transmitted.contains(&id) {
                let Some(transmit) = image::transmit_inline_image(&slot.icon.png, id) else {
                    continue;
                };
                out.push_str(&transmit);
                self.transmitted.insert(id);
            }
            // Save the cursor, place at the slot, restore. `z=-1` keeps the image under any text drawn later.
            out.push_str(&format!(
                "\x1b7\x1b[{};{}H{}\x1b8",
                slot.rect.y + 1,
                slot.rect.x + 1,
                image::place_kitty_image(id, slot.rect.width, slot.rect.height, -1),
            ));
        }
        self.placed = slots.iter().map(|slot| (slot.icon.id, slot.rect)).collect();
        out
    }

    /// Whether an icon is on screen.
    pub fn is_empty(&self) -> bool {
        self.placed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::terminal::image::{GraphicsProtocol, set_protocol_for_test};

    fn icon(id: u32) -> ProviderIcon {
        ProviderIcon {
            id,
            png: Arc::from(b"\x89PNG\r\n\x1a\n-not-a-real-image".as_slice()),
        }
    }

    fn row(buf: &Buffer, width: u16) -> String {
        (0..width)
            .map(|x| buf[(x, 0)].symbol().to_owned())
            .collect()
    }

    #[test]
    fn the_icon_takes_two_cells_and_a_gap_before_the_label() {
        let icon = icon(7);
        let label = IconLabel {
            label: "Anthropic",
            style: Style::default(),
            icon: Some(&icon),
        };
        assert_eq!(label.width(), 12);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let slot = label.render(&mut buf, 2, 0, 12, Style::default());
        assert_eq!(row(&buf, 20), "     Anthropic      ");
        assert_eq!(slot.unwrap().rect, Rect::new(2, 0, 2, 1));
    }

    #[test]
    fn a_row_too_short_for_both_keeps_the_label_and_drops_the_icon() {
        let icon = icon(7);
        let label = IconLabel {
            label: "Anthropic",
            style: Style::default(),
            icon: Some(&icon),
        };
        assert_eq!(label.width_within(10), 9);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let slot = label.render(&mut buf, 0, 0, 10, Style::default());
        assert_eq!(row(&buf, 20), "Anthropic           ");
        assert_eq!(slot, None);
    }

    #[test]
    fn a_label_with_no_icon_is_its_text() {
        let label = IconLabel {
            label: "Groq",
            style: Style::default(),
            icon: None,
        };
        assert_eq!(label.width(), 4);
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
        assert_eq!(label.render(&mut buf, 0, 0, 8, Style::default()), None);
        assert_eq!(row(&buf, 8), "Groq    ");
    }

    #[test]
    fn the_layer_transmits_once_places_each_frame_and_deletes_on_leave() {
        let _guard = set_protocol_for_test(GraphicsProtocol::Kitty);
        let mut layer = IconLayer::default();
        let slot = IconSlot {
            icon: icon(0x7000_0001),
            rect: Rect::new(60, 30, 2, 1),
        };

        let first = layer.sync(std::slice::from_ref(&slot));
        assert!(first.contains("a=t"), "the image is sent: {first:?}");
        assert!(first.contains("\x1b[31;61H"), "the cursor goes to the slot");
        assert!(first.contains("a=p,i=1879048193"), "then it is placed");
        assert!(first.starts_with("\x1b_G") && first.ends_with("\x1b8"));

        let second = layer.sync(std::slice::from_ref(&slot));
        assert!(!second.contains("a=t"), "an unmoved icon is not sent again");
        assert!(second.contains("a=p,i=1879048193"));

        let moved = IconSlot {
            rect: Rect::new(58, 30, 2, 1),
            ..slot.clone()
        };
        let third = layer.sync(std::slice::from_ref(&moved));
        assert!(third.contains("a=t"), "a moved icon is sent again");
        assert!(third.contains("\x1b[31;59H"));

        let gone = layer.sync(&[]);
        assert_eq!(gone, "\x1b_Ga=d,d=i,i=1879048193,q=2\x1b\\");
        assert!(layer.is_empty());
        assert_eq!(layer.sync(&[]), "", "nothing to do once it is gone");
    }

    #[test]
    fn a_terminal_with_no_graphics_gets_no_escape() {
        let _guard = set_protocol_for_test(GraphicsProtocol::None);
        let mut layer = IconLayer::default();
        let slot = IconSlot {
            icon: icon(9),
            rect: Rect::new(0, 0, 2, 1),
        };
        assert_eq!(layer.sync(&[slot]), "");
    }
}
