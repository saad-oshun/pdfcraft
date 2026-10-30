use super::geometry::{HANDLES, grabs_points, handle_pos, is_markup, reshapable, reshaped, resizable};
use super::state::{Gesture, PageCx, SETTLE_SECS};
use super::{
    Annotation, Color32, CommentTool, CornerRadius, DocInfo, DocView, Edit, LineEnding, NewAnnotation, Pos2, QuickTool, Rect, SELECT_BLUE, Shape,
    Stroke, color32, vec2,
};

/// Paint comment selection, hover and gesture previews on a page.
pub(crate) fn paint_page(ui: &egui::Ui, painter: &egui::Painter, cx: &PageCx<'_>, view: &DocView) {
    let cv = &view.comments;
    let pointer = cx.ptr.pos;
    // A redaction mark shows its fill while the pointer is over it, as Acrobat's does, so the
    // marked area reads before anything is applied.
    if cv.gesture.is_none()
        && let Some(a) = pointer.and_then(|p| cx.hit(p))
        && a.subtype == "Redact"
    {
        let ([r, g, b], opacity) = cx.redact_mark;
        let fill = Color32::from_rgba_unmultiplied((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, (opacity.clamp(0.0, 1.0) * 255.0) as u8);
        for rect in cx.redact_rects(a) {
            painter.rect_filled(rect, CornerRadius::ZERO, fill);
        }
    }
    if cv.gesture.is_none()
        && let Some(a) = pointer.and_then(|p| cx.hit(p))
        && (cx.tool == QuickTool::Select || matches!(cx.tool, QuickTool::Fill(_)) && a.fill_sign)
        && cv.selected != Some((cx.page, a.index))
    {
        for r in cx.screen_rects(a) {
            painter.rect_stroke(r.expand(2.0), CornerRadius::same(2), Stroke::new(1.0, SELECT_BLUE.gamma_multiply(0.7)), egui::StrokeKind::Outside);
        }
    }
    if let Some((page, index)) = cv.selected
        && page == cx.page
        && let Some(a) = cx.get(index)
    {
        if is_markup(&a.subtype) && !a.quads.is_empty() {
            for r in cx.screen_rects(a) {
                painter.rect_stroke(r.expand(2.0), CornerRadius::ZERO, Stroke::new(1.0, SELECT_BLUE), egui::StrokeKind::Middle);
            }
            return paint_gesture(painter, cx, view);
        }
        if view.signature_drag.contains(page, index) && matches!(cv.gesture, Some(Gesture::Move { page: p, index: i, .. }) if (p, i) == (page, index))
        {
            return paint_gesture(painter, cx, view);
        }
        let shift = ui.input(|i| i.modifiers.shift);
        if cx.allowed && grabs_points(cx.tool) && reshapable(a) {
            return paint_points(painter, cx, view, a, pointer, shift);
        }
        let r = cx.adjusted_rect(a, cv.gesture.as_ref(), pointer, shift, view.pending_edit.as_ref()).expand(2.0);
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, SELECT_BLUE), egui::StrokeKind::Middle);
        if cx.allowed && resizable(a) {
            for h in HANDLES {
                let c = handle_pos(r, h);
                painter.circle(c, 4.5, Color32::WHITE, Stroke::new(1.0, SELECT_BLUE));
            }
        }
    }
    paint_gesture(painter, cx, view);
}

/// A selected line, connected lines, polygon or cloud: a handle on each end or corner and, while
/// one is dragged (or its edit is on its way), the new shape over the old.
/// The points a selected line, polygon or cloud is drawn with while it changes: an end or corner
/// being dragged, the whole comment being moved by its body, or the edit just released (until the
/// page is drawn again). `None` while it isn't changing.
pub(super) fn changing_points(
    gesture: Option<&Gesture>,
    pending: Option<&Edit>,
    (page, index): (usize, usize),
    points: &[[f64; 2]],
    to_user: impl Fn(Pos2) -> [f64; 2],
    pointer: Option<Pos2>,
    shift: bool,
) -> Option<Vec<[f64; 2]>> {
    let shifted = |dx: f64, dy: f64| points.iter().map(|q| [q[0] + dx, q[1] + dy]).collect();
    match (gesture, pending) {
        (Some(Gesture::Reshape { page: p, index: i, point, points: from }), _) if (*p, *i) == (page, index) => {
            pointer.map(|at| reshaped(from, *point, to_user(at), shift))
        }
        (Some(Gesture::Move { page: p, index: i, from }), _) if (*p, *i) == (page, index) => pointer.map(|at| {
            let (a, b) = (to_user(*from), to_user(at));
            shifted(b[0] - a[0], b[1] - a[1])
        }),
        (_, Some(Edit::ReshapeAnnotation { page: p, index: i, points: to })) if (*p, *i) == (page, index) => Some(to.clone()),
        (_, Some(Edit::MoveAnnotation { page: p, index: i, dx, dy })) if (*p, *i) == (page, index) => Some(shifted(*dx, *dy)),
        _ => None,
    }
}

