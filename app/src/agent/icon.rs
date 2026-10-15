//! Slat's menu-bar mark, drawn at 2x into RGBA: four slats, the third solid
//! and taller, the rest outlined. Used as a template image (alpha only).

use tray_icon::Icon;

const SCALE: f64 = 2.0;
/// Points. 22 pt tall (the status bar's max) with the glyph centred in it;
/// edges sit on the 2x pixel grid.
const W: f64 = 20.0;
const H: f64 = 22.0;
const SLAT_W: f64 = 3.0;
const GAP: f64 = 1.5;
const X0: f64 = 1.0;
const STROKE: f64 = 1.0;
/// Badge dot centre and radii (cleared ring, then the dot).
const BADGE: (f64, f64) = (17.0, 5.0);
const BADGE_CLEAR: f64 = 4.5;
const BADGE_DOT: f64 = 3.0;

fn in_rect(px: f64, py: f64, (x, y, w, h): (f64, f64, f64, f64), r: f64) -> bool {
    let dx = (x + r - px).max(px - (x + w - r)).max(0.0);
    let dy = (y + r - py).max(py - (y + h - r)).max(0.0);
    px >= x && px <= x + w && py >= y && py <= y + h && dx * dx + dy * dy <= r * r
}

fn covered(px: f64, py: f64, badge: bool) -> bool {
    if badge {
        let d = (px - BADGE.0).hypot(py - BADGE.1);
        if d <= BADGE_DOT {
            return true;
        }
        if d <= BADGE_CLEAR {
            return false;
        }
    }
    (0..4).any(|i| {
        let x = X0 + f64::from(i) * (SLAT_W + GAP);
        let tall = i == 2;
        let h = if tall { 14.0 } else { 10.0 };
        let outer = (x, (H - h) / 2.0, SLAT_W, h);
        let inner = (outer.0 + STROKE, outer.1 + STROKE, SLAT_W - 2.0 * STROKE, h - 2.0 * STROKE);
        in_rect(px, py, outer, 1.0) && (tall || !in_rect(px, py, inner, 0.0))
    })
}

pub fn rgba(badge: bool) -> (Vec<u8>, u32, u32) {
    const SS: u32 = 4; // supersamples per axis
    let (w, h) = ((W * SCALE) as u32, (H * SCALE) as u32);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let mut hits = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = (f64::from(x) + (f64::from(sx) + 0.5) / f64::from(SS)) / SCALE;
                    let py = (f64::from(y) + (f64::from(sy) + 0.5) / f64::from(SS)) / SCALE;
                    hits += u32::from(covered(px, py, badge));
                }
            }
            out.extend_from_slice(&[0, 0, 0, (hits * 255 / (SS * SS)) as u8]);
        }
    }
    (out, w, h)
}

pub fn glyph(badge: bool) -> Icon {
    let (data, w, h) = rgba(badge);
    Icon::from_rgba(data, w, h).expect("valid icon size")
}
