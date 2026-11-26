//! The route dialog: every link between this client and the model's provider, how long each takes to answer.

use std::collections::HashMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use xai_grok_sampling_types::{BodyEncoding, RequestTransport};
use xai_grok_shell::agent::model_route::{RouteLink, RouteView};

use crate::render::SafeBuf;
use crate::theme::Theme;
use crate::views::goal_detail::truncate_to_width;
use crate::views::icon_label::{IconLabel, IconSlot};
use crate::views::provider_brand::ProviderBrand;

/// What a latency probe of one link found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeState {
    Pending,
    Millis(u64),
    Failed(String),
}

/// The dialog's state. `selected` is a position in `view.links`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HopsDialogState {
    /// The chain of the session model's provider. `None` when it declares no hop.
    pub view: Option<RouteView>,
    /// The model the title names.
    pub model_label: String,
    pub selected: usize,
    /// One probe per link, by link id.
    pub probes: HashMap<String, ProbeState>,
    /// The reason the last toggle did not save.
    pub notice: Option<String>,
    /// The chain is being read from config.
    pub loading: bool,
}

impl HopsDialogState {
    pub fn links(&self) -> &[RouteLink] {
        self.view.as_ref().map_or(&[], |view| view.links.as_slice())
    }

    pub fn selected_link(&self) -> Option<&RouteLink> {
        self.links().get(self.selected)
    }

    /// Move the selection by `step` and stop at the ends.
    pub fn move_selection(&mut self, step: isize) {
        let last = self.links().len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + step).clamp(0, last.max(0)) as usize;
    }

    /// Replace the chain after a reload and keep the selection on the same link.
    pub fn set_view(&mut self, view: Option<RouteView>) {
        let selected_id = self.selected_link().map(|link| link.id.clone());
        self.view = view;
        self.selected = selected_id
            .and_then(|id| self.links().iter().position(|link| link.id == id))
            .unwrap_or(0);
    }
}

/// What a frame of the dialog put on screen, for the pointer.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HopsDialogOutput {
    pub close: Option<Rect>,
    /// One rect per link row, with the link's position in the chain.
    pub rows: Vec<(usize, Rect)>,
    /// Where the serving provider's mark goes.
    pub icon: Option<IconSlot>,
}

/// The live facts the dialog shows beside the chain.
#[derive(Debug, Clone, Copy, Default)]
pub struct HopsDialogLive<'a> {
    pub transport: Option<&'a RequestTransport>,
    pub provider: Option<&'a ProviderBrand>,
    pub tokens_per_sec: Option<f64>,
}

const CHROME_ROWS: u16 = 4;
const KIND_WIDTH: usize = 8;
const LATENCY_WIDTH: usize = 12;

fn popup_width(screen: Rect) -> u16 {
    ((screen.width as f32 * 0.75) as u16)
        .clamp(56, 112)
        .min(screen.width.saturating_sub(4))
}

fn body_width(popup_width: u16) -> u16 {
    popup_width.saturating_sub(4)
}

/// A byte count in the largest unit that keeps it short.
fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KIB {
        format!("{bytes} B")
    } else if value < KIB * KIB {
        format!("{:.1} K", value / KIB)
    } else {
        format!("{:.1} M", value / (KIB * KIB))
    }
}

fn encoding_label(encoding: BodyEncoding) -> &'static str {
    match encoding {
        BodyEncoding::Plain => "plain JSON",
        BodyEncoding::Zstd => "zstd",
        BodyEncoding::ZstdDelta => "zstd delta",
    }
}

/// One line that says how the last request travelled.
pub fn transport_summary(transport: &RequestTransport) -> String {
    let mut summary = format!(
        "headers in {} ms \u{b7} {}",
        transport.headers_ms,
        encoding_label(transport.encoding)
    );
    match transport.saved_percent() {
        Some(saved) => summary.push_str(&format!(
            " {} \u{2192} {} (-{saved}%)",
            format_bytes(transport.plain_bytes),
            format_bytes(transport.wire_bytes)
        )),
        None => summary.push_str(&format!(" {}", format_bytes(transport.wire_bytes))),
    }
    summary
}

