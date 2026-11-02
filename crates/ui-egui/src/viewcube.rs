//! The navigation overlays on the drawing area: the ViewCube (Top face, Home, compass ring and
//! the WCS menu) and the UCS icon.
//!
//! Every overlay registers a widget that senses clicks *and* drags, so egui hands the pointer to
//! the overlay instead of the canvas underneath (a click on the ViewCube must never also pick or
//! start a selection window in the drawing). What the overlays do goes through commands:
//! `zoom.extents` (Top face, Home), `ui.toggle.viewcube`, `ui.toggle.ucsicon`.
//!
//! CADCraft's model view is a 2D plan view of the World Coordinate System: there is no view
//! twist, no 3D view direction and no user coordinate system yet, so the compass ring explains
//! that instead of rotating, and the WCS menu lists the only coordinate system there is.

use egui::{Color32, CursorIcon, Pos2, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::CadApp;
use crate::theme::Tokens;

/// Shown when the compass ring is clicked or dragged.
pub const NO_TWIST: &str =
    "ViewCube: rotating the view needs a view twist, which the 2D plan view doesn't support yet. Use 'WCS ⌵' below to select Coordinate System.";

/// Where the ViewCube's parts are on the canvas.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub center: Pos2,
    /// Radius of the compass ring's centre line.
    pub ring: f32,
    pub face: Rect,
    pub home: Rect,
    /// The "WCS" coordinate-system menu under the cube.
    pub pill: Rect,
    /// Everything that belongs to the cube (the area kept away from the canvas).
    pub bounds: Rect,
    pub rot_ccw: Rect,
    pub rot_cw: Rect,
    pub arrow_top: Rect,
    pub arrow_bottom: Rect,
    pub arrow_left: Rect,
    pub arrow_right: Rect,
}

impl Layout {
    pub fn new(canvas: Rect) -> Self {
        let center = pos2(canvas.right() - 92.0, canvas.top() + 82.0);
        let ring = 58.0;
        let rot_ccw = Rect::from_center_size(center + vec2(36.0, -56.0), vec2(20.0, 20.0));
        let rot_cw = Rect::from_center_size(center + vec2(56.0, -36.0), vec2(20.0, 20.0));
        let arrow_top = Rect::from_center_size(center + vec2(0.0, -28.0), vec2(14.0, 10.0));
        let arrow_bottom = Rect::from_center_size(center + vec2(0.0, 28.0), vec2(14.0, 10.0));
        let arrow_left = Rect::from_center_size(center + vec2(-28.0, 0.0), vec2(10.0, 14.0));
        let arrow_right = Rect::from_center_size(center + vec2(28.0, 0.0), vec2(10.0, 14.0));
        Layout {
            center,
            ring,
            face: Rect::from_center_size(center, vec2(44.0, 44.0)),
            home: Rect::from_center_size(center + vec2(-ring + 2.0, -ring + 2.0), vec2(20.0, 20.0)),
            pill: Rect::from_center_size(center + vec2(0.0, ring + 26.0), vec2(56.0, 16.0)),
            bounds: Rect::from_center_size(center, vec2(2.0 * (ring + 18.0), 2.0 * (ring + 18.0))),
            rot_ccw,
            rot_cw,
            arrow_top,
            arrow_bottom,
            arrow_left,
            arrow_right,
        }
    }

    /// The part of the cube at `p` (not the pill, which is its own widget).
    pub fn part_at(&self, p: Pos2) -> Option<Part> {
        if self.home.contains(p) {
            return Some(Part::Home);
        }
        if self.rot_ccw.contains(p) {
            return Some(Part::RotCcw);
        }
        if self.rot_cw.contains(p) {
            return Some(Part::RotCw);
        }
        if self.arrow_top.contains(p) {
            return Some(Part::ArrowTop);
        }
        if self.arrow_bottom.contains(p) {
            return Some(Part::ArrowBottom);
        }
        if self.arrow_left.contains(p) {
            return Some(Part::ArrowLeft);
        }
        if self.arrow_right.contains(p) {
            return Some(Part::ArrowRight);
        }
        if self.face.contains(p) {
            return Some(Part::Face);
        }
        let d = p.distance(self.center);
        ((self.ring - 10.0..=self.ring + 10.0).contains(&d)).then_some(Part::Ring)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Face,
    Home,
    Ring,
    RotCcw,
    RotCw,
    ArrowTop,
    ArrowBottom,
    ArrowLeft,
    ArrowRight,
}

impl Part {
    fn tip(self) -> &'static str {
        match self {
            Part::Face => "Top: plan view of the drawing (zoom extents)",
            Part::Home => "Home: zoom to the drawing extents (North Up)",
            Part::Ring => "Compass: Click rotation arrows (top-right) or ring directions to orient view",
            Part::RotCcw => "Rotate 90° Counter-Clockwise (Left)",
            Part::RotCw => "Rotate 90° Clockwise (Right)",
            Part::ArrowTop => "Back View (or Top View)",
            Part::ArrowBottom => "Front View",
            Part::ArrowLeft => "Left View",
            Part::ArrowRight => "Right View",
        }
    }
}

