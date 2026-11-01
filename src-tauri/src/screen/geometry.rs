//! Monitor geometry (pure). All rects here are in **physical** pixels unless noted.
//!
//! xcap and tauri both name monitors by `szDevice` (e.g. `\\.\DISPLAY1`), so monitors are paired
//! by name, never by index. Scale factors come from tauri (xcap's are not trusted).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn area(&self) -> i64 {
        self.w.max(0) as i64 * self.h.max(0) as i64
    }
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }
    pub fn translate(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
}

/// One display, as tauri reports it (bounds + work area physical, scale from the OS).
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorGeom {
    pub name: String,
    pub bounds: Rect,
    pub work: Rect,
    pub scale: f64,
    pub primary: bool,
}

/// Index of the monitor containing a physical point.
pub fn monitor_at_point(monitors: &[MonitorGeom], px: i32, py: i32) -> Option<usize> {
    monitors.iter().position(|m| m.bounds.contains(px, py))
}

/// Monitor with the largest overlap with `r`; if none overlaps, the one whose centre is
/// nearest to `r`'s centre. (Same rule the old capture.rs:318-359 used.)
pub fn monitor_for_rect(monitors: &[MonitorGeom], r: &Rect) -> Option<usize> {
    let best = monitors
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.bounds.intersect(r).map(|o| (i, o.area())))
        .max_by_key(|&(_, a)| a)
        .map(|(i, _)| i);
    best.or_else(|| nearest_monitor(monitors, r.center()))
}

pub fn nearest_monitor(monitors: &[MonitorGeom], (cx, cy): (i32, i32)) -> Option<usize> {
    monitors
        .iter()
        .enumerate()
        .min_by_key(|(_, m)| {
            let (mx, my) = m.bounds.center();
            let (dx, dy) = ((cx - mx) as i64, (cy - my) as i64);
            dx * dx + dy * dy
        })
        .map(|(i, _)| i)
}

pub fn primary_index(monitors: &[MonitorGeom]) -> Option<usize> {
    monitors
        .iter()
        .position(|m| m.primary)
        .or(if monitors.is_empty() { None } else { Some(0) })
}

