//! CSV / ASCII point file reader for CADCraft (AutoCAD Civil 3D parity).
//!
//! Supports standard Civil 3D point file formats:
//! - PNEZD (comma or space delimited: Point#, Northing, Easting, Elevation, Description)
//! - PENZD (comma or space delimited: Point#, Easting, Northing, Elevation, Description)
//! - ENZ (Easting, Northing, Elevation)
//! - NEZ (Northing, Easting, Elevation)
//! - PNE (Point#, Northing, Easting)
//! - PEN (Point#, Easting, Northing)
//! - XYZ (space or comma delimited: X, Y, Z)
//! - Auto-detect (from headers or numerical patterns)

use std::path::Path;

use cadcraft_color::Color;
use cadcraft_doc::{Common, Drawing, EntityKind, HAlign, HVal, Layer, Point, Space, Text, VAlign};
use cadcraft_geom::Vec3;

use crate::{IoError, Result};

/// Maximum points imported to guard against hostile allocations.
pub const MAX_IMPORT_POINTS: usize = 500_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PointFileFormat {
    #[default]
    Auto,
    PnezdComma,
    PenzdComma,
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

    pub fn from_key(key: &str) -> Self {
        let k = key.trim().to_ascii_lowercase();
        if k.contains("penzd") {
            PointFileFormat::PenzdComma
        } else if k.contains("pnezd") {
            PointFileFormat::PnezdComma
        } else if k.contains("enz") {
            PointFileFormat::EnzComma
        } else if k.contains("nez") {
            PointFileFormat::NezComma
        } else if k.contains("pen") {
            PointFileFormat::PenComma
        } else if k.contains("pne") {
            PointFileFormat::PneComma
        } else if k.contains("xyz_space") || (k.contains("xyz") && k.contains("space")) {
            PointFileFormat::XyzSpace
        } else if k.contains("xyz") {
            PointFileFormat::XyzComma
        } else {
            PointFileFormat::Auto
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

impl Default for PointImportOptions {
    fn default() -> Self {
        Self { format: PointFileFormat::Auto, layer_name: "POINTS".to_string(), create_labels: true, elevation_offset: 0.0 }
    }
}

/// Detect the delimiter of a text file (comma, semicolon, tab, or whitespace).
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

/// Check if a string looks like a number.
fn is_numeric(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    t.parse::<f64>().is_ok()
}

/// Parse a string into a float, returning None if NaN or Infinite.
fn parse_num(s: &str) -> Option<f64> {
    let v = s.trim().parse::<f64>().ok()?;
    if v.is_finite() { Some(v) } else { None }
}

/// Auto-detect point file format based on headers and sample rows.
fn detect_format(lines: &[&str], delim: char) -> PointFileFormat {
    if lines.is_empty() {
        return PointFileFormat::PenzdComma;
    }

    // Check header row if present
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

    // Sample numeric rows to inspect column count and coordinate magnitude
    for line in lines {
        let parts = split_row(line, delim);
        if parts.len() < 2 {
            continue;
        }
        if !is_numeric(parts[0]) && !is_numeric(parts[1]) {
            continue; // Header row
        }

        match parts.len() {
            2 => return PointFileFormat::PenComma,
            3 => return if delim == ' ' { PointFileFormat::XyzSpace } else { PointFileFormat::EnzComma },
            4 => {
                // Point#, East, North, Elev OR East, North, Elev, Desc
                if is_numeric(parts[0]) && is_numeric(parts[1]) && is_numeric(parts[2]) {
                    // Inspect Easting vs Northing magnitude:
                    // In UTM / State Plane, Northing is often larger than Easting (e.g. 7 digits vs 6 digits)
                    let v1 = parse_num(parts[1]).unwrap_or(0.0);
                    let v2 = parse_num(parts[2]).unwrap_or(0.0);
                    if v1 > 1_000_000.0 && v2 < 1_000_000.0 {
                        return PointFileFormat::PneComma; // PNE (North first)
                    }
                }
                return PointFileFormat::PenComma;
            }
            _ => {
                // 5 or more columns
                if parts.len() >= 5 && is_numeric(parts[1]) && is_numeric(parts[2]) {
                    let v1 = parse_num(parts[1]).unwrap_or(0.0);
                    let v2 = parse_num(parts[2]).unwrap_or(0.0);
                    if v1 > 1_000_000.0 && v2 < 1_000_000.0 {
                        return PointFileFormat::PnezdComma; // Northing before Easting
                    }
                }
                return PointFileFormat::PenzdComma; // Easting before Northing (standard Civil 3D default)
            }
        }
    }

    PointFileFormat::PenzdComma
}

/// Parse survey points from text bytes using the specified format.
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

        // Skip headers where coordinate fields are non-numeric
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
                // Point#, Easting, Northing, Elevation, Description
                pt.id = parts.first().copied().unwrap_or("").to_string();
                pt.easting = parts.get(1).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.northing = parts.get(2).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.elevation = parts.get(3).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.description = parts.get(4..).map(|s| s.join(" ")).unwrap_or_default();
            }
            PointFileFormat::PnezdComma => {
                // Point#, Northing, Easting, Elevation, Description
                pt.id = parts.first().copied().unwrap_or("").to_string();
                pt.northing = parts.get(1).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.easting = parts.get(2).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.elevation = parts.get(3).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.description = parts.get(4..).map(|s| s.join(" ")).unwrap_or_default();
            }
            PointFileFormat::EnzComma => {
                // Easting, Northing, Elevation
                pt.id = auto_id.to_string();
                pt.easting = parts.first().and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.northing = parts.get(1).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.elevation = parts.get(2).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.description = parts.get(3..).map(|s| s.join(" ")).unwrap_or_default();
            }
            PointFileFormat::NezComma => {
                // Northing, Easting, Elevation
                pt.id = auto_id.to_string();
                pt.northing = parts.first().and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.easting = parts.get(1).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.elevation = parts.get(2).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.description = parts.get(3..).map(|s| s.join(" ")).unwrap_or_default();
            }
            PointFileFormat::PenComma => {
                // Point#, Easting, Northing
                pt.id = parts.first().copied().unwrap_or("").to_string();
                pt.easting = parts.get(1).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.northing = parts.get(2).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.description = parts.get(3..).map(|s| s.join(" ")).unwrap_or_default();
            }
            PointFileFormat::PneComma => {
                // Point#, Northing, Easting
                pt.id = parts.first().copied().unwrap_or("").to_string();
                pt.northing = parts.get(1).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.easting = parts.get(2).and_then(|s| parse_num(s)).unwrap_or(0.0);
                pt.description = parts.get(3..).map(|s| s.join(" ")).unwrap_or_default();
            }
            PointFileFormat::XyzSpace | PointFileFormat::XyzComma => {
                // X (Easting), Y (Northing), Z (Elevation)
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

/// Import parsed survey points directly into a CADCraft Drawing.
pub fn import_points_into(d: &mut Drawing, points: &[SurveyPoint], opts: &PointImportOptions) -> Result<usize> {
    if points.is_empty() {
        return Ok(0);
    }

    let layer_name = if opts.layer_name.trim().is_empty() { "POINTS".to_string() } else { opts.layer_name.trim().to_string() };
    let labels_layer = format!("{layer_name}_LABELS");

    // Ensure layers exist with distinct ACI colors
    if !d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(&layer_name)) {
        d.layers.push(Layer {
            name: layer_name.clone(),
            color: Color::Index(4), // Cyan for points
            ..Default::default()
        });
    }

    if opts.create_labels && !d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(&labels_layer)) {
        d.layers.push(Layer {
            name: labels_layer.clone(),
            color: Color::Index(2), // Yellow for point labels
            ..Default::default()
        });
    }

    // Ensure point markers are visible as circle with cross (PDMODE 35)
    if d.header.i64("PDMODE", 0) == 0 {
        d.header.set("PDMODE", HVal::Int(35));
        d.header.set("PDSIZE", HVal::Real(0.0));
    }

    let common = Common { layer: layer_name.clone(), ..Default::default() };

    let common_lbl = Common { layer: labels_layer, ..Default::default() };

    // Estimate suitable label text height from point coordinate spread
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

        // 1. Add AutoCAD Point entity
        let _ = d.add(&Space::Model, common.clone(), EntityKind::Point(Point { p: p3, angle: 0.0 }));
        count += 1;

        // 2. Add Civil 3D style point annotation if enabled
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

/// Read a CSV / TXT point file and produce a standalone Drawing.
pub fn read(bytes: &[u8], name: &str) -> Result<Drawing> {
    let mut d = Drawing::new_metric();
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or("POINTS");
    d.header.set("DWGNAME", HVal::Str(format!("{stem}.dwg")));

    let points = parse_survey_points(bytes, PointFileFormat::Auto);
    if points.is_empty() {
        return Err(IoError::Format(format!("{name}: no valid coordinates found in point file")));
    }

    let opts = PointImportOptions { format: PointFileFormat::Auto, layer_name: stem.to_uppercase(), create_labels: true, elevation_offset: 0.0 };

    import_points_into(&mut d, &points, &opts)?;
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_penzd_csv() {
        let csv = b"101,500123.45,3124567.89,125.40,IRON_PIN\n102,500150.00,3124600.00,126.10,CORNER\n";
        let pts = parse_survey_points(csv, PointFileFormat::PenzdComma);
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].id, "101");
        assert_eq!(pts[0].easting, 500123.45);
        assert_eq!(pts[0].northing, 3124567.89);
        assert_eq!(pts[0].elevation, 125.40);
        assert_eq!(pts[0].description, "IRON_PIN");
    }

    #[test]
    fn parses_xyz_space() {
        let txt = b"10.5 20.25 100.0\n30.0 40.0 105.5\n";
        let pts = parse_survey_points(txt, PointFileFormat::XyzSpace);
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].easting, 10.5);
        assert_eq!(pts[0].northing, 20.25);
        assert_eq!(pts[0].elevation, 100.0);
    }

    #[test]
    fn auto_detects_headers_and_imports_into_drawing() {
        let csv = b"PointNumber,Easting,Northing,Elevation,Description\n1,100.0,200.0,15.0,BENCHMARK\n2,150.0,250.0,16.2,TREE\n";
        let mut d = Drawing::new_metric();
        let pts = parse_survey_points(csv, PointFileFormat::Auto);
        assert_eq!(pts.len(), 2);
        let count = import_points_into(&mut d, &pts, &PointImportOptions::default()).unwrap();
        assert_eq!(count, 2);
        assert_eq!(d.model.len(), 4); // 2 Points + 2 Text labels
    }
}
