//! tachyonfx animation effects: border draw-in, content fades, connection
//! pulse, and the splash shimmer.
//!
//! [`Animations`] is owned by [`crate::app::App`] but advanced only by the
//! run loop (`App::render_effects`). Triggers enqueue effects without reading
//! the clock, so they are safe to call from tests. Custom effects are driven
//! by `EffectTimer` alpha, never wall-clock time, keeping `TestBackend` output
//! deterministic.

use ratatui::layout::{Margin, Rect};
use ratatui::style::Color;
use ratatui::Frame;
use tachyonfx::{fx, CellFilter, Duration as FxDuration, Effect, Interpolation};

use crate::theme;

/// Near-black start colour for reveals; disappears into a dark terminal
/// background so the fade reads as content appearing, not a colour flash.
const VOID: Color = Color::Rgb(0x0A, 0x0A, 0x0A);

/// The settled colour of a drawn border ring. Kept close to `theme::BORDER_FOCUS`
/// (cyan) so the hand-off to the statically-rendered border is seamless when the
/// draw-in completes.
const RING: (u8, u8, u8) = (0x1E, 0xC4, 0xD6);
/// The bright crest of the travelling comet; near-white cyan.
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
        // Slow ping-pong fade on the connection dot: LIVE -> LIVE_DIM -> LIVE.
        //
        // Filter is attached to the *inner* `fade_to_fg`, not to the outer
        // `repeating`/`ping_pong` wrappers. `PingPong` stores a filter but never
        // applies it to the wrapped effect, so an outer filter leaks the fade
        // across the whole header row.
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
        // Header is smaller so it draws in slightly faster.
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

    /// Redraw cadence: fast (~30 fps) while transient effects play, slow (~10
    /// fps) for the connection pulse, and `None` when nothing is animating.
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

        // Pulse only while connected; otherwise the static red dot is left
        // untouched. The FgColor filter confines it to the single LED cell.
        if connected {
            self.pulse.process(dur, buf, header);
        }
    }
}

/// The shimmer's centre colour as HSL (degrees, %, %). At zero amplitude,
/// `hsl_to_rgb` returns exactly `theme::SPLASH_DOT`, so the shimmer begins with
/// no colour jump when the reveal fade hands over.
const SHIMMER_CENTER: (f32, f32, f32) = (186.36, 75.12, 60.59);

/// One full drift of the shimmer wave. The bloom and the steady loop share this
/// cycle, so the wave's *pace* never changes across the hand-off either.
const SHIMMER_CYCLE: (u32, Interpolation) = (2000, Interpolation::Linear);

/// Boot splash reveal: foreground-only fade from `SPLASH_DOT_DIM` to
/// `SPLASH_DOT`, then a seamless hand-off to the repeating [`shimmer`].
///
/// The hand-off is colour- and motion-continuous by construction:
/// - the fade lands on exactly [`SHIMMER_CENTER`] (= `theme::SPLASH_DOT`);
/// - the bloom opens at zero amplitude and eases up over ~20% of one cycle;
/// - the bloom advances one full turn, matching the repeating loop's first frame.
///
/// Standalone effect processed by [`crate::boot`] over the wordmark rect.
pub(crate) fn splash_reveal<T: Into<tachyonfx::EffectTimer>>(timer: T) -> Effect {
    fx::sequence(&[
        fx::fade_from_fg(theme::SPLASH_DOT_DIM, timer),
        // Bloom: amplitude eased 0->full over the first ~20% of a cycle (~400ms),
        // drifting at the steady loop's pace so the shimmer is alive before hand-off.
        shimmer(SHIMMER_CYCLE, |a| smoothstep((a / 0.2).min(1.0))),
        // Steady state: the same wave at full amplitude, looping forever.
        fx::repeating(shimmer(SHIMMER_CYCLE, |_| 1.0)),
    ])
}