/// A selection rect in CSS px of a full-monitor capture overlay.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
pub struct CssRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Convert a CSS-px selection on a monitor's overlay into a crop rect in the captured image.
///
/// The overlay covers the monitor exactly, so CSS px × the monitor's scale = image px. The
/// result is clamped to the image; `None` if nothing (≥ 2×2 px) is left.
pub fn css_to_image_crop(
    sel: CssRect,
    scale: f64,
    img_w: u32,
    img_h: u32,
) -> Option<(u32, u32, u32, u32)> {
    if !(sel.x.is_finite() && sel.y.is_finite() && sel.width.is_finite() && sel.height.is_finite())
    {
        return None;
    }
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    // normalise negative widths (drag up/left)
    let (x, w) = if sel.width < 0.0 {
        (sel.x + sel.width, -sel.width)
    } else {
        (sel.x, sel.width)
    };
    let (y, h) = if sel.height < 0.0 {
        (sel.y + sel.height, -sel.height)
    } else {
        (sel.y, sel.height)
    };
    let x0 = (x * scale).round().max(0.0);
    let y0 = (y * scale).round().max(0.0);
    let x1 = ((x + w) * scale).round().min(img_w as f64);
    let y1 = ((y + h) * scale).round().min(img_h as f64);
    if x1 - x0 < 2.0 || y1 - y0 < 2.0 {
        return None;
    }
    Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

#[cfg(test)]
pub mod fixtures {
    use super::*;

    /// 4K @ 200% at (0,0) + 1080p @ 100% at (3840,540) + 1080p @ 125% at negative x.
    pub fn mixed_dpi() -> Vec<MonitorGeom> {
        vec![
            MonitorGeom {
                name: r"\\.\DISPLAY1".into(),
                bounds: Rect::new(0, 0, 3840, 2160),
                work: Rect::new(0, 0, 3840, 2080),
                scale: 2.0,
                primary: true,
            },
            MonitorGeom {
                name: r"\\.\DISPLAY2".into(),
                bounds: Rect::new(3840, 540, 1920, 1080),
                work: Rect::new(3840, 540, 1920, 1040),
                scale: 1.0,
                primary: false,
            },
            MonitorGeom {
                name: r"\\.\DISPLAY3".into(),
                bounds: Rect::new(-1920, 0, 1920, 1080),
                work: Rect::new(-1920, 0, 1920, 1032),
                scale: 1.25,
                primary: false,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::mixed_dpi;
    use super::*;

    #[test]
    fn point_lookup_across_mixed_dpi() {
        let m = mixed_dpi();
        assert_eq!(monitor_at_point(&m, 10, 10), Some(0));
        assert_eq!(monitor_at_point(&m, 3839, 2159), Some(0));
        assert_eq!(monitor_at_point(&m, 3840, 600), Some(1));
        assert_eq!(monitor_at_point(&m, 3840, 100), None); // above DISPLAY2
        assert_eq!(monitor_at_point(&m, -5, 500), Some(2));
    }

    #[test]
    fn rect_lookup_prefers_overlap_then_nearest() {
        let m = mixed_dpi();
        // straddles DISPLAY1/DISPLAY2, mostly on 2
        assert_eq!(
            monitor_for_rect(&m, &Rect::new(3700, 700, 600, 400)),
            Some(1)
        );
        // mostly on 1
        assert_eq!(
            monitor_for_rect(&m, &Rect::new(3500, 700, 600, 400)),
            Some(0)
        );
        // entirely off-screen to the far right -> nearest is DISPLAY2
        assert_eq!(
            monitor_for_rect(&m, &Rect::new(9000, 900, 100, 100)),
            Some(1)
        );
        assert_eq!(primary_index(&m), Some(0));
        assert_eq!(primary_index(&[]), None);
    }

    #[test]
    fn crop_uses_the_monitors_scale() {
        // 200% monitor: CSS (100,50) 300x200 -> image (200,100) 600x400
        let sel = CssRect {
            x: 100.0,
            y: 50.0,
            width: 300.0,
            height: 200.0,
        };
        assert_eq!(
            css_to_image_crop(sel, 2.0, 3840, 2160),
            Some((200, 100, 600, 400))
        );
        // 125% monitor rounds to nearest px
        let sel = CssRect {
            x: 10.0,
            y: 10.0,
            width: 101.0,
            height: 33.0,
        };
        assert_eq!(
            css_to_image_crop(sel, 1.25, 1920, 1080),
            Some((13, 13, 126, 41))
        );
    }

    #[test]
    fn crop_clamps_and_normalises() {
        // drag beyond the right/bottom edge is clamped
        let sel = CssRect {
            x: 1800.0,
            y: 1000.0,
            width: 400.0,
            height: 400.0,
        };
        assert_eq!(
            css_to_image_crop(sel, 1.0, 1920, 1080),
            Some((1800, 1000, 120, 80))
        );
        // negative width/height (dragged up-left)
        let sel = CssRect {
            x: 300.0,
            y: 300.0,
            width: -100.0,
            height: -50.0,
        };
        assert_eq!(
            css_to_image_crop(sel, 1.0, 1920, 1080),
            Some((200, 250, 100, 50))
        );
        // negative origin clamps to 0
        let sel = CssRect {
            x: -20.0,
            y: -20.0,
            width: 40.0,
            height: 40.0,
        };
        assert_eq!(css_to_image_crop(sel, 1.0, 100, 100), Some((0, 0, 20, 20)));
        // degenerate
        let sel = CssRect {
            x: 5.0,
            y: 5.0,
            width: 0.5,
            height: 50.0,
        };
        assert_eq!(css_to_image_crop(sel, 1.0, 100, 100), None);
        let sel = CssRect {
            x: f64::NAN,
            y: 5.0,
            width: 10.0,
            height: 50.0,
        };
        assert_eq!(css_to_image_crop(sel, 1.0, 100, 100), None);
        // fully outside
        let sel = CssRect {
            x: 500.0,
            y: 5.0,
            width: 10.0,
            height: 50.0,
        };
        assert_eq!(css_to_image_crop(sel, 1.0, 100, 100), None);
    }

    #[test]
    fn rect_ops() {
        let a = Rect::new(0, 0, 10, 10);
        assert_eq!(
            a.intersect(&Rect::new(5, 5, 10, 10)),
            Some(Rect::new(5, 5, 5, 5))
        );
        assert_eq!(a.intersect(&Rect::new(10, 0, 5, 5)), None);
        assert_eq!(a.translate(-3, 4), Rect::new(-3, 4, 10, 10));
        assert_eq!(a.center(), (5, 5));
    }
}
