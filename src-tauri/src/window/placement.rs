//! Overlay placement (pure). Positions are physical px; the overlay is always kept fully
//! inside the union of the monitors' work areas.

use serde::{Deserialize, Serialize};

use crate::screen::geometry::{monitor_for_rect, primary_index, MonitorGeom, Rect};

/// Gap between the top of the work area and the overlay when placed top-centre (logical px).
pub const TOP_GAP: f64 = 24.0;
/// A saved rect must overlap some work area by at least this much to be restored in place.
pub const MIN_VISIBLE: (i32, i32) = (64, 32);

/// What `<appConfig>/placement.json` stores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPlacement {
    pub x: i32,
    pub y: i32,
    #[serde(default)]
    pub monitor: Option<String>,
    #[serde(default)]
    pub scale: Option<f64>,
}

/// Is every pixel of `r` inside some monitor's work area? (Monitors never overlap.)
pub fn fully_contained(r: &Rect, monitors: &[MonitorGeom]) -> bool {
    let covered: i64 = monitors
        .iter()
        .filter_map(|m| m.work.intersect(r))
        .map(|i| i.area())
        .sum();
    covered >= r.area()
}

/// Move/shrink `r` so it fits inside `work`.
pub fn clamp_into(r: &Rect, work: &Rect) -> Rect {
    let w = r.w.min(work.w).max(1);
    let h = r.h.min(work.h).max(1);
    let x = r.x.clamp(work.x, work.right() - w);
    let y = r.y.clamp(work.y, work.bottom() - h);
    Rect::new(x, y, w, h)
}

/// Top-centre of `m`'s work area for a window of physical size (w, h).
pub fn top_center(m: &MonitorGeom, w: i32, h: i32) -> Rect {
    let r = Rect::new(
        m.work.x + (m.work.w - w) / 2,
        m.work.y + (TOP_GAP * m.scale).round() as i32,
        w,
        h,
    );
    clamp_into(&r, &m.work)
}

/// Physical size of a logical size on a given monitor.
pub fn physical_size(logical: (f64, f64), scale: f64) -> (i32, i32) {
    (
        (logical.0 * scale).round() as i32,
        (logical.1 * scale).round() as i32,
    )
}

/// Where to put the overlay at startup. `logical_size` is the overlay's logical size; the
/// physical size depends on which monitor it lands on.
pub fn restore(
    saved: Option<&SavedPlacement>,
    logical_size: (f64, f64),
    monitors: &[MonitorGeom],
) -> Option<Rect> {
    let primary = primary_index(monitors)?;
    if let Some(s) = saved {
        // find the monitor it would land on using the size at that monitor's scale
        let probe = |scale: f64| {
            let (w, h) = physical_size(logical_size, scale);
            Rect::new(s.x, s.y, w, h)
        };
        let guess = probe(s.scale.unwrap_or(monitors[primary].scale));
        if let Some(i) = monitor_for_rect(monitors, &guess) {
            let m = &monitors[i];
            let r = probe(m.scale);
            if let Some(o) = m.work.intersect(&r) {
                if o.w >= MIN_VISIBLE.0 && o.h >= MIN_VISIBLE.1 {
                    return Some(clamp_into(&r, &m.work));
                }
            }
        }
    }
    let m = &monitors[primary];
    let (w, h) = physical_size(logical_size, m.scale);
    Some(top_center(m, w, h))
}

/// Re-fit after a display change or resize: keep `r` if it's fully visible, else clamp it
/// into the work area of the monitor it mostly sits on.
pub fn reclamp(r: &Rect, monitors: &[MonitorGeom]) -> Rect {
    if monitors.is_empty() || fully_contained(r, monitors) {
        return *r;
    }
    match monitor_for_rect(monitors, r) {
        Some(i) => clamp_into(r, &monitors[i].work),
        None => *r,
    }
}

