//! Pure geometry helpers (AF-8): edge-zone hit test, overlay width, and
//! monitor-local anchor/margin computation for the layer-shell overlay.
//!
//! Everything here is deterministic and side-effect free so it can be unit
//! tested without a running compositor.

/// A rectangle in pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// The corner of the tracked window where the overlay bar is anchored.
///
/// Maps from the `from_overlay` config field. Note: this is distinct from
/// `from_area` (the edge-zone side used by [`in_edge_zone`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    BotLeft,
    BotRight,
    TopLeft,
    TopRight,
}

impl Anchor {
    /// Parse a `from_overlay` config value. Unknown values fall back to `BotLeft`.
    pub fn from_config(s: &str) -> Anchor {
        match s {
            "bot_left" => Anchor::BotLeft,
            "bot_right" => Anchor::BotRight,
            "top_left" => Anchor::TopLeft,
            "top_right" => Anchor::TopRight,
            _ => Anchor::BotLeft,
        }
    }
}

/// Default overlay bar height in pixels (kept from the original implementation).
pub const OVERLAY_HEIGHT: i32 = 36;

/// Fallback overlay width when a fixed pixel `overlay_size` cannot be parsed.
pub const DEFAULT_OVERLAY_WIDTH: i32 = 250;

/// Compute the size of the mouse "change zone" along the relevant axis.
///
/// For `left`/`right` areas the zone runs along the window width; for
/// `top`/`bottom` it runs along the height. The zone is `fraction` of that
/// dimension, floored at `min_px`.
fn change_zone_size(win: Rect, from_area: &str, fraction: f64, min_px: i32) -> i32 {
    let dimension = match from_area {
        "top" | "bottom" => win.height,
        // "left"/"right" and any unknown value default to width
        _ => win.width,
    };
    if dimension <= 0 {
        return min_px.max(0);
    }
    let by_fraction = (dimension as f64 * fraction) as i32;
    by_fraction.max(min_px)
}

/// Is the cursor within the configured edge zone of the window?
///
/// `from_area` is one of `left`/`right`/`top`/`bottom` (unknown -> `left`).
/// A zero-sized window is never a hit (guards against divide/overflow).
pub fn in_edge_zone(
    win: Rect,
    cursor: (i32, i32),
    from_area: &str,
    fraction: f64,
    min_px: i32,
) -> bool {
    if win.width <= 0 || win.height <= 0 {
        return false;
    }
    let (cx, cy) = cursor;
    let max_allowed = change_zone_size(win, from_area, fraction, min_px);

    match from_area {
        "right" => {
            let dist = (win.x + win.width) - cx;
            dist >= 0 && dist <= max_allowed
        }
        "top" => {
            let dist = cy - win.y;
            dist >= 0 && dist <= max_allowed
        }
        "bottom" => {
            let dist = (win.y + win.height) - cy;
            dist >= 0 && dist <= max_allowed
        }
        // "left" and any unknown value
        _ => {
            let dist = cx - win.x;
            dist >= 0 && dist <= max_allowed
        }
    }
}

/// Compute the overlay bar width from config.
///
/// `overlay_size` is either `change_area_x`, `change_area_y`, or a fixed pixel
/// string. For the `change_area_*` variants the width is the change-zone size
/// minus `2 * offset_x`. An unparseable fixed value falls back to
/// [`DEFAULT_OVERLAY_WIDTH`]. The result is floored at 1 so the surface is
/// never zero/negative width.
pub fn overlay_width(
    win: Rect,
    overlay_size: &str,
    from_area: &str,
    fraction: f64,
    min_px: i32,
    offset_x: i32,
) -> i32 {
    let width = match overlay_size {
        "change_area_x" => {
            let zone = change_zone_size(win, from_area, fraction, min_px);
            zone - (2 * offset_x)
        }
        "change_area_y" => {
            // Perpendicular dimension: swap the area axis.
            let perp_area = match from_area {
                "top" | "bottom" => "left",
                _ => "top",
            };
            let zone = change_zone_size(win, perp_area, fraction, min_px);
            zone - (2 * offset_x)
        }
        fixed => fixed.parse::<i32>().unwrap_or(DEFAULT_OVERLAY_WIDTH),
    };
    width.max(1)
}