fn paint_points(painter: &egui::Painter, cx: &PageCx<'_>, view: &DocView, a: &Annotation, pointer: Option<Pos2>, shift: bool) {
    let cv = &view.comments;
    let points: Vec<[f64; 2]> = a.points.iter().map(|q| [f64::from(q[0]), f64::from(q[1])]).collect();
    let changing = changing_points(cv.gesture.as_ref(), view.pending_edit.as_ref(), (cx.page, a.index), &points, |p| cx.to_user(p), pointer, shift);
    let changed = changing.is_some();
    let points = changing.unwrap_or(points);
    let screen: Vec<Pos2> = points.iter().map(|q| cx.to_screen(*q)).collect();
    if changed {
        let stroke = Stroke::new(1.5, SELECT_BLUE);
        painter.add(if a.subtype == "Polygon" {
            egui::Shape::closed_line(screen.clone(), stroke)
        } else {
            egui::Shape::line(screen.clone(), stroke)
        });
    }
    for c in screen {
        painter.circle(c, 4.5, Color32::WHITE, Stroke::new(1.0, SELECT_BLUE));
    }
    paint_gesture(painter, cx, view);
}

/// The eraser as the pointer sees it: a circle exactly as wide as what it rubs out.
fn paint_eraser_circle(painter: &egui::Painter, at: Pos2, radius: f32) {
    painter.circle(at, radius, Color32::from_white_alpha(70), Stroke::new(1.5, Color32::from_gray(60)));
    painter.circle_stroke(at, radius + 1.0, Stroke::new(1.0, Color32::from_white_alpha(200)));
}

fn paint_gesture(painter: &egui::Painter, cx: &PageCx<'_>, view: &DocView) {
    // Hovering with the eraser: its circle follows the pointer over the page.
    if cx.tool == QuickTool::Comment(CommentTool::Eraser)
        && cx.allowed
        && view.comments.gesture.is_none()
        && let Some(p) = cx.ptr.pos
        && cx.xf.rect.contains(p)
    {
        paint_eraser_circle(painter, p, eraser_radius_px(cx.prefs.style(CommentTool::Eraser).width) as f32);
    }
    let now = painter.ctx().input(|i| i.time);
    for s in view.comments.settling.iter().filter(|s| s.page == cx.page && now - s.since < SETTLE_SECS) {
        paint_draw(painter, cx, &s.tool, &s.points);
    }
    if let Some(Gesture::Draw { page, tool, points }) = &view.comments.gesture
        && *page == cx.page
    {
        paint_draw(painter, cx, tool, points);
    }
}