/// Move by (dx, dy) without leaving the visible desktop. Crossing to an adjacent monitor works
/// as long as the window stays fully visible; at an outer edge it moves as far as it can.
pub fn move_by(r: &Rect, dx: i32, dy: i32, monitors: &[MonitorGeom]) -> Rect {
    if monitors.is_empty() {
        return r.translate(dx, dy);
    }
    let start = reclamp(r, monitors);
    let ok = |c: &Rect| fully_contained(c, monitors);
    let full = start.translate(dx, dy);
    if ok(&full) {
        return full;
    }
    // try the largest partial step on each axis independently
    let step_axis = |from: Rect, d: i32, horizontal: bool| -> Rect {
        let mut k = d.abs();
        while k > 0 {
            let s = d.signum() * k;
            let c = if horizontal {
                from.translate(s, 0)
            } else {
                from.translate(0, s)
            };
            if ok(&c) {
                return c;
            }
            k -= 1;
        }
        from
    };
    let after_x = step_axis(start, dx, true);
    step_axis(after_x, dy, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::geometry::fixtures::mixed_dpi;

    #[test]
    fn restore_without_saved_goes_top_center_of_primary() {
        let m = mixed_dpi();
        let r = restore(None, (440.0, 560.0), &m).unwrap();
        // primary is 4K@200% -> 880x1120 physical
        assert_eq!((r.w, r.h), (880, 1120));
        assert_eq!(r.x, (3840 - 880) / 2);
        assert_eq!(r.y, 48);
    }

    #[test]
    fn restore_keeps_valid_position_with_target_monitor_scale() {
        let m = mixed_dpi();
        let saved = SavedPlacement {
            x: 4000,
            y: 600,
            monitor: Some(r"\\.\DISPLAY2".into()),
            scale: Some(1.0),
        };
        let r = restore(Some(&saved), (440.0, 560.0), &m).unwrap();
        assert_eq!(r, Rect::new(4000, 600, 440, 560));
        // on the negative-x 125% monitor
        let saved = SavedPlacement {
            x: -1800,
            y: 100,
            monitor: None,
            scale: Some(1.25),
        };
        let r = restore(Some(&saved), (440.0, 560.0), &m).unwrap();
        assert_eq!(r, Rect::new(-1800, 100, 550, 700));
    }

    #[test]
    fn restore_clamps_partially_offscreen_and_resets_lost_monitor() {
        let m = mixed_dpi();
        // hanging off the bottom of DISPLAY2 but visible enough -> clamped inside its work area
        let saved = SavedPlacement {
            x: 5000,
            y: 1400,
            monitor: None,
            scale: Some(1.0),
        };
        let r = restore(Some(&saved), (440.0, 560.0), &m).unwrap();
        assert!(fully_contained(&r, &m));
        assert_eq!(r, Rect::new(5000, 540 + 1040 - 560, 440, 560));
        // on a monitor that no longer exists (far away) -> top-centre of primary
        let saved = SavedPlacement {
            x: 20000,
            y: 20000,
            monitor: Some(r"\\.\DISPLAY9".into()),
            scale: Some(1.0),
        };
        let r = restore(Some(&saved), (440.0, 560.0), &m).unwrap();
        assert_eq!(r.y, 48);
        assert_eq!(r.w, 880);
        assert!(restore(None, (440.0, 560.0), &[]).is_none());
    }

    #[test]
    fn move_within_and_across_monitors() {
        let m = mixed_dpi();
        let r = Rect::new(100, 100, 400, 300);
        assert_eq!(move_by(&r, 12, 0, &m), Rect::new(112, 100, 400, 300));
        // at the left edge of DISPLAY1 it can continue onto DISPLAY3 (y range 0..1032 overlaps)
        let r = Rect::new(0, 100, 400, 300);
        assert_eq!(move_by(&r, -12, 0, &m), Rect::new(-12, 100, 400, 300));
        // at the far-left edge of DISPLAY3 it stops exactly at the edge
        let r = Rect::new(-1915, 100, 400, 300);
        assert_eq!(move_by(&r, -12, 0, &m), Rect::new(-1920, 100, 400, 300));
        // can't cross into DISPLAY2 at a height DISPLAY2 doesn't cover: stops at the boundary
        let r = Rect::new(3430, 100, 400, 300);
        assert_eq!(move_by(&r, 12, 0, &m), Rect::new(3440, 100, 400, 300));
        // ...but can cross where both monitors cover the rows
        let r = Rect::new(3440, 700, 400, 300);
        assert_eq!(move_by(&r, 12, 0, &m), Rect::new(3452, 700, 400, 300));
        // diagonal near a corner moves as far as possible on each axis
        let r = Rect::new(5, 5, 100, 100);
        assert_eq!(move_by(&r, -12, -12, &m), Rect::new(-7, 0, 100, 100));
    }

    #[test]
    fn reclamp_pulls_window_back() {
        let m = mixed_dpi();
        let r = Rect::new(3700, 2000, 400, 300); // off the bottom of DISPLAY1
        let c = reclamp(&r, &m);
        assert!(fully_contained(&c, &m));
        assert_eq!(c, Rect::new(3440, 2080 - 300, 400, 300));
        let inside = Rect::new(10, 10, 10, 10);
        assert_eq!(reclamp(&inside, &m), inside);
    }

    #[test]
    fn clamp_shrinks_oversized() {
        let work = Rect::new(0, 0, 1920, 1040);
        assert_eq!(
            clamp_into(&Rect::new(-50, -50, 3000, 2000), &work),
            Rect::new(0, 0, 1920, 1040)
        );
    }
}