/// Paint the ViewCube's ring, compass letters, Top face and Home button, and handle the pointer.
/// The canvas paints the WCS pill itself; this returns the pointer's part for its highlight.
pub fn show(app: &mut CadApp, ui: &mut egui::Ui, l: &Layout) {
    let resp = ui.interact(l.bounds, ui.id().with("viewcube"), Sense::click_and_drag());
    let part = resp.hover_pos().and_then(|p| l.part_at(p));
    let p = ui.painter_at(l.bounds.union(l.pill));
    let ring_col = if part == Some(Part::Ring) { Color32::from_rgb(0x5c, 0x67, 0x7c) } else { Color32::from_rgb(0x48, 0x50, 0x5c) };
    p.circle_stroke(l.center, l.ring, Stroke::new(9.0, ring_col));
    p.circle_stroke(l.center, l.ring + 4.5, Stroke::new(1.0, Color32::from_rgb(0x5c, 0x65, 0x72)));
    let f = egui::FontId::proportional(17.0);
    let lc = Color32::from_rgb(0xc8, 0xcc, 0xd2);
    let c = l.center;
    let ring = l.ring;

    // Cardinal directions rotated with view twist
    let (n_txt, e_txt, s_txt, w_txt) = match app.ui.view_twist % 4 {
        1 => ("W", "N", "E", "S"),
        2 => ("S", "W", "N", "E"),
        3 => ("E", "S", "W", "N"),
        _ => ("N", "E", "S", "W"),
    };
    p.text(c + vec2(0.0, -ring - 1.0), egui::Align2::CENTER_CENTER, n_txt, f.clone(), lc);
    p.text(c + vec2(0.0, ring + 1.0), egui::Align2::CENTER_CENTER, s_txt, f.clone(), lc);
    p.text(c + vec2(ring + 1.0, 0.0), egui::Align2::CENTER_CENTER, e_txt, f.clone(), lc);
    p.text(c + vec2(-ring - 1.0, 0.0), egui::Align2::CENTER_CENTER, w_txt, f, lc);

    let face_col = if part == Some(Part::Face) { Color32::from_rgb(0xb8, 0xbc, 0xc2) } else { Color32::from_rgb(0x9a, 0x9e, 0xa4) };
    p.rect_filled(l.face, 2.0, face_col);
    p.rect_stroke(l.face, 2.0, Stroke::new(1.0, Color32::from_rgb(0x6c, 0x70, 0x76)), egui::StrokeKind::Inside);
    let face_txt = if app.ui.view_name.is_empty() { "TOP".to_string() } else { app.ui.view_name.to_uppercase() };
    p.text(c, egui::Align2::CENTER_CENTER, face_txt, egui::FontId::proportional(12.0), Color32::from_rgb(0x50, 0x54, 0x5a));
    paint_home(&p, l.home, part == Some(Part::Home));

    // Curved rotation arrows beside the cube
    paint_rot_arrow(&p, l.rot_ccw, true, part == Some(Part::RotCcw));
    paint_rot_arrow(&p, l.rot_cw, false, part == Some(Part::RotCw));

    // Directional triangles around the cube
    paint_triangle(&p, l.arrow_top, 0, part == Some(Part::ArrowTop));
    paint_triangle(&p, l.arrow_bottom, 1, part == Some(Part::ArrowBottom));
    paint_triangle(&p, l.arrow_left, 2, part == Some(Part::ArrowLeft));
    paint_triangle(&p, l.arrow_right, 3, part == Some(Part::ArrowRight));

    let resp = match part {
        Some(part) => resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(part.tip()),
        None => resp,
    };
    let pressed_on = resp.interact_pointer_pos().and_then(|o| l.part_at(o));
    if resp.clicked() {
        match pressed_on {
            Some(Part::Face | Part::Home) => {
                app.ui.view_name = "Top".to_string();
                let _ = app.run("zoom.extents", json!({}));
            }
            Some(Part::RotCcw) => {
                let _ = app.run("view.twist.ccw", json!({}));
            }
            Some(Part::RotCw) => {
                let _ = app.run("view.twist.cw", json!({}));
            }
            Some(Part::ArrowTop) => {
                if app.ui.view_name != "Top" {
                    let _ = app.run("view.top", json!({}));
                } else {
                    let _ = app.run("view.back", json!({}));
                }
            }
            Some(Part::ArrowBottom) => {
                let _ = app.run("view.front", json!({}));
            }
            Some(Part::ArrowLeft) => {
                let _ = app.run("view.left", json!({}));
            }
            Some(Part::ArrowRight) => {
                let _ = app.run("view.right", json!({}));
            }
            Some(Part::Ring) => {
                app.session.echo(NO_TWIST);
            }
            None => {}
        }
    }
    if resp.drag_started() && pressed_on == Some(Part::Ring) {
        app.session.echo(NO_TWIST);
    }
    let mut action = None;
    resp.context_menu(|ui| {
        if ui.button("Plan View (North Up)").clicked() {
            action = Some(("view.orient.north", json!({})));
            ui.close();
        }
        if ui.button("Rotate 90° Clockwise").clicked() {
            action = Some(("view.twist.cw", json!({})));
            ui.close();
        }
        if ui.button("Rotate 90° Counter-Clockwise").clicked() {
            action = Some(("view.twist.ccw", json!({})));
            ui.close();
        }
        ui.separator();
        if ui.button("Front View").clicked() {
            action = Some(("view.front", json!({})));
            ui.close();
        }
        if ui.button("Left View").clicked() {
            action = Some(("view.left", json!({})));
            ui.close();
        }
        ui.separator();
        if ui.button("Home (Zoom Extents)").clicked() {
            action = Some(("zoom.extents", json!({})));
            ui.close();
        }
        ui.separator();
        if ui.button("Hide ViewCube").clicked() {
            action = Some(("ui.toggle.viewcube", json!({"on": false})));
            ui.close();
        }
    });
    if let Some((id, params)) = action {
        let _ = app.run(id, params);
    }
    // The WCS pill (painted by the canvas) opens the coordinate-system menu.
    let pill = ui.interact(l.pill, ui.id().with("viewcube_wcs"), Sense::click_and_drag()).on_hover_cursor(CursorIcon::PointingHand);
    let pill_tip = format!("Coordinate System: {} (Click to select CRS / Map)", app.map_tiles.current_crs.display_name());
    let pill = pill.on_hover_text(pill_tip);
    ucs_menu(app, egui::Popup::menu(&pill));
}

