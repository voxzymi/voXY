//! The curve editor for `Value::Curve` parameters (Curves): a square graph,
//! input across, output up. Drag a point to move it, drag on empty space to
//! add one there, double-click a point to remove it. Each change is a new
//! `Curve` for the usual `SetProperty`, whose merge key makes one drag one
//! undo step.

use egui::{Color32, Pos2, Sense, Stroke, Ui, Vec2};
use voxy_core::Curve;
use voxy_core::glam::DVec2;

/// The smallest gap between neighbouring points' inputs.
const MIN_GAP: f64 = 1e-3;

/// How close (in points) the pointer must be to grab a curve point.
const GRAB: f32 = 8.0;

/// `points` with point `i` moved to `to`, kept inside the graph and
/// strictly between its neighbours' inputs.
pub(crate) fn move_point(points: &[DVec2], i: usize, to: DVec2) -> Vec<DVec2> {
    let mut out = points.to_vec();
    let lo = if i == 0 {
        0.0
    } else {
        points[i - 1].x + MIN_GAP
    };
    let hi = if i + 1 == points.len() {
        1.0
    } else {
        points[i + 1].x - MIN_GAP
    };
    let x = if lo <= hi {
        to.x.clamp(lo, hi)
    } else {
        points[i].x
    };
    out[i] = DVec2::new(x, to.y.clamp(0.0, 1.0));
    out
}

/// `points` with a new point at `at`, and its index; `None` when the curve
/// is full or `at` is too close to an existing point's input.
pub(crate) fn insert_point(points: &[DVec2], at: DVec2) -> Option<(Vec<DVec2>, usize)> {
    if points.len() >= Curve::MAX_POINTS {
        return None;
    }
    let at = at.clamp(DVec2::ZERO, DVec2::ONE);
    if points.iter().any(|p| (p.x - at.x).abs() < MIN_GAP) {
        return None;
    }
    let i = points.partition_point(|p| p.x < at.x);
    let mut out = points.to_vec();
    out.insert(i, at);
    Some((out, i))
}

/// `points` without point `i`; `None` when only two are left.
pub(crate) fn remove_point(points: &[DVec2], i: usize) -> Option<Vec<DVec2>> {
    if points.len() <= Curve::MIN_POINTS || i >= points.len() {
        return None;
    }
    let mut out = points.to_vec();
    out.remove(i);
    Some(out)
}

/// Draws the editor and returns the edited curve, if it changed.
pub(crate) fn show(ui: &mut Ui, id: egui::Id, curve: &Curve) -> Option<Curve> {
    let side = ui.available_width().clamp(120.0, 200.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click_and_drag());
    let to_screen = |p: DVec2| {
        Pos2::new(
            rect.left() + p.x as f32 * rect.width(),
            rect.bottom() - p.y as f32 * rect.height(),
        )
    };
    let to_curve = |p: Pos2| {
        DVec2::new(
            f64::from((p.x - rect.left()) / rect.width()),
            f64::from((rect.bottom() - p.y) / rect.height()),
        )
    };
    let points = curve.points();
    let nearest = |p: Pos2| {
        points
            .iter()
            .enumerate()
            .map(|(i, q)| (i, to_screen(*q).distance(p)))
            .filter(|(_, d)| *d <= GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    let drag_id = id.with("dragging");
    let mut edited: Option<Vec<DVec2>> = None;

    if response.double_clicked()
        && let Some(p) = response.interact_pointer_pos()
        && let Some(i) = nearest(p)
    {
        edited = remove_point(points, i);
    } else if response.drag_started()
        && let Some(p) = response.interact_pointer_pos()
    {
        let grabbed = nearest(p).or_else(|| {
            let (next, i) = insert_point(points, to_curve(p))?;
            edited = Some(next);
            Some(i)
        });
        ui.data_mut(|d| d.insert_temp(drag_id, grabbed));
    } else if response.dragged()
        && let Some(p) = response.interact_pointer_pos()
        && let Some(Some(i)) = ui.data(|d| d.get_temp::<Option<usize>>(drag_id))
        && i < points.len()
    {
        edited = Some(move_point(points, i, to_curve(p)));
    }
    if response.drag_stopped() {
        ui.data_mut(|d| d.remove::<Option<usize>>(drag_id));
    }

    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
    let grid = Stroke::new(1.0, visuals.faint_bg_color);
    for k in 1..4 {
        let t = k as f32 / 4.0;
        let x = rect.left() + t * rect.width();
        let y = rect.top() + t * rect.height();
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            grid,
        );
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            grid,
        );
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_top()],
        Stroke::new(1.0, visuals.weak_text_color().gamma_multiply(0.4)),
    );
    let shown = edited
        .as_ref()
        .and_then(|p| Curve::new(p.clone()).ok())
        .unwrap_or_else(|| curve.clone());
    let line: Vec<Pos2> = (0..=128)
        .map(|k| {
            let x = f64::from(k) / 128.0;
            to_screen(DVec2::new(x, shown.eval(x)))
        })
        .collect();
    let accent = visuals.selection.stroke.color;
    painter.add(egui::Shape::line(line, Stroke::new(1.5, accent)));
    let hover = response.hover_pos().and_then(nearest);
    for (i, p) in shown.points().iter().enumerate() {
        let fill = if hover == Some(i) {
            accent
        } else {
            Color32::WHITE
        };
        painter.circle(to_screen(*p), 4.0, fill, Stroke::new(1.0, Color32::BLACK));
    }
    response.on_hover_text(
        "Drag a point to move it. Drag elsewhere to add one. Double-click a point to remove it.",
    );

    let next = Curve::new(edited?).ok()?;
    (next != *curve).then_some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(p: &[(f64, f64)]) -> Vec<DVec2> {
        p.iter().map(|(x, y)| DVec2::new(*x, *y)).collect()
    }

    #[test]
    fn moving_keeps_points_in_order_and_inside() {
        let p = pts(&[(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)]);
        // Past a neighbour: stops just short of it.
        let m = move_point(&p, 1, DVec2::new(1.5, -2.0));
        assert_eq!(m[1], DVec2::new(1.0 - MIN_GAP, 0.0));
        assert!(Curve::new(m).is_ok());
        let m = move_point(&p, 0, DVec2::new(0.9, 0.3));
        assert_eq!(m[0], DVec2::new(0.5 - MIN_GAP, 0.3));
        let m = move_point(&p, 2, DVec2::new(0.2, 2.0));
        assert_eq!(m[2], DVec2::new(0.5 + MIN_GAP, 1.0));
    }

    #[test]
    fn inserting_and_removing_respect_the_limits() {
        let p = pts(&[(0.0, 0.0), (1.0, 1.0)]);
        let (q, i) = insert_point(&p, DVec2::new(0.25, 0.6)).unwrap();
        assert_eq!(i, 1);
        assert_eq!(q[1], DVec2::new(0.25, 0.6));
        assert!(Curve::new(q.clone()).is_ok());
        // Not on top of an existing input.
        assert!(insert_point(&q, DVec2::new(0.25, 0.1)).is_none());
        // Two points are the minimum.
        assert!(remove_point(&p, 0).is_none());
        assert_eq!(remove_point(&q, 1).unwrap(), p);
        // A full curve takes no more.
        let full: Vec<DVec2> = (0..Curve::MAX_POINTS)
            .map(|k| DVec2::new(k as f64 / (Curve::MAX_POINTS - 1) as f64, 0.5))
            .collect();
        assert!(insert_point(&full, DVec2::new(0.0105, 0.5)).is_none());
    }
}