/// Hue and brightness wave across the splash dots. `envelope` maps the timer
/// `alpha` to the wave amplitude (`0` = flat [`SHIMMER_CENTER`], `1` = full
/// swing), letting the caller bloom in the wave from nothing then hold it steady.
/// Only `●` cells are painted; empty cells are skipped. Under [`fx::repeating`]
/// the spatial phase advances one full turn per cycle, keeping the loop seamless.
/// Pace is driven by timer alpha, never wall-clock, so it stays deterministic.
fn shimmer<T, E>(timer: T, envelope: E) -> Effect
where
    T: Into<tachyonfx::EffectTimer>,
    E: Fn(f32) -> f32 + Send + 'static,
{
    use std::f32::consts::TAU;
    let (base_h, base_s, base_l) = SHIMMER_CENTER;
    fx::effect_fn_buf((), timer, move |_state, ctx, buf| {
        let area = ctx.area;
        let t = ctx.alpha();
        let amp = envelope(t).clamp(0.0, 1.0);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let Some(cell) = buf.cell_mut((x, y)) else {
                    continue;
                };
                if cell.symbol() != "●" {
                    continue;
                }
                // Phase = spatial gradient minus time, so the wave travels; the
                // `t * TAU` term loops cleanly under `repeating`.
                let col = (x - area.x) as f32;
                let row = (y - area.y) as f32;
                let phase = col * 0.20 + row * 0.55 - t * TAU;
                let hue = base_h + amp * 58.0 * phase.sin();
                // Brightness uses a different phase offset so hue and brightness
                // don't pulse in lockstep.
                let light = base_l + amp * 18.0 * (phase + 1.3).sin();
                cell.set_fg(hsl_to_rgb(hue, base_s, light));
            }
        }
    })
}

/// Smoothstep (`3a²−2a³`): an eased `0→1` ramp with zero slope at both ends, so
/// the shimmer's amplitude opens (and tops out) without a hard edge.
fn smoothstep(a: f32) -> f32 {
    let a = a.clamp(0.0, 1.0);
    a * a * (3.0 - 2.0 * a)
}

/// HSL (`h` degrees, `s`/`l` percent) → an RGB [`Color`]. Used by the splash
/// shimmer to sweep hue and lightness smoothly.
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Color {
    let h = h.rem_euclid(360.0) / 360.0;
    let s = (s / 100.0).clamp(0.0, 1.0);
    let l = (l / 100.0).clamp(0.0, 1.0);
    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return Color::Rgb(v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let channel = |t: f32| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round() as u8
    };
    Color::Rgb(channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
}

/// Comet orbiting a box's border ring - the connecting spinner. Unlike
/// [`border_draw_in`], the ring stays settled; only the comet and its trail are
/// painted, wrapping across corners. Wrap in [`fx::repeating`] for a continuous
/// orbit.
pub(crate) fn orbit<T: Into<tachyonfx::EffectTimer>>(timer: T) -> Effect {
    fx::effect_fn_buf((), timer, move |_state, ctx, buf| {
        let ring = ring_positions(ctx.area);
        let n = ring.len();
        if n == 0 {
            return;
        }
        // Head sweeps 0→n across one timer cycle; `repeating` restarts it, so the
        // comet laps the ring forever at a constant pace (alpha is timer-driven,
        // never wall-clock, so tests stay deterministic).
        let head = ctx.alpha() * n as f32;
        for (i, (x, y)) in ring.into_iter().enumerate() {
            // Circular distance the comet has travelled past this cell; wrapping
            // keeps the trail continuous across the top-left seam.
            let mut d = head - i as f32;
            if d < 0.0 {
                d += n as f32;
            }
            let color = if d < TRAIL {
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
        // Overshoot by one TRAIL so the glow fully exits before the effect
        // completes; last frame is a clean settled ring.
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

    #[test]
    fn shimmer_blooms_out_of_the_settled_splash_colour() {
        // At zero amplitude the wave is exactly the fade's endpoint colour.
        // If someone retunes the shimmer centre and forgets this, the seam
        // reappears; lock it to `theme::SPLASH_DOT`.
        let (h, s, l) = SHIMMER_CENTER;
        assert_eq!(
            hsl_to_rgb(h, s, l),
            theme::SPLASH_DOT,
            "zero-amplitude shimmer must equal the fade's endpoint (no colour pop)"
        );
    }

    #[test]
    fn smoothstep_eases_from_zero_to_one() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(1.0), 1.0);
        assert_eq!(smoothstep(0.5), 0.5, "symmetric midpoint");
        assert!(smoothstep(-1.0) == 0.0 && smoothstep(2.0) == 1.0, "clamped");
    }
}
