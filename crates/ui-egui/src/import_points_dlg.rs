use cadcraft_color::Color;
use cadcraft_doc::{Common, Drawing, EntityKind, HAlign, HVal, Layer, Point, Space, Text, VAlign};
use cadcraft_geom::Vec3;
use egui::{Color32, RichText, Stroke};
use serde_json::json;

use crate::CadApp;

pub const MAX_IMPORT_POINTS: usize = 500_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PointFileFormat {
    #[default]
    Auto,
    PenzdComma,
    PnezdComma,
    EnzComma,
    NezComma,
    PneComma,
    PenComma,
    XyzSpace,
    XyzComma,
}

impl PointFileFormat {
    pub const ALL: [PointFileFormat; 9] = [
        PointFileFormat::Auto,
        PointFileFormat::PenzdComma,
        PointFileFormat::PnezdComma,
        PointFileFormat::EnzComma,
        PointFileFormat::NezComma,
        PointFileFormat::PneComma,
        PointFileFormat::PenComma,
        PointFileFormat::XyzSpace,
        PointFileFormat::XyzComma,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            PointFileFormat::Auto => "Auto-detect Format",
            PointFileFormat::PenzdComma => "PENZD (comma delimited)",
            PointFileFormat::PnezdComma => "PNEZD (comma delimited)",
            PointFileFormat::EnzComma => "ENZ (comma delimited)",
            PointFileFormat::NezComma => "NEZ (comma delimited)",
            PointFileFormat::PneComma => "PNE (comma delimited)",
            PointFileFormat::PenComma => "PEN (comma delimited)",
            PointFileFormat::XyzSpace => "XYZ (space delimited)",
            PointFileFormat::XyzComma => "XYZ (comma delimited)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct SurveyPoint {
    pub id: String,
    pub easting: f64,   // X coordinate
    pub northing: f64,  // Y coordinate
    pub elevation: f64, // Z coordinate
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct PointImportOptions {
    pub format: PointFileFormat,
    pub layer_name: String,
    pub create_labels: bool,
    pub elevation_offset: f64,
}

fn detect_delimiter(first_lines: &[&str]) -> char {
    let mut comma_count = 0;
    let mut semi_count = 0;
    let mut tab_count = 0;

    for line in first_lines {
        comma_count += line.chars().filter(|c| *c == ',').count();
        semi_count += line.chars().filter(|c| *c == ';').count();
        tab_count += line.chars().filter(|c| *c == '\t').count();
    }

    if comma_count >= semi_count && comma_count >= tab_count && comma_count > 0 {
        ','
    } else if semi_count > comma_count && semi_count > tab_count {
        ';'
    } else if tab_count > comma_count && tab_count > semi_count {
        '\t'
    } else {
        ' '
    }
}

fn split_row(line: &str, delim: char) -> Vec<&str> {
    if delim == ' ' { line.split_whitespace().map(str::trim).filter(|s| !s.is_empty()).collect() } else { line.split(delim).map(str::trim).collect() }
}

fn is_numeric(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    t.parse::<f64>().is_ok()
}

fn parse_num(s: &str) -> Option<f64> {
    let v = s.trim().parse::<f64>().ok()?;
    if v.is_finite() { Some(v) } else { None }
}

fn detect_format(lines: &[&str], delim: char) -> PointFileFormat {
    if lines.is_empty() {
        return PointFileFormat::PenzdComma;
    }

    if let Some(first) = lines.first() {
        let cols: Vec<String> = split_row(first, delim).iter().map(|s| s.to_ascii_lowercase()).collect();
        let has_easting = cols.iter().any(|c| c.contains("east") || c == "x");
        let has_northing = cols.iter().any(|c| c.contains("north") || c == "y");
        let has_pt = cols.iter().any(|c| c.contains("point") || c.contains("pt") || c == "id" || c == "no");

        if has_easting && has_northing {
            let e_idx = cols.iter().position(|c| c.contains("east") || c == "x").unwrap_or(0);
            let n_idx = cols.iter().position(|c| c.contains("north") || c == "y").unwrap_or(1);
            if e_idx < n_idx {
                return if has_pt { PointFileFormat::PenzdComma } else { PointFileFormat::EnzComma };
            } else {
                return if has_pt { PointFileFormat::PnezdComma } else { PointFileFormat::NezComma };
            }
        }
    }

    for line in lines {
        let parts = split_row(line, delim);
        if parts.len() < 2 {
            continue;
        }
        if !is_numeric(parts[0]) && !is_numeric(parts[1]) {
            continue;
        }

        match parts.len() {
            2 => return PointFileFormat::PenComma,
            3 => return if delim == ' ' { PointFileFormat::XyzSpace } else { PointFileFormat::EnzComma },
            4 => {
                if is_numeric(parts[0]) && is_numeric(parts[1]) && is_numeric(parts[2]) {
                    let v1 = parse_num(parts[1]).unwrap_or(0.0);
                    let v2 = parse_num(parts[2]).unwrap_or(0.0);
                    if v1 > 1_000_000.0 && v2 < 1_000_000.0 {
                        return PointFileFormat::PneComma;
                    }
                }
                return PointFileFormat::PenComma;
            }
            _ => {
                if parts.len() >= 5 && is_numeric(parts[1]) && is_numeric(parts[2]) {
                    let v1 = parse_num(parts[1]).unwrap_or(0.0);
                    let v2 = parse_num(parts[2]).unwrap_or(0.0);
                    if v1 > 1_000_000.0 && v2 < 1_000_000.0 {
                        return PointFileFormat::PnezdComma;
                    }
                }
                return PointFileFormat::PenzdComma;
            }
        }
    }

    PointFileFormat::PenzdComma
}

pub fn parse_survey_points(bytes: &[u8], format: PointFileFormat) -> Vec<SurveyPoint> {
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    let raw_lines: Vec<&str> = text.lines().map(str::trim).filter(|s| !s.is_empty()).collect();
    if raw_lines.is_empty() {
        return Vec::new();
    }

    let delim = match format {
        PointFileFormat::XyzSpace => ' ',
        PointFileFormat::Auto => detect_delimiter(&raw_lines.iter().take(5).copied().collect::<Vec<_>>()),
        _ => ',',
    };

    let actual_format =
        if format == PointFileFormat::Auto { detect_format(&raw_lines.iter().take(10).copied().collect::<Vec<_>>(), delim) } else { format };

    let mut points = Vec::new();
    let mut auto_id = 1;

    for line in raw_lines {
        if points.len() >= MAX_IMPORT_POINTS {
            break;
        }

        let parts = split_row(line, delim);
        if parts.is_empty() {
            continue;
        }

        let sample_num = match actual_format {
            PointFileFormat::PnezdComma | PointFileFormat::PenzdComma | PointFileFormat::PneComma | PointFileFormat::PenComma => {
                parts.get(1).copied().unwrap_or("")
            }
            _ => parts.first().copied().unwrap_or(""),
        };
        if !is_numeric(sample_num) {
            continue;
        }

        let mut pt = SurveyPoint::default();

        match actual_format {
            PointFileFormat::PenzdComma => {
                pt.id = parts.first().copied().unwrap_or("").to_string();
                pt.easting = parts.get(1).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.northing = parts.get(2).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.elevation = parts.get(3).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.description = parts.get(4..).map(|s| s.join(" ")).unwrap_or_default();
            }
            PointFileFormat::PnezdComma => {
                pt.id = parts.first().copied().unwrap_or("").to_string();
                pt.northing = parts.get(1).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.easting = parts.get(2).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.elevation = parts.get(3).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.description = parts.get(4..).map(|s| s.join(" ")).unwrap_or_default();
            }
            PointFileFormat::EnzComma => {
                pt.id = auto_id.to_string();
                pt.easting = parts.first().and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.northing = parts.get(1).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.elevation = parts.get(2).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.description = parts.get(3..).map(|s| s.join(" ")).unwrap_or_default();
            }
            PointFileFormat::NezComma => {
                pt.id = auto_id.to_string();
                pt.northing = parts.first().and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.easting = parts.get(1).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.elevation = parts.get(2).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.description = parts.get(3..).map(|s| s.join(" ")).unwrap_or_default();
            }
            PointFileFormat::PenComma => {
                pt.id = parts.first().copied().unwrap_or("").to_string();
                pt.easting = parts.get(1).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.northing = parts.get(2).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.description = parts.get(3..).map(|s| s.join(" ")).unwrap_or_default();
            }
            PointFileFormat::PneComma => {
                pt.id = parts.first().copied().unwrap_or("").to_string();
                pt.northing = parts.get(1).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.easting = parts.get(2).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.description = parts.get(3..).map(|s| s.join(" ")).unwrap_or_default();
            }
            PointFileFormat::XyzSpace | PointFileFormat::XyzComma => {
                pt.id = auto_id.to_string();
                pt.easting = parts.first().and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.northing = parts.get(1).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.elevation = parts.get(2).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.description = parts.get(3..).map(|s| s.join(" ")).unwrap_or_default();
            }
            PointFileFormat::Auto => {}
        }

        if pt.id.is_empty() {
            pt.id = auto_id.to_string();
        }
        auto_id += 1;
        points.push(pt);
    }

    points
}

pub fn import_points_into(d: &mut Drawing, points: &[SurveyPoint], opts: &PointImportOptions) -> Result<usize, String> {
    if points.is_empty() {
        return Ok(0);
    }

    let layer_name = if opts.layer_name.trim().is_empty() { "POINTS".to_string() } else { opts.layer_name.trim().to_string() };
    let labels_layer = format!("{layer_name}_LABELS");

    if !d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(&layer_name)) {
        d.layers.push(Layer { name: layer_name.clone(), color: Color::Index(4), ..Default::default() });
    }

    if opts.create_labels && !d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(&labels_layer)) {
        d.layers.push(Layer { name: labels_layer.clone(), color: Color::Index(2), ..Default::default() });
    }