/// Compute the layer-shell anchor and monitor-local margins that place the
/// overlay bar at the configured corner of the tracked window.
///
/// `win` and `monitor` are both in global coordinates; the returned margins are
/// relative to the anchored monitor edges. Margins are clamped to `[0, size]`.
/// For a window spanning multiple monitors the caller should pass the monitor
/// containing the window's top-left corner.
pub fn compute_anchor_margins(
    win: Rect,
    monitor: Rect,
    from_overlay: &str,
    offset_x: i32,
    offset_y: i32,
    overlay_width: i32,
    overlay_height: i32,
) -> (Anchor, i32, i32) {
    let anchor = Anchor::from_config(from_overlay);

    // Window position in monitor-local coordinates.
    let lx = win.x - monitor.x;
    let ty = win.y - monitor.y;

    // Overlay top-left in monitor-local coordinates for each corner.
    let (overlay_left, overlay_top) = match anchor {
        Anchor::BotLeft => (lx + offset_x, ty + win.height - overlay_height - offset_y),
        Anchor::BotRight => (
            lx + win.width - overlay_width - offset_x,
            ty + win.height - overlay_height - offset_y,
        ),
        Anchor::TopLeft => (lx + offset_x, ty + offset_y),
        Anchor::TopRight => (lx + win.width - overlay_width - offset_x, ty + offset_y),
    };

    // Convert to margins relative to the anchored edges.
    let margin_x = match anchor {
        Anchor::BotLeft | Anchor::TopLeft => overlay_left,
        Anchor::BotRight | Anchor::TopRight => monitor.width - (overlay_left + overlay_width),
    };
    let margin_y = match anchor {
        Anchor::TopLeft | Anchor::TopRight => overlay_top,
        Anchor::BotLeft | Anchor::BotRight => monitor.height - (overlay_top + overlay_height),
    };

    let margin_x = margin_x.clamp(0, monitor.width.max(0));
    let margin_y = margin_y.clamp(0, monitor.height.max(0));
    (anchor, margin_x, margin_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- in_edge_zone ----

    #[test]
    fn test_in_edge_zone_left_hit() {
        let win = Rect::new(100, 100, 800, 600);
        // 12.5% of 800 = 100; cursor 50px from left is within.
        assert!(in_edge_zone(win, (150, 400), "left", 0.125, 250));
    }

    #[test]
    fn test_in_edge_zone_left_miss() {
        let win = Rect::new(100, 100, 800, 600);
        // Center of window is outside the left zone (min_px 50 here).
        assert!(!in_edge_zone(win, (500, 400), "left", 0.125, 50));
    }

    #[test]
    fn test_in_edge_zone_uses_min_px_floor() {
        // Tiny window: fraction*width = 0.125*100 = 12 < 40 min_px, so min_px used.
        let win = Rect::new(0, 0, 100, 100);
        assert!(in_edge_zone(win, (30, 50), "left", 0.125, 40));
        // 45 is beyond the 40px floor.
        assert!(!in_edge_zone(win, (45, 50), "left", 0.125, 40));
    }

    #[test]
    fn test_in_edge_zone_right_top_bottom() {
        let win = Rect::new(0, 0, 800, 600);
        // right: within 100px of right edge (x=800)
        assert!(in_edge_zone(win, (750, 300), "right", 0.125, 100));
        assert!(!in_edge_zone(win, (600, 300), "right", 0.125, 100));
        // top: within 100px of top edge (y=0), 0.125*600=75 -> floor 100
        assert!(in_edge_zone(win, (400, 50), "top", 0.125, 100));
        assert!(!in_edge_zone(win, (400, 200), "top", 0.125, 100));
        // bottom: within 100px of bottom edge (y=600)
        assert!(in_edge_zone(win, (400, 550), "bottom", 0.125, 100));
        assert!(!in_edge_zone(win, (400, 400), "bottom", 0.125, 100));
    }

    #[test]
    fn test_in_edge_zone_zero_size_window() {
        let win = Rect::new(0, 0, 0, 0);
        assert!(!in_edge_zone(win, (0, 0), "left", 0.125, 250));
        let win2 = Rect::new(10, 10, 100, 0);
        assert!(!in_edge_zone(win2, (10, 10), "top", 0.125, 250));
    }

    // ---- overlay_width ----

    #[test]
    fn test_overlay_width_fixed_px() {
        let win = Rect::new(0, 0, 800, 600);
        assert_eq!(overlay_width(win, "250", "left", 0.125, 250, 8), 250);
    }

    #[test]
    fn test_overlay_width_change_area_x() {
        let win = Rect::new(0, 0, 800, 600);
        // zone = max(0.125*800=100, 250) = 250; minus 2*8 = 234.
        assert_eq!(overlay_width(win, "change_area_x", "left", 0.125, 250, 8), 234);
    }

    #[test]
    fn test_overlay_width_invalid_string() {
        let win = Rect::new(0, 0, 800, 600);
        assert_eq!(
            overlay_width(win, "not-a-number", "left", 0.125, 250, 8),
            DEFAULT_OVERLAY_WIDTH
        );
    }

    // ---- compute_anchor_margins ----

    #[test]
    fn test_compute_anchor_margins_bot_left() {
        let win = Rect::new(0, 0, 800, 600);
        let monitor = Rect::new(0, 0, 1920, 1080);
        let (anchor, mx, my) =
            compute_anchor_margins(win, monitor, "bot_left", 8, 26, 250, 36);
        assert_eq!(anchor, Anchor::BotLeft);
        assert_eq!(mx, 8);
        // top = 600 - 36 - 26 = 538; margin_bottom = 1080 - (538+36) = 506.
        assert_eq!(my, 506);
    }

    #[test]
    fn test_compute_anchor_margins_top_right() {
        let win = Rect::new(100, 50, 800, 600);
        let monitor = Rect::new(0, 0, 1920, 1080);
        let (anchor, mx, my) =
            compute_anchor_margins(win, monitor, "top_right", 8, 26, 250, 36);
        assert_eq!(anchor, Anchor::TopRight);
        // left = 100 + 800 - 250 - 8 = 642; margin_right = 1920 - (642+250) = 1028.
        assert_eq!(mx, 1028);
        // top = 50 + 26 = 76.
        assert_eq!(my, 76);
    }

    #[test]
    fn test_compute_anchor_margins_clamps_negative() {
        // Window pushed left of the monitor so the left margin would be negative.
        let win = Rect::new(-100, 0, 800, 600);
        let monitor = Rect::new(0, 0, 1920, 1080);
        let (_anchor, mx, _my) =
            compute_anchor_margins(win, monitor, "bot_left", 8, 26, 250, 36);
        // left = -100 + 8 = -92 -> clamped to 0.
        assert_eq!(mx, 0);
    }

    #[test]
    fn test_compute_anchor_margins_multi_monitor_offset() {
        // Window sits on a secondary monitor at global x=1920.
        let win = Rect::new(1920, 0, 800, 600);
        let monitor = Rect::new(1920, 0, 1920, 1080);
        let (anchor, mx, _my) =
            compute_anchor_margins(win, monitor, "bot_left", 8, 26, 250, 36);
        assert_eq!(anchor, Anchor::BotLeft);
        // Monitor-local left = (1920-1920) + 8 = 8, NOT the global 1928.
        assert_eq!(mx, 8);
    }
}
