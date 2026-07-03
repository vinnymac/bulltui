//! Centralized colors and styles. All color choices live here.

use bullmq::{EventKind, JobState};
use ratatui::style::{Color, Modifier, Style};

pub const ACCENT: Color = Color::Cyan;
pub const MUTED: Color = Color::DarkGray;
pub const BORDER: Color = Color::DarkGray;
pub const BORDER_FOCUS: Color = Color::Cyan;

pub const WARN: Color = Color::Yellow;
pub const DANGER: Color = Color::Red;

/// Sentinel color for the live connection indicator. A unique RGB value so
/// `CellFilter::FgColor` targets only this cell without matching other greens
/// (e.g. completed counts). [`LIVE_DIM`] is the pulse's dim endpoint.
pub const LIVE: Color = Color::Rgb(0x53, 0xE0, 0x6A);
pub const LIVE_DIM: Color = Color::Rgb(0x1C, 0x4A, 0x27);

/// Amber for the boot LED while the connection is being established.
/// Distinct from the live green and error red.
pub const CONNECTING: Color = Color::Rgb(0xE0, 0xA5, 0x30);

/// Boot splash dot colours: fully lit and dim. The reveal fades between these
/// two (never from black), so dots appear to warm up rather than flash in.
pub const SPLASH_DOT: Color = Color::Rgb(0x4F, 0xD6, 0xE6);
pub const SPLASH_DOT_DIM: Color = Color::Rgb(0x16, 0x3E, 0x47);

pub const SELECTION_BG: Color = Color::Indexed(238);
pub const HEADER_FG: Color = Color::Cyan;

pub fn title() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn header() -> Style {
    Style::default().fg(HEADER_FG).add_modifier(Modifier::BOLD)
}

pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

pub fn selected() -> Style {
    Style::default()
        .bg(SELECTION_BG)
        .add_modifier(Modifier::BOLD)
}

pub fn key_hint() -> Style {
    Style::default().fg(ACCENT)
}

/// Accent color for the scrollbar thumb; contrasts with the muted track.
pub fn scrollbar_thumb() -> Style {
    Style::default().fg(ACCENT)
}

/// Border-tone color for the scrollbar track; recedes behind the accent thumb.
pub fn scrollbar_track() -> Style {
    Style::default().fg(BORDER)
}

/// Style for the multi-select check mark on a chosen row.
pub fn select_mark() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn danger() -> Style {
    Style::default().fg(DANGER).add_modifier(Modifier::BOLD)
}

/// Per-state color for badges, counts, and bar fills. Matches bull-board's
/// dark-mode palette; muted RGB values chosen for dark terminal backgrounds.
pub fn state_color(state: JobState) -> Color {
    match state {
        JobState::Active => Color::Rgb(0x5B, 0x8A, 0xD7),
        JobState::Waiting => Color::Rgb(0xBF, 0x93, 0x2B),
        JobState::WaitingChildren => Color::Rgb(0xDD, 0x95, 0x5A),
        JobState::Prioritized => Color::Rgb(0xC3, 0x66, 0xD1),
        JobState::Completed => Color::Rgb(0x2D, 0x86, 0x4D),
        JobState::Failed => Color::Rgb(0xBF, 0x40, 0x40),
        JobState::Delayed => Color::Rgb(0x93, 0x74, 0xDC),
        JobState::Paused => Color::Rgb(0xA8, 0xA2, 0x9F),
    }
}

pub fn state_style(state: JobState) -> Style {
    Style::default().fg(state_color(state))
}

/// Colour for a live-feed event row, by kind (reusing the state palette).
pub fn event_color(kind: EventKind) -> Color {
    use EventKind::*;
    match kind {
        Completed => state_color(JobState::Completed),
        Failed | RetriesExhausted | Stalled => state_color(JobState::Failed),
        Active | Progress => state_color(JobState::Active),
        Waiting | WaitingChildren => state_color(JobState::Waiting),
        Delayed => state_color(JobState::Delayed),
        Paused | Resumed | Drained => state_color(JobState::Paused),
        Added => ACCENT,
        Deduplicated | Duplicated => WARN,
        Removed | Cleaned | Other => MUTED,
    }
}

/// Near-black and near-white inks for text on colored fills. Used by
/// [`contrast_text`] to pick whichever yields the higher WCAG contrast ratio.
const INK_DARK: Color = Color::Rgb(0x18, 0x1D, 0x25);
const INK_LIGHT: Color = Color::Rgb(0xEC, 0xEF, 0xF3);

/// A legible text colour for content drawn on top of `bg`, picking near-black or
/// near-white by whichever yields the higher WCAG contrast ratio. Non-RGB
/// backgrounds fall back to dark ink.
pub fn contrast_text(bg: Color) -> Color {
    let Color::Rgb(r, g, b) = bg else {
        return INK_DARK;
    };
    // Relative luminance (WCAG 2.x): linearize each channel, then weight.
    let lin = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
    // Contrast ratio vs white text vs black text; take whichever is higher.
    let vs_light = 1.05 / (l + 0.05);
    let vs_dark = (l + 0.05) / 0.05;
    if vs_dark >= vs_light {
        INK_DARK
    } else {
        INK_LIGHT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_text_picks_legible_ink_per_state() {
        // The bull-board dark palette is mostly mid-tone, so dark ink wins on
        // contrast for every state except the darkest fill (failed red), which
        // takes light ink. This guards the bar counts staying readable.
        for s in JobState::ALL {
            let want = if s == JobState::Failed {
                INK_LIGHT
            } else {
                INK_DARK
            };
            assert_eq!(contrast_text(state_color(s)), want, "{s:?}");
        }
    }
}