/// What each hop said about its own time, from `Server-Timing`.
pub fn server_timing_summary(transport: &RequestTransport) -> Option<String> {
    // A relay reports its own share. The rest of the wait is the link to it.
    let link = transport.link_ms().map(|ms| format!("link {ms} ms"));
    let parts: Vec<String> = link
        .into_iter()
        .chain(transport.server_timing.iter().filter_map(|metric| {
            let dur = metric.dur_ms?;
            Some(match metric.desc.as_deref() {
                Some(desc) => format!("{} {dur:.0} ms ({desc})", metric.name),
                None => format!("{} {dur:.0} ms", metric.name),
            })
        }))
        .collect();
    (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
}

fn latency_cell(link: &RouteLink, probe: Option<&ProbeState>, theme: &Theme) -> (String, Style) {
    let dim = Style::default().fg(theme.gray_dim);
    if !link.enabled {
        return ("skipped".to_owned(), dim);
    }
    match probe {
        None => (String::new(), dim),
        Some(ProbeState::Pending) => ("measuring".to_owned(), dim),
        Some(ProbeState::Millis(ms)) => {
            (format!("{ms} ms"), Style::default().fg(theme.text_primary))
        }
        Some(ProbeState::Failed(_)) => (
            "no answer".to_owned(),
            Style::default().fg(theme.accent_error),
        ),
    }
}

fn marker(link: &RouteLink) -> &'static str {
    match (link.optional, link.enabled) {
        (true, true) => "[x]",
        (true, false) => "[ ]",
        (false, _) => " \u{b7} ",
    }
}

/// The body rows under the link rows, each already styled.
fn detail_lines(
    state: &HopsDialogState,
    live: HopsDialogLive<'_>,
    theme: &Theme,
    width: u16,
) -> Vec<Line<'static>> {
    let cols = width as usize;
    let label = |text: &str| Span::styled(format!("{text:<14}"), Style::default().fg(theme.gray));
    let value = |text: String, style: Style| {
        Span::styled(truncate_to_width(&text, cols.saturating_sub(14)), style)
    };
    let plain = Style::default().fg(theme.text_primary);
    let mut lines = Vec::new();
    match &state.view {
        Some(view) => match &view.error {
            Some(error) => lines.push(Line::from(vec![
                label("Broken"),
                value(error.clone(), Style::default().fg(theme.accent_error)),
            ])),
            None => lines.push(Line::from(vec![
                label("Sends to"),
                value(view.entry_url.clone(), plain),
            ])),
        },
        None if state.loading => lines.push(Line::from(Span::styled(
            truncate_to_width("Reading the route from config.json", cols),
            Style::default().fg(theme.gray),
        ))),
        None => {
            lines.push(Line::from(Span::styled(
                truncate_to_width("This model's provider declares no hops.", cols),
                plain,
            )));
            lines.push(Line::from(Span::styled(
                truncate_to_width(
                    "Add `hops` to its `models.providers.<id>` block in config.json.",
                    cols,
                ),
                Style::default().fg(theme.gray),
            )));
        }
    }
    if let Some(transport) = live.transport {
        lines.push(Line::from(vec![
            label("Last request"),
            value(transport_summary(transport), plain),
        ]));
        if let Some(timing) = server_timing_summary(transport) {
            lines.push(Line::from(vec![label("Hop timing"), value(timing, plain)]));
        }
    }
    if let Some(notice) = &state.notice {
        lines.push(Line::from(Span::styled(
            truncate_to_width(notice, cols),
            Style::default().fg(theme.accent_error),
        )));
    }
    lines
}

/// The rows the body takes: the links, a gap, the provider row and the details.
fn body_rows(state: &HopsDialogState, live: HopsDialogLive<'_>, theme: &Theme, width: u16) -> u16 {
    let links = state.links().len() as u16;
    let provider = u16::from(live.provider.is_some());
    let details = detail_lines(state, live, theme, width).len() as u16;
    links + u16::from(links > 0) + provider + details
}

/// The centered rect the dialog takes on `screen`.
pub fn hops_dialog_area(screen: Rect, state: &HopsDialogState, live: HopsDialogLive<'_>) -> Rect {
    let theme = Theme::current();
    let w = popup_width(screen);
    let h = (body_rows(state, live, &theme, body_width(w)) + CHROME_ROWS)
        .min(screen.height.saturating_sub(2));
    Rect::new(
        screen.x + screen.width.saturating_sub(w) / 2,
        screen.y + screen.height.saturating_sub(h) / 2,
        w,
        h,
    )
}

