//! Screen-space geometry for the clipboard-history flyout.
pub(super) const WIDTH_DIP: f32 = 360.0;
pub(super) const HEIGHT_DIP: f32 = 520.0;

#[derive(Clone, Copy)]
pub(super) struct WorkArea {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug)]
pub(super) struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub scale: f32,
}

pub(super) enum Origin {
    Caret { x: i32, y: i32, below: i32 },
    Tray,
    Window { x: i32, y: i32 },
}

pub(super) fn place(work: WorkArea, requested_scale: f32, origin: Origin) -> Placement {
    let available_w = (work.right - work.left).max(1);
    let available_h = (work.bottom - work.top).max(1);
    // Keep layout, paint and hit testing in the same coordinate space, even
    // when the monitor cannot fit the flyout at its requested DPI.
    let scale = requested_scale.min(available_w as f32 / WIDTH_DIP).min(available_h as f32 / HEIGHT_DIP);
    let w = (WIDTH_DIP * scale).round() as i32;
    let h = (HEIGHT_DIP * scale).round() as i32;
    let margin = (8.0 * scale).round() as i32;
    let (x, y) = match origin {
        Origin::Tray => (work.right - w - margin, work.bottom - h - margin),
        Origin::Caret { x, y, below } => {
            let next = y + below + margin / 2;
            let top = if next + h > work.bottom { y - h - margin / 2 } else { next };
            (x, top)
        }
        Origin::Window { x, y } => (x, y),
    };
    // Prefer the normal inset but never make it impossible to fit the flyout.
    let margin_x = margin.min((available_w - w) / 2);
    let margin_y = if matches!(origin, Origin::Tray) { margin.min((available_h - h) / 2) } else { 0 };
    let x = x.clamp(work.left + margin_x, work.left + available_w - w - margin_x);
    let y = y.clamp(work.top + margin_y, work.top + available_h - h - margin_y);
    Placement { x, y, w, h, scale }
}

pub(super) fn close_rect(width: i32, scale: f32) -> (i32, i32, i32, i32) {
    let px = |dip: f32| (dip * scale).round() as i32;
    (width - px(40.0), px(4.0), px(32.0), px(28.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_inside(p: Placement, work: WorkArea) {
        assert!(p.x >= work.left && p.y >= work.top, "{p:?}");
        assert!(p.x + p.w <= work.right && p.y + p.h <= work.bottom, "{p:?}");
        assert_eq!(p.w, (WIDTH_DIP * p.scale).round() as i32);
        assert_eq!(p.h, (HEIGHT_DIP * p.scale).round() as i32);
        let (x, y, w, h) = close_rect(p.w, p.scale);
        assert!(x >= 0 && y >= 0 && w > 0 && h > 0 && x + w <= p.w && y + h <= p.h);
    }

    #[test]
    fn high_dpi_popup_and_close_button_fit_on_narrow_monitor() {
        let work = WorkArea { left: -500, top: 0, right: 0, bottom: 1500 };
        assert_inside(place(work, 2.0, Origin::Caret { x: -200, y: 200, below: 14 }), work);
        assert_inside(place(work, 2.0, Origin::Tray), work);
    }

    #[test]
    fn tray_panel_does_not_overhang_top_of_short_work_area() {
        let work = WorkArea { left: 0, top: 32, right: 1600, bottom: 600 };
        assert_inside(place(work, 1.5, Origin::Tray), work);
    }

    #[test]
    fn dragging_or_a_dpi_change_cannot_leave_the_panel_off_screen() {
        let work = WorkArea { left: -1280, top: -900, right: 0, bottom: 0 };
        assert_inside(place(work, 1.25, Origin::Window { x: -70, y: -12 }), work);
        assert_inside(place(work, 1.25, Origin::Window { x: -1600, y: -950 }), work);
    }

    #[test]
    fn caret_keeps_popup_above_when_there_is_no_room_below() {
        let work = WorkArea { left: 0, top: 0, right: 1920, bottom: 1040 };
        let p = place(work, 1.0, Origin::Caret { x: 100, y: 1010, below: 20 });
        assert_eq!((p.x, p.y, p.w, p.h), (100, 486, 360, 520));
    }

    #[test]
    fn normal_display_keeps_requested_dpi_and_anchor() {
        let p = place(WorkArea { left: 0, top: 0, right: 1920, bottom: 1040 }, 1.25, Origin::Window { x: 400, y: 100 });
        assert_eq!((p.x, p.y, p.w, p.h, p.scale), (400, 100, 450, 650, 1.25));
    }
}