/// A small house, drawn in code.
fn paint_home(p: &egui::Painter, r: Rect, hot: bool) {
    let t = Tokens::get();
    if hot {
        p.rect_filled(r, 3.0, t.control_hover);
    }
    let st = Stroke::new(1.3, if hot { t.text } else { t.text_dim });
    let c = r.center();
    let roof = [c + vec2(-6.5, 0.5), c + vec2(0.0, -6.0), c + vec2(6.5, 0.5)];
    p.line(roof.to_vec(), st);
    p.line(vec![c + vec2(-4.5, -1.0), c + vec2(-4.5, 6.0), c + vec2(4.5, 6.0), c + vec2(4.5, -1.0)], st);
    p.line_segment([c + vec2(-1.2, 6.0), c + vec2(-1.2, 2.5)], st);
    p.line_segment([c + vec2(1.2, 6.0), c + vec2(1.2, 2.5)], st);
    p.line_segment([c + vec2(-1.2, 2.5), c + vec2(1.2, 2.5)], st);
}

/// Curved rotation arrow (counter-clockwise or clockwise).
fn paint_rot_arrow(p: &egui::Painter, r: Rect, ccw: bool, hot: bool) {
    if hot {
        p.rect_filled(r, 4.0, Color32::from_rgba_premultiplied(0x5c, 0x67, 0x7c, 80));
    }
    let st = Stroke::new(1.6, if hot { Color32::from_rgb(0xec, 0xf0, 0xf8) } else { Color32::from_rgb(0x8a, 0x94, 0xa6) });
    let c = r.center();
    if ccw {
        // Curve up and left (counter-clockwise)
        let arc = vec![pos2(c.x + 5.0, c.y + 4.0), pos2(c.x + 4.0, c.y - 1.0), pos2(c.x, c.y - 4.0), pos2(c.x - 5.0, c.y - 3.0)];
        p.line(arc, st);
        // Arrow head pointing left
        p.line_segment([pos2(c.x - 5.0, c.y - 3.0), pos2(c.x - 2.0, c.y - 6.5)], st);
        p.line_segment([pos2(c.x - 5.0, c.y - 3.0), pos2(c.x - 2.0, c.y + 0.5)], st);
    } else {
        // Curve right and down (clockwise)
        let arc = vec![pos2(c.x - 4.0, c.y - 5.0), pos2(c.x + 1.0, c.y - 4.0), pos2(c.x + 4.0, c.y), pos2(c.x + 3.0, c.y + 5.0)];
        p.line(arc, st);
        // Arrow head pointing down
        p.line_segment([pos2(c.x + 3.0, c.y + 5.0), pos2(c.x - 0.5, c.y + 2.0)], st);
        p.line_segment([pos2(c.x + 3.0, c.y + 5.0), pos2(c.x + 6.5, c.y + 2.0)], st);
    }
}