/// Draw the dialog.
pub fn render_hops_dialog(
    buf: &mut Buffer,
    area: Rect,
    state: &HopsDialogState,
    live: HopsDialogLive<'_>,
    close_hovered: bool,
) -> HopsDialogOutput {
    let theme = Theme::current();
    let mut output = HopsDialogOutput::default();
    if area.width < 30 || area.height < CHROME_ROWS + 1 {
        return output;
    }
    let base = Style::default().bg(theme.bg_base);
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, y)) {
                cell.reset();
                cell.set_style(base);
            }
        }
    }
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(theme.gray).bg(theme.bg_base))
        .style(base);
    let inner = block.inner(area);
    ratatui::widgets::Widget::render(block, area, buf);

    let close_text = format!("[{}]", crate::glyphs::ballot_x());
    let close_w = close_text.width() as u16;
    let close_x = area.x + area.width.saturating_sub(close_w + 1);
    let title_cols = close_x.saturating_sub(area.x + 3) as usize;
    let title = truncate_to_width(&format!(" Route \u{b7} {} ", state.model_label), title_cols);
    buf.set_span_safe(
        area.x + 2,
        area.y,
        &Span::styled(
            title,
            Style::default()
                .fg(theme.accent_model)
                .bg(theme.bg_base)
                .add_modifier(Modifier::BOLD),
        ),
        title_cols as u16,
    );
    let close_style = if close_hovered {
        Style::default()
            .fg(theme.text_primary)
            .bg(theme.bg_base)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.gray).bg(theme.bg_base)
    };
    buf.set_span_safe(
        close_x,
        area.y,
        &Span::styled(close_text, close_style),
        close_w,
    );
    output.close = Some(Rect::new(close_x, area.y, close_w, 1));

    let x = inner.x + 1;
    let w = body_width(area.width);
    let cols = w as usize;
    let last_body_row = inner.y + inner.height.saturating_sub(2);
    let mut y = inner.y;

    let links = state.links();
    let name_width = links
        .iter()
        .map(|link| link.name.width())
        .max()
        .unwrap_or(0)
        .clamp(4, 20);
    let url_width = cols.saturating_sub(4 + name_width + 1 + KIND_WIDTH + 1 + LATENCY_WIDTH + 1);
    for (index, link) in links.iter().enumerate() {
        if y >= last_body_row {
            break;
        }
        let selected = index == state.selected;
        let row_bg = if selected {
            theme.bg_highlight
        } else {
            theme.bg_base
        };
        let on = Style::default().bg(row_bg);
        let name_style = if link.enabled {
            on.fg(theme.text_primary)
        } else {
            on.fg(theme.gray_dim)
        };
        let (latency, latency_style) = latency_cell(link, state.probes.get(&link.id), &theme);
        let line = Line::from(vec![
            Span::styled(
                format!("{} ", marker(link)),
                on.fg(if link.optional {
                    theme.accent_success
                } else {
                    theme.gray
                }),
            ),
            Span::styled(
                format!(
                    "{:<name_width$} ",
                    truncate_to_width(&link.name, name_width)
                ),
                name_style.add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::styled(format!("{:<KIND_WIDTH$} ", link.kind), on.fg(theme.gray)),
            Span::styled(
                format!("{:<url_width$} ", truncate_to_width(&link.url, url_width)),
                on.fg(theme.gray),
            ),
            Span::styled(
                format!("{latency:>LATENCY_WIDTH$}"),
                latency_style.bg(row_bg),
            ),
        ]);
        buf.set_line_safe(x, y, &line, w);
        output.rows.push((index, Rect::new(x, y, w, 1)));
        y += 1;
    }
    if !links.is_empty() {
        y += 1;
    }

    if let Some(brand) = live.provider.filter(|_| y < last_body_row) {
        let lead = "Served by     ";
        buf.set_span_safe(
            x,
            y,
            &Span::styled(lead, Style::default().fg(theme.gray).bg(theme.bg_base)),
            w,
        );
        let label_x = x + lead.width() as u16;
        let label = IconLabel {
            label: &brand.name,
            style: Style::default()
                .fg(crate::theme::quantize(brand.color))
                .bg(theme.bg_base)
                .add_modifier(Modifier::BOLD),
            icon: brand
                .icon
                .as_ref()
                .filter(|_| crate::views::icon_label::icons_supported()),
        };
        let room = w.saturating_sub(lead.width() as u16);
        let used = label.width_within(room);
        output.icon = label.render(buf, label_x, y, room, base);
        if let Some(rate) = live.tokens_per_sec {
            let text = format!("  {rate:.0} tok/s");
            buf.set_span_safe(
                label_x + used,
                y,
                &Span::styled(text, Style::default().fg(theme.gray).bg(theme.bg_base)),
                room.saturating_sub(used),
            );
        }
        y += 1;
    }

    for line in detail_lines(state, live, &theme, w) {
        if y >= last_body_row {
            break;
        }
        buf.set_line_safe(x, y, &line, w);
        y += 1;
    }

    let hint = if links.iter().any(|link| link.optional) {
        "\u{2191}/\u{2193} select \u{b7} Space toggle \u{b7} r measure \u{b7} Esc close"
    } else {
        "r measure \u{b7} Esc close"
    };
    buf.set_span_safe(
        x,
        inner.y + inner.height - 1,
        &Span::styled(hint, Style::default().fg(theme.gray_dim).bg(theme.bg_base)),
        w,
    );
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_sampling_types::ServerTiming;

    fn link(id: &str, kind: &str, optional: bool, enabled: bool, url: &str) -> RouteLink {
        RouteLink {
            id: id.to_owned(),
            name: id.to_owned(),
            kind: kind.to_owned(),
            optional,
            enabled,
            url: url.to_owned(),
        }
    }

    fn chain() -> RouteView {
        RouteView {
            provider_id: "openrouter".to_owned(),
            links: vec![
                link("nas", "relay", true, true, "http://nas.lan:8317"),
                link(
                    "bifrost",
                    "gateway",
                    true,
                    false,
                    "http://bifrost.lan:8080/v1",
                ),
                link(
                    "openrouter",
                    "provider",
                    false,
                    true,
                    "https://openrouter.ai/api/v1",
                ),
            ],
            entry_url: "http://nas.lan:8317/openrouter/api/v1".to_owned(),
            error: None,
        }
    }

    fn state() -> HopsDialogState {
        let mut probes = HashMap::new();
        probes.insert("nas".to_owned(), ProbeState::Millis(42));
        probes.insert(
            "openrouter".to_owned(),
            ProbeState::Failed("timed out".to_owned()),
        );
        HopsDialogState {
            view: Some(chain()),
            model_label: "DeepSeek V4".to_owned(),
            selected: 0,
            probes,
            notice: None,
            loading: false,
        }
    }

    fn render(
        state: &HopsDialogState,
        live: HopsDialogLive<'_>,
    ) -> (Vec<String>, HopsDialogOutput) {
        let screen = Rect::new(0, 0, 110, 30);
        let mut buf = Buffer::empty(screen);
        let area = hops_dialog_area(screen, state, live);
        let output = render_hops_dialog(&mut buf, area, state, live, false);
        let rows = (0..screen.height)
            .map(|y| {
                (0..screen.width)
                    .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
                    .collect::<String>()
            })
            .collect();
        (rows, output)
    }

    fn row_with<'a>(rows: &'a [String], needle: &str) -> &'a str {
        rows.iter()
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no row holds {needle:?} in:\n{}", rows.join("\n")))
    }

    #[test]
    fn every_link_is_a_row_with_its_toggle_kind_address_and_latency() {
        let (rows, output) = render(&state(), HopsDialogLive::default());
        let nas = row_with(&rows, "http://nas.lan:8317 ");
        assert!(nas.contains("[x] nas"), "{nas}");
        assert!(nas.contains("relay"), "{nas}");
        assert!(nas.trim_end().ends_with("42 ms \u{2502}"), "{nas}");
        let bifrost = row_with(&rows, "bifrost.lan");
        assert!(bifrost.contains("[ ] bifrost"), "{bifrost}");
        assert!(
            bifrost.contains("skipped"),
            "a hop that is off is not measured: {bifrost}"
        );
        let provider = row_with(&rows, "https://openrouter.ai/api/v1");
        assert!(provider.contains("\u{b7}  openrouter"), "{provider}");
        assert!(provider.contains("no answer"), "{provider}");
        assert_eq!(output.rows.len(), 3);
        assert_eq!(output.rows[1].0, 1);
        assert!(row_with(&rows, "Sends to").contains("http://nas.lan:8317/openrouter/api/v1"));
        assert!(row_with(&rows, "Route").contains("DeepSeek V4"));
    }

    #[test]
    fn the_last_request_and_the_hops_own_timing_are_shown() {
        let transport = RequestTransport {
            encoding: BodyEncoding::ZstdDelta,
            plain_bytes: 400 * 1024,
            wire_bytes: 4 * 1024,
            headers_ms: 812,
            server_timing: vec![ServerTiming {
                name: "ai-proxy".to_owned(),
                dur_ms: Some(640.4),
                desc: Some("upstream".to_owned()),
            }],
        };
        let live = HopsDialogLive {
            transport: Some(&transport),
            ..Default::default()
        };
        let (rows, _) = render(&state(), live);
        let last = row_with(&rows, "Last request");
        assert!(
            last.contains("headers in 812 ms \u{b7} zstd delta 400.0 K \u{2192} 4.0 K (-99%)"),
            "{last}"
        );
        assert!(
            row_with(&rows, "Hop timing").contains("ai-proxy 640 ms (upstream)"),
            "{rows:?}"
        );
    }

    #[test]
    fn the_serving_provider_is_named_with_its_rate() {
        let brand = crate::views::provider_brand::brand_for("deepinfra");
        let live = HopsDialogLive {
            provider: Some(&brand),
            tokens_per_sec: Some(41.4),
            ..Default::default()
        };
        let (rows, _) = render(&state(), live);
        let served = row_with(&rows, "Served by");
        assert!(served.contains("DeepInfra"), "{served}");
        assert!(served.contains("41 tok/s"), "{served}");
    }

    #[test]
    fn a_broken_chain_shows_why_in_place_of_the_entry_url() {
        let mut state = state();
        let view = state.view.as_mut().unwrap();
        view.entry_url.clear();
        view.error = Some("relay `nas` has no `routes.bifrost` entry".to_owned());
        let (rows, _) = render(&state, HopsDialogLive::default());
        assert!(row_with(&rows, "Broken").contains("routes.bifrost"));
        assert!(!rows.iter().any(|row| row.contains("Sends to")));
    }

    #[test]
    fn a_provider_with_no_hops_says_so() {
        let state = HopsDialogState {
            model_label: "local".to_owned(),
            ..Default::default()
        };
        let (rows, output) = render(&state, HopsDialogLive::default());
        assert!(rows.iter().any(|row| row.contains("declares no hops")));
        assert!(output.rows.is_empty());
        assert!(row_with(&rows, "Esc close").contains("r measure"));
    }

    #[test]
    fn the_selection_stops_at_the_ends_and_follows_its_link_through_a_reload() {
        let mut state = state();
        state.move_selection(-1);
        assert_eq!(state.selected, 0);
        state.move_selection(5);
        assert_eq!(state.selected, 2);
        state.selected = 1;
        let mut reloaded = chain();
        reloaded.links.remove(0);
        state.set_view(Some(reloaded));
        assert_eq!(state.selected_link().unwrap().id, "bifrost");
    }

    #[test]
    fn a_plain_request_reports_its_size_with_no_saving() {
        let transport = RequestTransport {
            encoding: BodyEncoding::Plain,
            plain_bytes: 900,
            wire_bytes: 900,
            headers_ms: 30,
            server_timing: Vec::new(),
        };
        assert_eq!(
            transport_summary(&transport),
            "headers in 30 ms \u{b7} plain JSON 900 B"
        );
        assert_eq!(server_timing_summary(&transport), None);
    }

    /// A relay reports its decode and its upstream wait. The rest is the link to it.
    #[test]
    fn a_relay_s_timing_splits_the_wait_into_the_link_and_the_upstream() {
        let transport = RequestTransport {
            encoding: BodyEncoding::ZstdDelta,
            plain_bytes: 100_000,
            wire_bytes: 400,
            headers_ms: 450,
            server_timing: xai_grok_sampling_types::parse_server_timing([
                "relay-decode;dur=1.6, relay-upstream;dur=410.2;desc=\"openrouter\"",
            ]),
        };
        assert_eq!(transport.link_ms(), Some(38));
        assert_eq!(
            server_timing_summary(&transport).as_deref(),
            Some("link 38 ms \u{b7} relay-decode 2 ms \u{b7} relay-upstream 410 ms (openrouter)")
        );
    }

    /// A metric from another server is shown as it is. With no relay metric, no link time is derived.
    #[test]
    fn a_timing_with_no_relay_metric_derives_no_link_time() {
        let transport = RequestTransport {
            headers_ms: 450,
            server_timing: xai_grok_sampling_types::parse_server_timing(["edge;dur=12"]),
            ..Default::default()
        };
        assert_eq!(transport.link_ms(), None);
        assert_eq!(
            server_timing_summary(&transport).as_deref(),
            Some("edge 12 ms")
        );
    }
}
