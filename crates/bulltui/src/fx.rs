//! tachyonfx-powered animations, tuned to stay subtle and *intentional*.
//!
//! The signature flourish is the **bootup border draw-in**: a bright comet
//! travels clockwise around each box's border ring, drawing the frame onto the
//! screen one edge at a time (see [`border_draw_in`]). Beyond that the kit is
//! restrained — a pulsing connection LED, soft content fades when navigating,
//! and a gentle inner shimmer when a live poll changes the data.
//!
//! Design: [`Animations`] is owned by [`crate::app::App`] but is *only ever
//! advanced by the real run loop* (`App::render_effects`). The renderers in
//! [`crate::ui`] stay pure, so `TestBackend`-driven tests observe a static,
//! deterministic frame. Triggers (`intro`, `transition`, `live_update`, …)
//! merely enqueue effects and never read the clock, so they are harmless when
//! invoked from tests. The one custom effect ([`border_draw_in`]) is driven by
//! its `EffectTimer` alpha (advanced by the per-frame `Duration` the run loop
//! feeds in), never by wall-clock time, so it too stays deterministic.

use ratatui::layout::{Margin, Rect};
use ratatui::style::Color;
use ratatui::Frame;
use tachyonfx::{fx, CellFilter, Duration as FxDuration, Effect, Interpolation};

use crate::theme;

/// Colour that not-yet-revealed cells fade from — a near-black that disappears
/// into a dark terminal, so reveals read as content emerging rather than as a
/// flash of colour.
const VOID: Color = Color::Rgb(0x0A, 0x0A, 0x0A);

/// The settled colour of a drawn border ring. Kept close to `theme::BORDER_FOCUS`
/// (cyan) so the hand-off to the statically-rendered border is seamless when the
/// draw-in completes.
const RING: (u8, u8, u8) = (0x1E, 0xC4, 0xD6);
/// The bright crest of the travelling comet — a near-white cyan.
const COMET: (u8, u8, u8) = (0xE4, 0xFF, 0xFF);
/// How many cells the comet's glowing tail spans.
const TRAIL: f32 = 10.0;

/// Which framed region an effect paints over. Resolved to a concrete `Rect`
/// every frame ([`crate::ui::regions`]) so effects stay correct across resizes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Zone {
    /// The top status box (LED + wordmark + breadcrumb + poll).
    Header,
    /// The main content stage below the header.
    Body,
}

/// All animation state for the application.
pub struct Animations {
    /// One-shot effects, each tagged with the zone it paints. Every effect runs
    /// to completion and is then dropped.
    fx: Vec<(Zone, Effect)>,
    /// The always-on connection "heartbeat", filtered to the live indicator.
    pulse: Effect,
}

impl Default for Animations {
    fn default() -> Self {
        Self::new()
    }
}

impl Animations {
    pub fn new() -> Self {
        // A slow breathing pulse on the connection dot. The dot is re-rendered
        // in its base colour ([`theme::LIVE`]) every frame, so fading the
        // foreground toward a dimmer green and back — ping-pong, forever —
        // reads as a gentle heartbeat.
        //
        // The filter is attached to the *inner* `fade_to_fg`, not the outer
        // `repeating`/`ping_pong` wrappers: `PingPong` stores a filter handed to
        // it but never applies it to the wrapped effect, so a filter on the
        // outer effect would silently leak the fade across the whole header row.
        let pulse = fx::repeating(fx::ping_pong(
            fx::fade_to_fg(theme::LIVE_DIM, (1100, Interpolation::SineInOut))
                .with_filter(CellFilter::FgColor(theme::LIVE)),
        ));

        Self {
            fx: Vec::new(),
            pulse,
        }
    }

    // -- triggers (cheap; never touch the clock) ---------------------------