/// Small direction triangle (0=Up, 1=Down, 2=Left, 3=Right).
fn paint_triangle(p: &egui::Painter, r: Rect, dir: u8, hot: bool) {
    let c = r.center();
    let col = if hot { Color32::from_rgb(0xec, 0xf0, 0xf8) } else { Color32::from_rgb(0x8a, 0x94, 0xa6) };
    let pts = match dir {
        0 => vec![c + vec2(0.0, -4.5), c + vec2(-4.0, 3.5), c + vec2(4.0, 3.5)],  // Up
        1 => vec![c + vec2(0.0, 4.5), c + vec2(-4.0, -3.5), c + vec2(4.0, -3.5)], // Down
        2 => vec![c + vec2(-4.5, 0.0), c + vec2(3.5, -4.0), c + vec2(3.5, 4.0)],  // Left
        _ => vec![c + vec2(4.5, 0.0), c + vec2(-3.5, -4.0), c + vec2(-3.5, 4.0)], // Right
    };
    p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
}

/// The UCS icon's arms (as drawn by the canvas at the lower-left corner): the X arm and the Y arm.
pub fn ucs_arms(canvas: Rect) -> [Rect; 2] {
    let o = pos2(canvas.left() + 36.0, canvas.bottom() - 34.0);
    [Rect::from_min_max(o + vec2(-8.0, -8.0), o + vec2(80.0, 8.0)), Rect::from_min_max(o + vec2(-8.0, -82.0), o + vec2(8.0, 8.0))]
}

/// Make the UCS icon clickable: hovering highlights it, a click (either button) opens the
/// coordinate-system menu.
pub fn ucs_icon(app: &mut CadApp, ui: &mut egui::Ui, canvas: Rect) {
    let t = Tokens::get();
    let [x, y] = ucs_arms(canvas);
    let rx = ui.interact(x, ui.id().with("ucs_x"), Sense::click_and_drag());
    let ry = ui.interact(y, ui.id().with("ucs_y"), Sense::click_and_drag());
    let resp = (rx | ry).on_hover_cursor(CursorIcon::PointingHand).on_hover_text("UCS icon: World Coordinate System");
    if resp.hovered() {
        let o = pos2(x.left() + 8.0, x.center().y);
        let st = Stroke::new(2.0, t.hover);
        let p = ui.painter_at(canvas);
        p.line_segment([o, o + vec2(60.0, 0.0)], st);
        p.line_segment([o, o + vec2(0.0, -60.0)], st);
    }
    // Opened with `Bool(true)`: a pointer-anchored popup records its position only then.
    let open = (resp.clicked() || resp.secondary_clicked()).then_some(egui::SetOpenCommand::Bool(true));
    ucs_menu(app, egui::Popup::menu(&resp).open_memory(open).at_pointer_fixed());
}