/// A drawing tool's shape from its points (a live drag, or a released one still settling).
fn paint_draw(painter: &egui::Painter, cx: &PageCx<'_>, tool: &CommentTool, points: &[[f64; 2]]) {
    {
        let style = cx.prefs.style(*tool);
        let zoom = cx.xf.zoom();
        let stroke = Stroke::new((style.width as f32 * zoom).max(1.0), color32(style.color));
        let pts: Vec<Pos2> = points.iter().map(|p| cx.to_screen(*p)).collect();
        match tool {
            CommentTool::Ink => {
                painter.add(egui::Shape::line(pts, stroke));
            }
            CommentTool::Eraser => {
                // The rubbed path, as wide as what it rubs out (see `erase`), and the round eraser
                // itself at the pointer end.
                let radius = eraser_radius_px(style.width) as f32;
                painter.add(egui::Shape::line(pts.clone(), Stroke::new(2.0 * radius, Color32::from_gray(128).gamma_multiply(0.2))));
                if let Some(at) = pts.last() {
                    paint_eraser_circle(painter, *at, radius);
                }
            }
            CommentTool::Line | CommentTool::Arrow if pts.len() == 2 => {
                painter.line_segment([pts[0], pts[1]], stroke);
                if *tool == CommentTool::Arrow {
                    let d = (pts[0] - pts[1]).normalized();
                    let s = (6.0 + 3.0 * style.width as f32) * zoom;
                    let rot = |v: egui::Vec2, a: f32| vec2(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos());
                    for a in [0.52f32, -0.52] {
                        painter.line_segment([pts[1], pts[1] + rot(d, a) * s], stroke);
                    }
                }
            }
            CommentTool::Rectangle if pts.len() == 2 => {
                painter.rect_stroke(Rect::from_two_pos(pts[0], pts[1]), CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            }
            CommentTool::Oval if pts.len() == 2 => {
                let r = Rect::from_two_pos(pts[0], pts[1]);
                painter.add(egui::Shape::ellipse_stroke(r.center(), r.size() / 2.0, stroke));
            }
            CommentTool::Polygon | CommentTool::PolyLine | CommentTool::Cloud => {
                // The vertices so far, and a rubber band to the pointer.
                let mut line = pts.clone();
                if let Some(p) = cx.ptr.pos {
                    line.push(p);
                }
                painter.add(egui::Shape::line(line, stroke));
                if let Some(first) = pts.first().filter(|_| *tool != CommentTool::PolyLine && pts.len() >= 3) {
                    painter.circle_stroke(*first, 5.0, Stroke::new(1.0, SELECT_BLUE));
                }
            }
            CommentTool::TextBox if pts.len() == 2 => {
                painter.rect_stroke(Rect::from_two_pos(pts[0], pts[1]), CornerRadius::ZERO, Stroke::new(1.0, SELECT_BLUE), egui::StrokeKind::Inside);
            }
            CommentTool::Callout if pts.len() == 2 => {
                painter.line_segment([pts[0], pts[1]], stroke);
                painter.rect_stroke(Rect::from_min_size(pts[1], vec2(160.0, 40.0) * zoom), CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            }
            _ => {}
        }
    }
}

/// The shape a finished drawing gesture makes (`None` if it is too small to mean anything).
pub(super) fn drawn_shape(tool: CommentTool, points: &[[f64; 2]]) -> Option<Shape> {
    let (first, last) = (*points.first()?, *points.last()?);
    let far = (first[0] - last[0]).hypot(first[1] - last[1]) >= 2.0;
    let rect = [first[0].min(last[0]), first[1].min(last[1]), first[0].max(last[0]), first[1].max(last[1])];
    let big = rect[2] - rect[0] >= 2.0 && rect[3] - rect[1] >= 2.0;
    match tool {
        CommentTool::Ink if !points.is_empty() => Some(Shape::Ink { strokes: vec![points.to_vec()] }),
        CommentTool::Line | CommentTool::Arrow if far => Some(Shape::Line {
            from: first,
            to: last,
            start: LineEnding::None,
            end: if tool == CommentTool::Arrow { LineEnding::OpenArrow } else { LineEnding::None },
        }),
        CommentTool::Rectangle if big => Some(Shape::Rectangle { rect }),
        CommentTool::Oval if big => Some(Shape::Oval { rect }),
        CommentTool::Polygon | CommentTool::Cloud if points.len() >= 3 => {
            Some(Shape::Polygon { vertices: points.to_vec(), cloud: tool == CommentTool::Cloud })
        }
        CommentTool::PolyLine if points.len() >= 2 => {
            Some(Shape::PolyLine { vertices: points.to_vec(), start: LineEnding::None, end: LineEnding::None })
        }
        _ => None,
    }
}

/// The eraser's radius on screen, in pixels, from its thickness setting (#3747): three pixels
/// per point, so the default 2 pt keeps the 12-pixel eraser it always had, and the thickness
/// slider's 0.5–12 pt gives 3 to 72 pixels across. A setting that isn't a number is the default.
fn eraser_radius_px(width: f64) -> f64 {
    3.0 * if width.is_finite() { width.clamp(0.5, 12.0) } else { 2.0 }
}

/// Eraser: rub out drawings along `path` (user space); one undo step for all of them. It rubs
/// as wide on screen as its preview, whatever the zoom. Freehand drawings lose only the part it
/// passes over; highlights, underlines, strike-outs and drawn lines and shapes it touches are
/// removed whole, like rubbing them off the page.
pub(super) fn erase(cx: &PageCx<'_>, path: &[[f64; 2]]) -> Option<Edit> {
    let zoom = f64::from(cx.xf.zoom());
    // A page with no size on screen has nothing to rub out (and no finite radius).
    if !(zoom.is_finite() && zoom > 0.0) || path.is_empty() {
        return None;
    }
    // Exactly the preview's width: no floor, which at a high zoom rubbed past it.
    let radius = eraser_radius_px(cx.prefs.style(CommentTool::Eraser).width) / zoom;
    let b = path.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |r, p| [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])]);
    let near = |r: [f64; 4]| r[0] <= b[2] + radius && r[2] >= b[0] - radius && r[1] <= b[3] + radius && r[3] >= b[1] - radius;
    let samples = eraser_samples(path, radius);
    let mut hits: Vec<(usize, bool)> = cx
        .comments()
        .filter(|a| !a.locked && !a.fill_sign)
        .filter(|a| near(a.rect.map(f64::from)))
        .filter_map(|a| {
            if a.subtype == "Ink" {
                return Some((a.index, true));
            }
            if !rubbed_whole(&a.subtype) {
                return None;
            }
            let quads: Vec<[f64; 4]> = if is_markup(&a.subtype) && !a.quads.is_empty() {
                a.quads.iter().map(quad_bounds).collect()
            } else {
                vec![a.rect.map(f64::from)]
            };
            let touched = quads.iter().any(|r| samples.iter().any(|p| point_near_rect(*p, *r, radius)));
            touched.then_some((a.index, false))
        })
        .collect();
    if hits.is_empty() {
        return None;
    }
    // Later indices first: deleting an emptied drawing shifts the ones after it.
    hits.sort_unstable_by_key(|h| std::cmp::Reverse(h.0));
    let edits =
        hits.into_iter()
            .map(|(index, ink)| {
                if ink {
                    Edit::EraseInk { page: cx.page, index, path: path.to_vec(), radius }
                } else {
                    Edit::DeleteAnnotation { page: cx.page, index }
                }
            })
            .collect();
    Some(Edit::Batch { label: "Erase".into(), edits })
}