    if d.header.i64("PDMODE", 0) == 0 {
        d.header.set("PDMODE", HVal::Int(35));
        d.header.set("PDSIZE", HVal::Real(0.0));
    }

    let common = Common { layer: layer_name, ..Default::default() };

    let common_lbl = Common { layer: labels_layer, ..Default::default() };

    let mut min_x = f64::MAX;
    let mut max_x = f64::MIN;
    let mut min_y = f64::MAX;
    let mut max_y = f64::MIN;

    for pt in points {
        min_x = min_x.min(pt.easting);
        max_x = max_x.max(pt.easting);
        min_y = min_y.min(pt.northing);
        max_y = max_y.max(pt.northing);
    }

    let span = (max_x - min_x).max(max_y - min_y);
    let text_height = if span > 10.0 { (span / 250.0).clamp(0.5, 20.0) } else { 0.25 };
    let text_offset = text_height * 0.6;

    let mut count = 0;
    for pt in points {
        let z = pt.elevation + opts.elevation_offset;
        let p3 = Vec3::new(pt.easting, pt.northing, z);

        let _ = d.add(&Space::Model, common.clone(), EntityKind::Point(Point { p: p3, angle: 0.0 }));
        count += 1;

        if opts.create_labels {
            let mut label_parts = Vec::new();
            if !pt.id.is_empty() {
                label_parts.push(pt.id.clone());
            }
            if pt.elevation != 0.0 || opts.elevation_offset != 0.0 {
                label_parts.push(format!("{:.3}", z));
            }
            if !pt.description.is_empty() {
                label_parts.push(pt.description.clone());
            }

            if !label_parts.is_empty() {
                let label_text = label_parts.join("  ");
                let ins = Vec3::new(pt.easting + text_offset, pt.northing + text_offset, z);
                let txt = Text {
                    insert: ins,
                    align_pt: None,
                    height: text_height,
                    value: label_text,
                    rotation: 0.0,
                    width_factor: 1.0,
                    oblique: 0.0,
                    style: "STANDARD".to_string(),
                    halign: HAlign::Left,
                    valign: VAlign::Bottom,
                };
                let _ = d.add(&Space::Model, common_lbl.clone(), EntityKind::Text(txt));
            }
        }
    }