/// The coordinate-system menu: Coordinate Reference Systems (UTM, WGS84, Mercator), background maps, and UCS icon switches.
fn ucs_menu(app: &mut CadApp, popup: egui::Popup<'_>) {
    let mut hide_ucs = false;
    let mut hide_cube = false;
    let mut run_cmd: Option<(&'static str, serde_json::Value)> = None;

    let cur_crs = app.map_tiles.current_crs;
    let manual_crs = app.map_tiles.manual_crs;

    popup.show(|ui| {
        ui.strong("Coordinate Reference System (CRS)");
        ui.label(format!("Active: {}", cur_crs.display_name()));
        ui.separator();

        // Auto-detect option
        let is_auto = manual_crs.is_none();
        let auto_label = if is_auto { format!("* Auto-Detect ({})", cur_crs.short_name()) } else { "Auto-Detect CRS".to_string() };
        if ui
            .selectable_label(is_auto, auto_label)
            .on_hover_text("Automatically detect CRS from drawing metadata, keywords, or coordinates range")
            .clicked()
        {
            run_cmd = Some(("crs.auto", json!({})));
            ui.close();
        }

        ui.separator();
        ui.label("Indian Survey / Railway Grid:");

        // UTM 43N
        let is_43 = matches!(manual_crs, Some(crate::map_tiles::CoordSystem::Utm { zone: 43, north: true }));
        let l43 =
            if is_43 { "* UTM Zone 43N (Delhi, Ambala, Haryana, Punjab, Mumbai)" } else { "  UTM Zone 43N (Delhi, Ambala, Haryana, Punjab, Mumbai)" };
        if ui
            .selectable_label(is_43, l43)
            .on_hover_text("UTM Zone 43 North: Delhi, Ambala, Haryana, Punjab, Rajasthan, Gujarat, Maharashtra")
            .clicked()
        {
            run_cmd = Some(("crs.utm43n", json!({})));
            ui.close();
        }

        // UTM 44N
        let is_44 = matches!(manual_crs, Some(crate::map_tiles::CoordSystem::Utm { zone: 44, north: true }));
        let l44 = if is_44 {
            "* UTM Zone 44N (Central/East India, UP, MP, AP, Hyderabad)"
        } else {
            "  UTM Zone 44N (Central/East India, UP, MP, AP, Hyderabad)"
        };
        if ui
            .selectable_label(is_44, l44)
            .on_hover_text("UTM Zone 44 North: Uttar Pradesh, Madhya Pradesh, Andhra Pradesh, Telangana, Hyderabad")
            .clicked()
        {
            run_cmd = Some(("crs.utm44n", json!({})));
            ui.close();
        }

        // UTM 45N
        let is_45 = matches!(manual_crs, Some(crate::map_tiles::CoordSystem::Utm { zone: 45, north: true }));
        let l45 = if is_45 {
            "* UTM Zone 45N (Kolkata, Bengal, Bihar, Assam, East India)"
        } else {
            "  UTM Zone 45N (Kolkata, Bengal, Bihar, Assam, East India)"
        };
        if ui.selectable_label(is_45, l45).on_hover_text("UTM Zone 45 North: West Bengal, Bihar, Jharkhand, Odisha, Assam, Northeast India").clicked()
        {
            run_cmd = Some(("crs.utm45n", json!({})));
            ui.close();
        }

        // UTM 42N
        let is_42 = matches!(manual_crs, Some(crate::map_tiles::CoordSystem::Utm { zone: 42, north: true }));
        let l42 = if is_42 { "* UTM Zone 42N (West Gujarat, Kutch, West Rajasthan)" } else { "  UTM Zone 42N (West Gujarat, Kutch, West Rajasthan)" };
        if ui.selectable_label(is_42, l42).on_hover_text("UTM Zone 42 North: Kutch, West Gujarat, West Rajasthan, Pakistan border").clicked() {
            run_cmd = Some(("crs.utm42n", json!({})));
            ui.close();
        }

        ui.separator();
        ui.label("Global Coordinate Systems:");

        // WGS 84
        let is_wgs = matches!(manual_crs, Some(crate::map_tiles::CoordSystem::Wgs84));
        let lwgs = if is_wgs { "* WGS 84 (GPS Degrees - EPSG:4326)" } else { "  WGS 84 (GPS Degrees - EPSG:4326)" };
        if ui.selectable_label(is_wgs, lwgs).on_hover_text("WGS 84: Standard GPS Latitude and Longitude degrees").clicked() {
            run_cmd = Some(("crs.wgs84", json!({})));
            ui.close();
        }

        // Web Mercator
        let is_merc = matches!(manual_crs, Some(crate::map_tiles::CoordSystem::WebMercator));
        let lmerc = if is_merc { "* Web Mercator (Meters - EPSG:3857)" } else { "  Web Mercator (Meters - EPSG:3857)" };
        if ui.selectable_label(is_merc, lmerc).on_hover_text("Web Mercator: Standard EPSG:3857 projected meters").clicked() {
            run_cmd = Some(("crs.mercator", json!({})));
            ui.close();
        }

        ui.separator();
        ui.strong("Background Map Imagery");

        let is_street = app.map_tiles.show_map && app.map_tiles.mode == crate::map_tiles::MapMode::Street;
        if ui.selectable_label(is_street, "Street Map (OpenStreetMap)").clicked() {
            run_cmd = Some(("geomap.street", json!({})));
            ui.close();
        }

        let is_sat = app.map_tiles.show_map && app.map_tiles.mode == crate::map_tiles::MapMode::Satellite;
        if ui.selectable_label(is_sat, "Satellite Imagery (Aerial View)").clicked() {
            run_cmd = Some(("geomap.satellite", json!({})));
            ui.close();
        }

        let is_hyb = app.map_tiles.show_map && app.map_tiles.mode == crate::map_tiles::MapMode::Hybrid;
        if ui.selectable_label(is_hyb, "Hybrid (Satellite + Street Labels)").clicked() {
            run_cmd = Some(("geomap.hybrid", json!({})));
            ui.close();
        }

        if app.map_tiles.show_map && ui.button("Turn Map OFF").clicked() {
            run_cmd = Some(("geomap.off", json!({})));
            ui.close();
        }

        ui.separator();
        ui.radio(true, "World (WCS) Default").on_hover_text("MonarchCAD draws in the World Coordinate System (WCS).");

        if ui.button("Hide UCS Icon").clicked() {
            hide_ucs = true;
            ui.close();
        }
        if ui.button("Hide ViewCube").clicked() {
            hide_cube = true;
            ui.close();
        }
    });

    if let Some((cmd, args)) = run_cmd {
        let _ = app.run(cmd, args);
    }
    if hide_ucs {
        let _ = app.run("ui.toggle.ucsicon", json!({"on": false}));
    }
    if hide_cube {
        let _ = app.run("ui.toggle.viewcube", json!({"on": false}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadcraft_engine::Session;

    /// The app with the sample drawing, laid out once in a 1400×900 window.
    fn app_and_ctx() -> (CadApp, egui::Context) {
        let mut app = CadApp::new(Session::empty(), crate::Services::default());
        app.ui.in_window_menu = true;
        app.start("ui.sample");
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, Vec::new());
        frame(&mut app, &ctx, Vec::new());
        (app, ctx)
    }

    fn frame(app: &mut CadApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }

    /// A real pointer click (move, press, release in separate frames, like the control channel).
    pub(crate) fn click(app: &mut CadApp, ctx: &egui::Context, at: Pos2, button: egui::PointerButton) {
        let m = egui::Modifiers::default();
        frame(app, ctx, vec![egui::Event::PointerMoved(at)]);
        frame(app, ctx, vec![egui::Event::PointerButton { pos: at, button, pressed: true, modifiers: m }]);
        frame(app, ctx, vec![egui::Event::PointerButton { pos: at, button, pressed: false, modifiers: m }]);
        frame(app, ctx, Vec::new());
    }

    /// A menu popup was shown in the last frame.
    fn menu_shown(ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.areas().visible_layer_ids().iter().any(|l| l.order == egui::Order::Foreground))
    }

    fn canvas(app: &CadApp) -> Rect {
        app.canvas.rect.unwrap_or(Rect::NOTHING)
    }

    fn view(app: &CadApp) -> (f64, f64, f64) {
        let v = app.session.state().map(|s| s.view()).unwrap_or_default();
        (v.center.x, v.center.y, v.height)
    }

    #[test]
    fn parts_are_found_where_they_are_drawn() {
        let l = Layout::new(Rect::from_min_size(pos2(200.0, 80.0), vec2(1000.0, 800.0)));
        assert_eq!(l.part_at(l.center), Some(Part::Face));
        assert_eq!(l.part_at(l.home.center()), Some(Part::Home));
        assert_eq!(l.part_at(l.center + vec2(0.0, -l.ring)), Some(Part::Ring), "N");
        assert_eq!(l.part_at(l.center + vec2(l.ring + 3.0, 0.0)), Some(Part::Ring), "E");
        assert_eq!(l.part_at(l.center + vec2(30.0, 30.0)), None, "between the face and the ring");
        assert!(l.bounds.contains_rect(l.home) && l.bounds.contains_rect(l.face));
    }

    #[test]
    fn viewcube_clicks_reach_the_cube_not_the_drawing() {
        let (mut app, ctx) = app_and_ctx();
        let l = Layout::new(canvas(&app));
        let fitted = view(&app);
        // Zoomed in, the Top face fits the drawing again (zoom extents) and nothing is picked.
        let _ = app.run("zoom.in", json!({}));
        assert_ne!(view(&app), fitted);
        click(&mut app, &ctx, l.center, egui::PointerButton::Primary);
        assert_eq!(view(&app), fitted, "Top face zooms to extents");
        assert!(app.session.pending_window.is_none(), "the click didn't start a selection window");
        assert!(app.session.selection().is_empty());
        // Home does the same.
        let _ = app.run("zoom.in", json!({}));
        click(&mut app, &ctx, l.home.center(), egui::PointerButton::Primary);
        assert_eq!(view(&app), fitted, "Home zooms to extents");
        // The ring explains that the 2D view can't rotate, and doesn't touch the drawing.
        click(&mut app, &ctx, l.center + vec2(0.0, -l.ring), egui::PointerButton::Primary);
        assert_eq!(app.session.log.last().map(String::as_str), Some(NO_TWIST));
        assert!(app.session.pending_window.is_none());
        assert_eq!(view(&app), fitted);
        // A click in the drawing still reaches the drawing.
        let empty = canvas(&app).center() + vec2(0.0, -300.0);
        click(&mut app, &ctx, empty, egui::PointerButton::Primary);
        assert!(app.session.pending_window.is_some(), "an empty-space click starts a selection window");
    }

    #[test]
    fn ucs_icon_and_wcs_pill_open_the_coordinate_system_menu() {
        let (mut app, ctx) = app_and_ctx();
        let [x, _] = ucs_arms(canvas(&app));
        click(&mut app, &ctx, x.center(), egui::PointerButton::Primary);
        assert!(menu_shown(&ctx), "UCS icon menu");
        assert!(app.session.pending_window.is_none(), "the click stayed on the icon");
        let l = Layout::new(canvas(&app));
        click(&mut app, &ctx, l.pill.center(), egui::PointerButton::Primary);
        assert!(menu_shown(&ctx), "WCS menu");
        assert!(app.session.pending_window.is_none());
    }

    #[test]
    fn viewcube_rotation_arrows_and_side_views() {
        let (mut app, ctx) = app_and_ctx();
        let l = Layout::new(canvas(&app));
        assert_eq!(app.ui.view_twist, 0);
        assert_eq!(app.ui.view_name, "Top");

        // Click CW rotation arrow
        click(&mut app, &ctx, l.rot_cw.center(), egui::PointerButton::Primary);
        assert_eq!(app.ui.view_twist, 1, "Rotated to West Up (90°)");

        click(&mut app, &ctx, l.rot_cw.center(), egui::PointerButton::Primary);
        assert_eq!(app.ui.view_twist, 2, "Rotated to South Up (180°)");

        // Click CCW rotation arrow
        click(&mut app, &ctx, l.rot_ccw.center(), egui::PointerButton::Primary);
        assert_eq!(app.ui.view_twist, 1, "Rotated CCW back to 90°");

        // Click bottom arrow for Front view
        click(&mut app, &ctx, l.arrow_bottom.center(), egui::PointerButton::Primary);
        assert_eq!(app.ui.view_name, "Front");

        // Click left arrow for Left view
        click(&mut app, &ctx, l.arrow_left.center(), egui::PointerButton::Primary);
        assert_eq!(app.ui.view_name, "Left");

        // Click Home resets to Top
        click(&mut app, &ctx, l.home.center(), egui::PointerButton::Primary);
        assert_eq!(app.ui.view_name, "Top");
    }
}
