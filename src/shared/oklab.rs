//! Colour arithmetic for the themes (spec §11.6, docs/theme-modes.md §10.2):
//! the WCAG contrast ratio the floors are stated in, and OKLab, the space a
//! colour's lightness is moved in when it has to clear one.
//!
//! OKLab rather than HSL or a scaling of the RGB channels, because its
//! lightness is *perceptual*: moving `L` and keeping `a` and `b` gives the
//! same colour, lighter — which is what "just far enough to clear the floor"
//! means to the eye. The built-in palettes were retuned this way by hand
//! (docs/theme-modes.md §3.1); a user theme's missing roles are fitted the
//! same way by [`fit`].
//!
//! Björn Ottosson's matrices, <https://bottosson.github.io/posts/oklab/>.

use crate::shared::osc11::{Rgb, relative_luminance};

/// WCAG 2.x contrast ratio of two colours, 1 (none) to 21 (black on white).
/// Symmetric.
pub fn contrast_ratio(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Whether a background is a **dark** one: white stands out against it more
/// than black does. The rule the built-in canvases are held to, and the one
/// that decides which of them a user theme's missing roles start from.
pub fn is_dark(background: Rgb) -> bool {
    let white = Rgb {
        r: 255,
        g: 255,
        b: 255,
    };
    let black = Rgb { r: 0, g: 0, b: 0 };
    contrast_ratio(background, white) > contrast_ratio(background, black)
}

/// A colour in OKLab: `l` is lightness, 0 (black) to 1 (white); `a` and `b`
/// carry hue and chroma.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

fn to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let c = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

impl Oklab {
    pub fn from_rgb(rgb: Rgb) -> Self {
        let (r, g, b) = (to_linear(rgb.r), to_linear(rgb.g), to_linear(rgb.b));
        let l = (0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        Self {
            l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        }
    }

    /// The linear-light channels, which leave `0..=1` when no display can
    /// give this hue at this chroma and lightness.
    fn linear(self) -> [f32; 3] {
        let l = (self.l + 0.396_337_78 * self.a + 0.215_803_76 * self.b).powi(3);
        let m = (self.l - 0.105_561_346 * self.a - 0.063_854_17 * self.b).powi(3);
        let s = (self.l - 0.089_484_18 * self.a - 1.291_485_5 * self.b).powi(3);
        [
            4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
            -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
            -0.004_196_086_4 * l - 0.703_418_6 * m + 1.707_614_7 * s,
        ]
    }

    fn in_gamut(self) -> bool {
        // Less than rounding to 8 bits absorbs, at either end of the scale —
        // near black a linear 0.0001 is a third of a step.
        const SLACK: f32 = 0.0001;
        self.linear()
            .iter()
            .all(|c| (-SLACK..=1.0 + SLACK).contains(c))
    }

    /// The colour a display shows for this one. Out of gamut, the **chroma**
    /// gives way and the lightness holds: the hue is kept as far towards
    /// white or black as it can be, and at the ends of the scale it is white
    /// or black. Clipping the channels instead would keep a tint at the top
    /// of the scale, and with it a ceiling on the contrast a fit can reach.
    pub fn to_rgb(self) -> Rgb {
        let mut fits = self;
        if !self.in_gamut() {
            // The largest share of the chroma that is in gamut.
            let (mut low, mut high) = (0.0f32, 1.0f32);
            for _ in 0..24 {
                let mid = (low + high) / 2.0;
                let scaled = Self {
                    a: self.a * mid,
                    b: self.b * mid,
                    ..self
                };
                if scaled.in_gamut() {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            fits = Self {
                a: self.a * low,
                b: self.b * low,
                ..self
            };
        }
        let [r, g, b] = fits.linear();
        Rgb {
            r: from_linear(r),
            g: from_linear(g),
            b: from_linear(b),
        }
    }

    /// The same hue and chroma at another lightness.
    pub fn at(self, l: f32) -> Self {
        Self {
            l: l.clamp(0.0, 1.0),
            ..self
        }
    }
}

/// How far a lightness is moved at a time while fitting. Fine enough that the
/// colour found is within a rounding of the first one that clears.
const STEP: f32 = 0.002;

/// A colour fitted to a floor, and whether it got there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fitted {
    pub color: Rgb,
    /// `false` — no lightness of this hue clears the floor on every ground;
    /// `color` is then the best one met.
    pub clears: bool,
}

/// The least contrast `color` has against any of `grounds`.
pub fn least_contrast(color: Rgb, grounds: &[Rgb]) -> f32 {
    grounds
        .iter()
        .map(|g| contrast_ratio(color, *g))
        .fold(f32::INFINITY, f32::min)
}

/// `start`, moved along OKLab lightness — hue and chroma kept — by the
/// **smallest** amount that gives it a contrast of at least `floor` against
/// every one of `grounds`. A colour that already clears is returned as it is.
///
/// The move goes away from the grounds first: lighter when `lighter`, which
/// is the direction text takes on a dark canvas. If the whole way to the end
/// of the scale clears nothing, the other direction is tried; failing both,
/// the best colour met on the way is returned with `clears: false`.
pub fn fit(start: Rgb, grounds: &[Rgb], floor: f32, lighter: bool) -> Fitted {
    if least_contrast(start, grounds) >= floor {
        return Fitted {
            color: start,
            clears: true,
        };
    }
    let from = Oklab::from_rgb(start);
    let mut best = (least_contrast(start, grounds), start);
    for direction in [lighter, !lighter] {
        let sign = if direction { 1.0 } else { -1.0 };
        let mut l = from.l;
        while (0.0..=1.0).contains(&l) {
            let color = from.at(l).to_rgb();
            let got = least_contrast(color, grounds);
            if got >= floor {
                return Fitted {
                    color,
                    clears: true,
                };
            }
            if got > best.0 {
                best = (got, color);
            }
            l += sign * STEP;
        }
    }
    Fitted {
        color: best.1,
        clears: false,
    }
}

/// `background`, moved in lightness by `delta` — its own hue and chroma kept.
/// How a role that is not text is derived for a canvas it was not chosen for:
/// it keeps its distance from the canvas (docs/theme-modes.md §10.2).
pub fn shifted(background: Rgb, delta: f32) -> Rgb {
    let lab = Oklab::from_rgb(background);
    lab.at(lab.l + delta).to_rgb()
}

/// The OKLab lightness `color` stands off `background` by; negative when it is
/// the darker of the two.
pub fn lightness_offset(color: Rgb, background: Rgb) -> f32 {
    Oklab::from_rgb(color).l - Oklab::from_rgb(background).l
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }
    const BLACK: Rgb = rgb(0, 0, 0);
    const WHITE: Rgb = rgb(255, 255, 255);

    #[test]
    fn the_contrast_ratio_is_the_wcag_one() {
        assert!((contrast_ratio(BLACK, WHITE) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(WHITE, BLACK) - 21.0).abs() < 0.01, "order");
        assert!((contrast_ratio(WHITE, WHITE) - 1.0).abs() < 0.001);
        // Measured outside the code (docs/theme-modes.md §3.1): the dark
        // palette's body text on its canvas.
        let got = contrast_ratio(rgb(201, 204, 210), rgb(15, 17, 21));
        assert!((got - 11.74).abs() < 0.01, "{got}");
    }

    #[test]
    fn a_background_is_dark_when_white_reads_better_on_it() {
        assert!(is_dark(BLACK));
        assert!(!is_dark(WHITE));
        assert!(is_dark(rgb(15, 17, 21)), "the dark canvas");
        assert!(!is_dark(rgb(250, 250, 252)), "the light canvas");
        assert!(is_dark(rgb(0, 43, 54)), "Solarized's dark base");
        // Mid grey is the light side of the line: black on it is 5.3:1,
        // white 3.9:1.
        assert!(!is_dark(rgb(128, 128, 128)));
        assert!(is_dark(rgb(100, 100, 100)));
    }

    #[test]
    fn oklab_round_trips_and_knows_its_ends() {
        let lab = Oklab::from_rgb(WHITE);
        assert!((lab.l - 1.0).abs() < 0.001, "{lab:?}");
        assert!(lab.a.abs() < 0.001 && lab.b.abs() < 0.001, "{lab:?}");
        assert!(Oklab::from_rgb(BLACK).l.abs() < 0.001);
        // Values from the reference implementation's table: sRGB red.
        let red = Oklab::from_rgb(rgb(255, 0, 0));
        assert!((red.l - 0.628).abs() < 0.001, "{red:?}");
        assert!((red.a - 0.225).abs() < 0.001, "{red:?}");
        assert!((red.b - 0.126).abs() < 0.001, "{red:?}");

        for color in [
            rgb(121, 169, 219),
            rgb(0, 116, 0),
            rgb(223, 105, 92),
            rgb(133, 139, 146),
            rgb(1, 2, 3),
            rgb(254, 253, 252),
        ] {
            assert_eq!(Oklab::from_rgb(color).to_rgb(), color);
        }
    }

    #[test]
    fn out_of_gamut_the_chroma_gives_way_and_the_lightness_holds() {
        // A saturated blue cannot be as light as white and stay that blue: at
        // the ends of the scale it is white and black, with no tint left to
        // cap the contrast.
        let blue = Oklab::from_rgb(rgb(0, 0, 255));
        assert_eq!(blue.at(1.0).to_rgb(), WHITE);
        // Black to a rounding: near the bottom of the scale one step of a
        // channel is less than the gamut check can tell from nothing.
        let darkest = blue.at(0.0).to_rgb();
        assert!(contrast_ratio(darkest, BLACK) < 1.01, "{darkest:?}");
        assert_eq!(blue.at(7.0), blue.at(1.0), "lightness is kept on its scale");

        // On the way there it is still a blue, and as light as was asked.
        let light = blue.at(0.85).to_rgb();
        assert!(light.b > light.r && light.b > light.g, "{light:?}");
        assert!((Oklab::from_rgb(light).l - 0.85).abs() < 0.01, "{light:?}");
        // Lighter is lighter, all the way up — what a fit walks along.
        let mut last = 0.0;
        for step in 0..=50 {
            let l = step as f32 / 50.0;
            let now = relative_luminance(blue.at(l).to_rgb());
            assert!(now >= last - 0.001, "at {l}: {now} after {last}");
            last = now;
        }
    }

    /// The moves of docs/theme-modes.md §3.1 were made by hand with this
    /// arithmetic in another language; the fit has to find the same colours.
    #[test]
    fn the_fit_finds_what_the_built_in_palettes_were_retuned_to() {
        let dark = [rgb(15, 17, 21), rgb(33, 36, 42)];
        let light = [rgb(250, 250, 252), rgb(222, 224, 228)];
        for (was, grounds, lighter, becomes) in [
            (rgb(126, 132, 139), dark, true, rgb(133, 139, 146)), // dark, muted
            (rgb(128, 128, 128), dark, true, rgb(138, 138, 138)), // dark, comments
            (rgb(0, 128, 0), light, false, rgb(0, 116, 0)),       // light, assistant
            (rgb(110, 116, 124), light, false, rgb(95, 100, 108)), // light, muted
            (rgb(110, 110, 110), light, false, rgb(99, 99, 99)),  // light, comments
        ] {
            let fitted = fit(was, &grounds, 4.5, lighter);
            assert!(fitted.clears, "{was:?}");
            assert!(least_contrast(fitted.color, &grounds) >= 4.5);
            let off = |a: u8, b: u8| a.abs_diff(b);
            assert!(
                off(fitted.color.r, becomes.r) <= 2
                    && off(fitted.color.g, becomes.g) <= 2
                    && off(fitted.color.b, becomes.b) <= 2,
                "{was:?} was retuned to {becomes:?}, fitted to {:?}",
                fitted.color
            );
        }
    }

    #[test]
    fn a_colour_that_clears_is_not_touched() {
        let grounds = [rgb(15, 17, 21), rgb(33, 36, 42)];
        let text = rgb(201, 204, 210);
        assert_eq!(
            fit(text, &grounds, 7.0, true),
            Fitted {
                color: text,
                clears: true
            }
        );
    }

    #[test]
    fn the_fit_is_the_smallest_move_that_clears() {
        let grounds = [rgb(15, 17, 21), rgb(33, 36, 42)];
        let start = rgb(90, 70, 60);
        let fitted = fit(start, &grounds, 4.5, true);
        assert!(fitted.clears);
        // One step back is under the floor…
        let lab = Oklab::from_rgb(fitted.color);
        let before = lab.at(lab.l - 2.0 * STEP).to_rgb();
        assert!(least_contrast(before, &grounds) < 4.5, "{before:?}");
        // …and the hue is the one it started with: still the reddest channel.
        assert!(fitted.color.r > fitted.color.g && fitted.color.g > fitted.color.b);
    }

    #[test]
    fn the_fit_turns_round_when_its_own_direction_has_no_room() {
        // Asked to go darker on a dark ground: nothing down there clears, and
        // the way up does.
        let grounds = [rgb(15, 17, 21)];
        let fitted = fit(rgb(60, 60, 60), &grounds, 4.5, false);
        assert!(fitted.clears);
        assert!(fitted.color.r > 60, "{:?}", fitted.color);
    }

    /// On a ground in the middle of the scale a floor can be cleared either
    /// way — black on mid grey is 5.3:1, white 3.9:1 — and then the direction
    /// asked for decides: text goes away from its canvas, not towards
    /// whichever end the scan happens to try first.
    #[test]
    fn where_both_ways_clear_the_fit_goes_the_way_it_was_asked() {
        let grounds = [rgb(128, 128, 128)];
        let start = rgb(120, 130, 125);
        assert!(least_contrast(start, &grounds) < 1.1);
        let (down, up) = (
            fit(start, &grounds, 3.0, false),
            fit(start, &grounds, 3.0, true),
        );
        assert!(down.clears && up.clears);
        let l = |c: Rgb| Oklab::from_rgb(c).l;
        assert!(l(down.color) < l(start), "{:?}", down.color);
        assert!(l(up.color) > l(start), "{:?}", up.color);
        for fitted in [down, up] {
            assert!(least_contrast(fitted.color, &grounds) >= 3.0);
        }
    }

    #[test]
    fn a_floor_nothing_clears_is_said_and_the_best_is_kept() {
        // Mid grey and near-black as the two grounds: no colour is 7:1 on
        // both — white is 3.9:1 on the grey, black is nothing on the black.
        let grounds = [rgb(128, 128, 128), rgb(10, 10, 10)];
        let fitted = fit(rgb(128, 128, 128), &grounds, 7.0, true);
        assert!(!fitted.clears);
        let got = least_contrast(fitted.color, &grounds);
        assert!(got > 3.5, "the best met, not the start: {got}");
        assert!(got < 7.0);
    }

    #[test]
    fn a_shifted_background_keeps_its_distance_and_its_hue() {
        let (canvas, backdrop) = (rgb(15, 17, 21), rgb(33, 36, 42));
        let delta = lightness_offset(backdrop, canvas);
        assert!(delta > 0.0, "the dark theme's backdrop is the lighter");
        // On its own canvas the shift is the backdrop, to a rounding of hue.
        let again = shifted(canvas, delta);
        assert!(contrast_ratio(again, backdrop) < 1.02, "{again:?}");
        // On another canvas: as far off it, in its colour.
        let solarized = rgb(0, 43, 54);
        let derived = shifted(solarized, delta);
        assert!((lightness_offset(derived, solarized) - delta).abs() < 0.01);
        assert!(derived.b > derived.r, "still a blue-green: {derived:?}");
        assert!(contrast_ratio(derived, solarized) > 1.15);
    }
}