    Ok(count)
}

#[derive(Clone, Debug)]
pub struct ImportPointsState {
    pub file_path: Option<String>,
    pub file_name: String,
    pub file_bytes: Vec<u8>,
    pub selected_format: PointFileFormat,
    pub parsed_points: Vec<SurveyPoint>,
    pub point_group: String,
    pub add_to_group: bool,
    pub create_labels: bool,
    pub do_elevation_adjustment: bool,
    pub elevation_offset: f64,
    pub do_coordinate_transformation: bool,
    pub transform_from_wgs84: bool,
    pub status: String,
}

impl Default for ImportPointsState {
    fn default() -> Self {
        Self {
            file_path: None,
            file_name: String::new(),
            file_bytes: Vec::new(),
            selected_format: PointFileFormat::Auto,
            parsed_points: Vec::new(),
            point_group: "POINTS".to_string(),
            add_to_group: true,
            create_labels: true,
            do_elevation_adjustment: false,
            elevation_offset: 0.0,
            do_coordinate_transformation: false,
            transform_from_wgs84: false,
            status: "No file selected. Click '+' to add a survey point file (CSV, TXT, XYZ)".to_string(),
        }
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let mut state = app.import_points_dlg.take().unwrap_or_default();
    let mut do_import = false;
    let mut browse_clicked = false;
    let mut clear_clicked = false;
    let mut cancel_clicked = false;
    let mut help_clicked = false;

    egui::Window::new(crate::tl!("Import Points"))
        .id(egui::Id::new("civil3d_import_points_dialog"))
        .open(open)
        .default_size([540.0, 620.0])
        .default_pos([530.0, 140.0])
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.set_width(520.0);
            ui.spacing_mut().item_spacing.y = 8.0;

            // 1. Selected Files Section
            ui.label(RichText::new("Selected Files:").strong());
            egui::Frame::new()
                .fill(Color32::from_rgb(24, 28, 36))
                .stroke(Stroke::new(1.0, Color32::from_rgb(60, 68, 80)))
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.set_min_width(450.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("File Name").strong().color(Color32::from_rgb(148, 163, 184)));
                                ui.add_space(140.0);
                                ui.label(RichText::new("Status").strong().color(Color32::from_rgb(148, 163, 184)));
                            });
                            ui.separator();

                            if let Some(path) = &state.file_path {
                                ui.horizontal(|ui| {
                                    let name = if state.file_name.is_empty() { path.as_str() } else { state.file_name.as_str() };
                                    ui.label(RichText::new(format!("📄 {name}")).strong().color(Color32::from_rgb(241, 245, 249)));
                                    ui.add_space(20.0);
                                    let valid = !state.parsed_points.is_empty();
                                    let status_color = if valid { Color32::from_rgb(34, 197, 94) } else { Color32::from_rgb(239, 68, 68) };
                                    ui.label(RichText::new(&state.status).color(status_color));
                                });
                            } else {
                                ui.label(RichText::new("(No files selected)").italics().color(Color32::from_rgb(100, 116, 139)));
                            }
                        });

                        ui.vertical(|ui| {
                            if ui
                                .button(RichText::new("➕").size(16.0).color(Color32::from_rgb(34, 197, 94)))
                                .on_hover_text("Add survey point file (CSV, TXT, XYZ, ASC, DAT)")
                                .clicked()
                            {
                                browse_clicked = true;
                            }
                            if ui
                                .button(RichText::new("✖").size(14.0).color(Color32::from_rgb(239, 68, 68)))
                                .on_hover_text("Remove selected file")
                                .clicked()
                            {
                                clear_clicked = true;
                            }
                        });
                    });
                });

            // 2. Specify point file format section
            ui.label(RichText::new("Specify point file format (filtering OFF):").strong());
            egui::Frame::new()
                .fill(Color32::from_rgb(20, 24, 32))
                .stroke(Stroke::new(1.0, Color32::from_rgb(60, 68, 80)))
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::same(4))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("import_points_format_scroll").max_height(110.0).auto_shrink([false, false]).show(
                        ui,
                        |ui| {
                            for fmt in PointFileFormat::ALL {
                                let selected = state.selected_format == fmt;
                                let text = fmt.display_name();
                                if ui.selectable_label(selected, text).clicked() {
                                    state.selected_format = fmt;
                                    if !state.file_bytes.is_empty() {
                                        state.parsed_points = parse_survey_points(&state.file_bytes, state.selected_format);
                                        state.status = format!("✓ {} points parsed", state.parsed_points.len());
                                    }
                                }
                            }
                        },
                    );
                });

            // 3. Preview Section
            ui.label(RichText::new("Preview:").strong());
            egui::Frame::new()
                .fill(Color32::from_rgb(15, 18, 24))
                .stroke(Stroke::new(1.0, Color32::from_rgb(50, 58, 70)))
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    egui::ScrollArea::both().id_salt("import_points_preview_scroll").max_height(140.0).auto_shrink([false, false]).show(ui, |ui| {
                        egui::Grid::new("points_preview_grid").striped(true).min_col_width(85.0).show(ui, |ui| {
                            ui.label(RichText::new("Point #").strong().color(Color32::from_rgb(148, 163, 184)));
                            ui.label(RichText::new("Easting (X)").strong().color(Color32::from_rgb(148, 163, 184)));
                            ui.label(RichText::new("Northing (Y)").strong().color(Color32::from_rgb(148, 163, 184)));
                            ui.label(RichText::new("Elevation (Z)").strong().color(Color32::from_rgb(148, 163, 184)));
                            ui.label(RichText::new("Raw Description").strong().color(Color32::from_rgb(148, 163, 184)));
                            ui.end_row();

                            if state.parsed_points.is_empty() {
                                ui.label("1");
                                ui.label("500123.450");
                                ui.label("3124567.890");
                                ui.label("125.400");
                                ui.label("IRON_PIN (sample)");
                                ui.end_row();
                                ui.label("2");
                                ui.label("500150.000");
                                ui.label("3124600.000");
                                ui.label("126.100");
                                ui.label("CORNER (sample)");
                                ui.end_row();
                            } else {
                                for pt in state.parsed_points.iter().take(30) {
                                    ui.label(&pt.id);
                                    ui.label(format!("{:.3}", pt.easting));
                                    ui.label(format!("{:.3}", pt.northing));
                                    ui.label(format!("{:.3}", pt.elevation));
                                    ui.label(&pt.description);
                                    ui.end_row();
                                }
                            }
                        });
                    });
                });

            // 4. Point Group & Annotations
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut state.add_to_group, "Add Points to Point Group / Layer:");
                    if state.add_to_group {
                        ui.add_sized([120.0, 22.0], egui::TextEdit::singleline(&mut state.point_group));
                    }
                });
                ui.checkbox(&mut state.create_labels, "Create Point Labels (Point #, Elevation, Description)");
            });

            // 5. Advanced options
            ui.group(|ui| {
                ui.label(RichText::new("Advanced options:").strong());
                ui.horizontal(|ui| {
                    ui.checkbox(&mut state.do_elevation_adjustment, "Do elevation adjustment if possible");
                    if state.do_elevation_adjustment {
                        ui.add(egui::DragValue::new(&mut state.elevation_offset).speed(0.1).prefix("Offset: "));
                    }
                });
                ui.checkbox(&mut state.do_coordinate_transformation, "Do coordinate transformation if possible");
                if state.do_coordinate_transformation {
                    ui.checkbox(&mut state.transform_from_wgs84, "Convert WGS84 Lat/Lon (degrees) to Current UTM Zone");
                }
            });

            ui.add_space(4.0);

            // 6. Action buttons (OK, Cancel, Help)
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_ok = !state.parsed_points.is_empty();
                    if ui.add_enabled(can_ok, egui::Button::new(RichText::new("   OK   ").strong())).clicked() {
                        do_import = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel_clicked = true;
                    }
                    if ui.button("Help").clicked() {
                        help_clicked = true;
                    }
                });
            });
        });

    if cancel_clicked {
        *open = false;
    }
    if help_clicked {
        app.session.echo("Civil 3D Point Formats: PNEZD = Point#, Northing, Easting, Elevation, Description. PENZD = Point#, Easting, Northing, Elevation, Description.");
    }

    // Handle Browse button
    if browse_clicked {
        let path_opt = if let Some(pick) = app.services.pick_points.as_ref() {
            pick()
        } else if let Some(pick) = app.services.pick_open.as_ref() {
            pick()
        } else {
            None
        };

        if let Some(path) = path_opt
            && let Ok(bytes) = std::fs::read(&path)
        {
            let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "survey.csv".to_string());
            let parsed = parse_survey_points(&bytes, state.selected_format);
            state.file_path = Some(path);
            state.file_name = name;
            state.status = if parsed.is_empty() { "No valid coordinates found".to_string() } else { format!("✓ {} points parsed", parsed.len()) };
            state.parsed_points = parsed;
            state.file_bytes = bytes;
        }
    }

    // Handle Clear button
    if clear_clicked {
        state.file_path = None;
        state.file_name.clear();
        state.file_bytes.clear();
        state.parsed_points.clear();
        state.status = "No file selected".to_string();
    }

    // Handle OK button (execute import)
    if do_import && !state.parsed_points.is_empty() {
        let prev_empty = app.session.state().map(|s| s.doc.entity_count() == 0).unwrap_or(false);

        // Transform coordinates if WGS84 -> UTM was selected
        let mut points_to_import = state.parsed_points.clone();
        if state.do_coordinate_transformation && state.transform_from_wgs84 {
            let zone = match app.map_tiles.current_crs {
                crate::map_tiles::CoordSystem::Utm { zone, .. } => zone,
                _ => 43,
            };
            for pt in &mut points_to_import {
                // If coordinates look like lat/lon degrees (e.g. lat ~ 20..35, lon ~ 68..95)
                let lat = pt.northing;
                let lon = pt.easting;
                if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) {
                    let (utm_e, utm_n) = crate::map_tiles::latlon_to_utm(lat, lon, zone, true);
                    pt.easting = utm_e;
                    pt.northing = utm_n;
                }
            }
        }

        let opts = PointImportOptions {
            format: state.selected_format,
            layer_name: if state.add_to_group { state.point_group.clone() } else { "POINTS".to_string() },
            create_labels: state.create_labels,
            elevation_offset: if state.do_elevation_adjustment { state.elevation_offset } else { 0.0 },
        };

        if let Ok(doc) = app.session.doc_mut() {
            match import_points_into(doc, &points_to_import, &opts) {
                Ok(count) => {
                    let layer = &opts.layer_name;
                    let file = &state.file_name;
                    app.session.echo(format!("Successfully imported {count} points from '{file}' onto layer '{layer}'."));
                    if prev_empty {
                        let _ = app.run("zoom.extents", json!({}));
                    }
                    *open = false;
                }
                Err(e) => {
                    app.session.echo(format!("Point import failed: {e}"));
                }
            }
        }
    }

    if *open {
        app.import_points_dlg = Some(state);
    } else {
        app.import_points_dlg = None;
    }
}