    /// Startup reveal: each box's border ring draws itself in clockwise while
    /// its inner content fades up just behind the arriving frame.
    pub fn intro(&mut self) {
        // The header is smaller, so it draws in a touch quicker — the eye lands
        // on the title, then the stage opens beneath it.
        self.fx.push((
            Zone::Header,
            border_draw_in((720, Interpolation::QuadInOut)),
        ));
        self.fx.push((
            Zone::Header,
            fx::fade_from_fg(VOID, (440, Interpolation::QuadOut))
                .with_filter(CellFilter::Inner(Margin::new(1, 1))),
        ));
        self.fx
            .push((Zone::Body, border_draw_in((980, Interpolation::QuadInOut))));
        self.fx.push((
            Zone::Body,
            fx::fade_from_fg(VOID, (620, Interpolation::QuadOut))
                .with_filter(CellFilter::Inner(Margin::new(1, 1))),
        ));
    }

    /// A soft fade-up of the body when navigating between views.
    pub fn transition(&mut self) {
        self.fx.push((
            Zone::Body,
            fx::fade_from_fg(VOID, (220, Interpolation::QuadOut)),
        ));
    }

    /// A quick, gentle fade when switching status / detail tabs.
    pub fn tab_switch(&mut self) {
        self.fx.push((
            Zone::Body,
            fx::fade_from_fg(VOID, (150, Interpolation::QuadOut)),
        ));
    }

    /// A subtle brighten-and-settle when a live poll brings changed data, so
    /// queue progress and state changes register without a hard flash. Filtered
    /// to inner cells so it shimmers the *content*, never the framing border or
    /// its titles.
    pub fn live_update(&mut self) {
        self.fx.push((
            Zone::Body,
            fx::ping_pong(
                fx::hsl_shift_fg([0.0, 0.0, 14.0], (220, Interpolation::SineInOut))
                    .with_filter(CellFilter::Inner(Margin::new(1, 1))),
            ),
        ));
    }

    // -- frame loop --------------------------------------------------------

    /// The redraw cadence the run loop should use: fast while transient effects
    /// play, gentle for the ambient heartbeat, and idle (`None`) when nothing
    /// is animating so a quiescent, disconnected UI costs nothing.
    pub fn frame_budget(&self, connected: bool) -> Option<std::time::Duration> {
        if !self.fx.is_empty() {
            Some(std::time::Duration::from_millis(33))
        } else if connected {
            Some(std::time::Duration::from_millis(100))
        } else {
            None
        }
    }

    /// Advance and paint every active effect. Called once per frame by the run
    /// loop, after the static UI has been rendered into `frame`.
    pub fn process(&mut self, frame: &mut Frame, elapsed: std::time::Duration, connected: bool) {
        let [header, body, _status] = crate::ui::regions(frame.area());
        let dur = FxDuration::from_millis(elapsed.as_millis().min(u32::MAX as u128) as u32);
        let buf = frame.buffer_mut();

        self.fx.retain_mut(|(zone, e)| {
            let area = match zone {
                Zone::Header => header,
                Zone::Body => body,
            };
            e.process(dur, buf, area);
            e.running()
        });

        // The heartbeat only beats while we're actually connected; otherwise
        // the static red dot from `ui::draw_header` is left untouched. The
        // FgColor filter confines it to the single LED cell in the header.
        if connected {
            self.pulse.process(dur, buf, header);
        }
    }
}