/// Comments the eraser removes whole when it touches them: text markup and drawn lines and shapes.
fn rubbed_whole(subtype: &str) -> bool {
    matches!(subtype, "Highlight" | "Underline" | "StrikeOut" | "Squiggly" | "Line" | "PolyLine" | "Polygon" | "Square" | "Circle")
}

/// The bounding box `[x0, y0, x1, y1]` of a `/QuadPoints` quadrilateral.
fn quad_bounds(q: &[f32; 8]) -> [f64; 4] {
    let xs = [q[0], q[2], q[4], q[6]].map(f64::from);
    let ys = [q[1], q[3], q[5], q[7]].map(f64::from);
    let min = |v: [f64; 4]| v.iter().copied().fold(f64::MAX, f64::min);
    let max = |v: [f64; 4]| v.iter().copied().fold(f64::MIN, f64::max);
    [min(xs), min(ys), max(xs), max(ys)]
}

/// Whether `p` is within `radius` of the rectangle `r` (corners in any order).
fn point_near_rect(p: [f64; 2], r: [f64; 4], radius: f64) -> bool {
    let (x0, x1) = (r[0].min(r[2]), r[0].max(r[2]));
    let (y0, y1) = (r[1].min(r[3]), r[1].max(r[3]));
    let dx = (x0 - p[0]).max(0.0).max(p[0] - x1);
    let dy = (y0 - p[1]).max(0.0).max(p[1] - y1);
    dx * dx + dy * dy <= radius * radius
}