/// A clockwise "draw-in" of a box's border ring: a bright comet travels around
/// the perimeter (top → right → bottom → left), leaving the settled border
/// colour behind it and darkness ahead, so the frame appears to draw itself.
///
/// Progress is taken from the effect's timer `alpha` (0 → 1), so the comet's
/// pace is set by the timer duration and is independent of the wall clock.
fn border_draw_in<T: Into<tachyonfx::EffectTimer>>(timer: T) -> Effect {
    fx::effect_fn_buf((), timer, move |_state, ctx, buf| {
        let ring = ring_positions(ctx.area);
        let n = ring.len();
        if n == 0 {
            return;
        }
        // Let the comet head travel one extra `TRAIL` past the final cell so the
        // glow fully exits by the time the effect completes — the last frame is
        // a cleanly-settled ring with no lingering hotspot.
        let head = ctx.alpha() * (n as f32 - 1.0 + TRAIL);
        for (i, (x, y)) in ring.into_iter().enumerate() {
            let d = head - i as f32;
            let color = if d < 0.0 {
                VOID
            } else if d < TRAIL {
                lerp_rgb(RING, COMET, 1.0 - d / TRAIL)
            } else {
                Color::Rgb(RING.0, RING.1, RING.2)
            };
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_fg(color);
            }
        }
    })
}

/// The cells of `area`'s outer ring, in clockwise order starting at the
/// top-left corner. Corners are not duplicated. Degenerate (zero/one-wide or
/// -tall) areas are handled gracefully.
fn ring_positions(area: Rect) -> Vec<(u16, u16)> {
    let mut v = Vec::new();
    if area.width == 0 || area.height == 0 {
        return v;
    }
    let (x0, y0) = (area.x, area.y);
    let x1 = area.x + area.width - 1;
    let y1 = area.y + area.height - 1;
    if area.height == 1 {
        v.extend((x0..=x1).map(|x| (x, y0)));
        return v;
    }
    if area.width == 1 {
        v.extend((y0..=y1).map(|y| (x0, y)));
        return v;
    }
    v.extend((x0..=x1).map(|x| (x, y0))); // top, left → right
    v.extend(((y0 + 1)..y1).map(|y| (x1, y))); // right, top → bottom
    v.extend((x0..=x1).rev().map(|x| (x, y1))); // bottom, right → left
    v.extend(((y0 + 1)..y1).rev().map(|y| (x0, y))); // left, bottom → top
    v
}

/// Linear interpolation between two RGB triples, returning a `Color::Rgb`.
fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color::Rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Drive one frame of the effects pass against a throwaway buffer.
    fn step(anim: &mut Animations, term: &mut Terminal<TestBackend>, ms: u64, connected: bool) {
        term.draw(|f| anim.process(f, std::time::Duration::from_millis(ms), connected))
            .unwrap();
    }

    #[test]
    fn transient_effects_drain_while_the_heartbeat_persists() {
        let mut anim = Animations::new();
        anim.intro();
        anim.transition();
        assert!(!anim.fx.is_empty(), "triggers enqueue effects");

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        // Advance well past the longest transient (980ms) in small steps.
        for _ in 0..40 {
            step(&mut anim, &mut term, 50, true);
        }
        assert!(
            anim.fx.is_empty(),
            "one-shot effects complete and are dropped"
        );
        assert!(
            anim.pulse.running(),
            "the connection heartbeat repeats forever"
        );
    }

    #[test]
    fn process_is_panic_free_when_disconnected_and_on_tiny_areas() {
        let mut anim = Animations::new();
        anim.intro();
        anim.live_update();
        anim.tab_switch();
        // A 1x1 terminal is degenerate (zero-height body); must not panic.
        let mut term = Terminal::new(TestBackend::new(1, 1)).unwrap();
        for _ in 0..10 {
            step(&mut anim, &mut term, 16, false);
        }
    }

    #[test]
    fn ring_positions_trace_the_perimeter_clockwise() {
        let ring = ring_positions(Rect::new(0, 0, 3, 3));
        // 8 distinct border cells for a 3x3 box, clockwise from the top-left.
        assert_eq!(
            ring,
            vec![
                (0, 0),
                (1, 0),
                (2, 0), // top, L→R
                (2, 1), // right
                (2, 2),
                (1, 2),
                (0, 2), // bottom, R→L
                (0, 1), // left, B→T
            ]
        );
        // No duplicates (corners counted once).
        let mut sorted = ring.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ring.len(), "no cell painted twice");
    }
}