/// Points along `path` no further apart than `radius`, so a quick stroke over a thin underline
/// still touches it. Capped, so a hostile path can't allocate without bound.
fn eraser_samples(path: &[[f64; 2]], radius: f64) -> Vec<[f64; 2]> {
    const CAP: usize = 20_000;
    let step = if radius.is_finite() && radius > 0.0 { radius } else { 1.0 };
    let mut out = Vec::with_capacity(path.len().min(CAP));
    out.extend(path.first().copied());
    for w in path.windows(2) {
        let &[a, c] = w else { continue };
        let len = ((c[0] - a[0]).powi(2) + (c[1] - a[1]).powi(2)).sqrt();
        let n = if len.is_finite() { (len / step).ceil().clamp(1.0, 1000.0) as usize } else { 1 };
        for i in 1..=n {
            if out.len() >= CAP {
                return out;
            }
            let t = i as f64 / n as f64;
            out.push([a[0] + (c[0] - a[0]) * t, a[1] + (c[1] - a[1]) * t]);
        }
    }
    out
}

/// Where the composer for editing `a` opens: the top-right corner of the comment as drawn on the
/// displayed page.
pub(super) fn edit_anchor(info: &DocInfo, a: &Annotation) -> [f64; 2] {
    let Some(p) = info.pages.get(a.page) else { return [f64::from(a.rect[2]), f64::from(a.rect[3])] };
    let r = a.shown_rect(p.rotation);
    let (u, v) = (p.user_to_view(r[0], r[1]), p.user_to_view(r[2], r[3]));
    p.view_to_user(u[0].max(v[0]), u[1].min(v[1])).map(f64::from)
}

/// Whether comment `index` on the composer's page is at least partly inside `viewport`, as drawn.
pub(super) fn edited_in_view(cx: &PageCx<'_>, index: usize, viewport: Rect) -> bool {
    cx.info.annotations.iter().any(|a| a.page == cx.page && a.index == index && viewport.intersects(cx.screen_rect(a)))
}

pub(super) fn new_comment(cx: &PageCx<'_>, tool: CommentTool, shape: Shape, contents: String) -> Edit {
    Edit::AddAnnotation(NewAnnotation { page: cx.page, shape, style: cx.prefs.style(tool), contents, author: cx.prefs.author.clone() })
}

#[cfg(test)]
mod eraser_tests {
    use super::{eraser_samples, point_near_rect, quad_bounds, rubbed_whole};

    #[test]
    fn a_stroke_across_a_thin_underline_touches_it_between_its_points() {
        // Two points far either side of a 1 pt line: only the samples in between reach it.
        let path = [[0.0, 0.0], [0.0, 100.0]];
        let line = [-5.0, 49.5, 5.0, 50.5];
        assert!(!path.iter().any(|p| point_near_rect(*p, line, 2.0)));
        assert!(eraser_samples(&path, 2.0).iter().any(|p| point_near_rect(*p, line, 2.0)));
    }

    #[test]
    fn quads_and_rects_in_any_corner_order() {
        assert_eq!(quad_bounds(&[10.0, 20.0, 30.0, 20.0, 10.0, 5.0, 30.0, 5.0]), [10.0, 5.0, 30.0, 20.0]);
        assert!(point_near_rect([0.0, 0.0], [10.0, 10.0, -10.0, -10.0], 0.0));
        assert!(!point_near_rect([20.0, 0.0], [-10.0, -10.0, 10.0, 10.0], 5.0));
    }

    #[test]
    fn hostile_paths_stay_bounded() {
        assert!(eraser_samples(&[], 1.0).is_empty());
        assert!(eraser_samples(&[[0.0, 0.0], [f64::INFINITY, 0.0]], 1.0).len() <= 2);
        assert!(eraser_samples(&[[0.0, 0.0], [1e12, 0.0]], 0.0).len() <= 1001);
        let long: Vec<[f64; 2]> = (0..100_000).map(|i| [f64::from(i) * 1000.0, 0.0]).collect();
        assert!(eraser_samples(&long, 1.0).len() <= 20_000);
    }

    #[test]
    fn highlights_and_shapes_are_rubbed_whole_but_notes_are_not() {
        for s in ["Highlight", "Underline", "StrikeOut", "Squiggly", "Line", "Square", "Circle", "Polygon", "PolyLine"] {
            assert!(rubbed_whole(s), "{s}");
        }
        for s in ["Text", "FreeText", "Stamp", "Ink", "Link", "Widget"] {
            assert!(!rubbed_whole(s), "{s}");
        }
    }
}
