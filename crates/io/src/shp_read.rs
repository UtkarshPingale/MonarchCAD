//! ESRI Shapefile (.shp) format reader for CADCraft.
//!
//! Converts ESRI Shapefile vector features (Point, MultiPoint, PolyLine, Polygon)
//! into native CADCraft Drawing entities (Point, LwPolyline) on dedicated layers
//! with vibrant GIS colors.

use std::path::Path;

use cadcraft_color::Color;
use cadcraft_doc::{Common, Drawing, EntityKind, HVal, Layer, LwPolyline, Point, Space};
use cadcraft_geom::{PolyVertex, Vec2, Vec3};

use crate::{IoError, Result};

/// Maximum shapefile records processed to guard against hostile allocations.
const MAX_SHP_RECORDS: usize = 1_000_000;
/// Maximum points in a single feature.
const MAX_POINTS_PER_FEATURE: usize = 200_000;
/// Maximum parts in a single feature.
const MAX_PARTS_PER_FEATURE: usize = 10_000;

/// Curated palette of distinct GIS layer colors (AutoCAD ACI codes).
const GIS_COLORS: &[u8] = &[
    1,   // Red
    3,   // Green
    4,   // Cyan
    2,   // Yellow
    6,   // Magenta
    5,   // Blue
    30,  // Orange
    70,  // Lime / Spring Green
    130, // Light Cyan / Sky Blue
    190, // Violet
    230, // Pink / Rose
];

fn pick_color(layer_name: &str) -> Color {
    let mut hash: u32 = 5381;
    for b in layer_name.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as u32);
    }
    let idx = (hash as usize) % GIS_COLORS.len();
    let aci = GIS_COLORS.get(idx).copied().unwrap_or(4);
    Color::Index(aci)
}

fn sanitize_layer_name(name: &str) -> String {
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or("Shapefile");
    let mut clean = String::new();
    for c in stem.chars() {
        if c.is_alphanumeric() || c == ' ' || c == '_' || c == '-' {
            clean.push(c);
        } else {
            clean.push('_');
        }
    }
    let trimmed = clean.trim();
    if trimmed.is_empty() { "Shapefile".to_string() } else { trimmed.to_string() }
}

/// Parse an ESRI .prj WKT string to extract a standard CRS identifier (e.g. "UTM_44N", "WGS84", "EPSG:3857").
pub fn parse_prj(text: &str) -> Option<String> {
    let upper = text.to_ascii_uppercase();
    for z in 1..=60 {
        let n1 = format!("UTM_ZONE_{z}N");
        let n2 = format!("UTM ZONE {z}N");
        let epsg_n = format!("326{z:02}");
        if upper.contains(&n1) || upper.contains(&n2) || upper.contains(&epsg_n) {
            return Some(format!("UTM_{z}N"));
        }
        let s1 = format!("UTM_ZONE_{z}S");
        let s2 = format!("UTM ZONE {z}S");
        let epsg_s = format!("327{z:02}");
        if upper.contains(&s1) || upper.contains(&s2) || upper.contains(&epsg_s) {
            return Some(format!("UTM_{z}S"));
        }
    }
    if let Some(pos) = upper.find("CENTRAL_MERIDIAN")
        && let Some(num_str) = upper[pos..].split(',').nth(1)
    {
        let clean: String = num_str.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
        if let Ok(cm) = clean.trim().parse::<f64>() {
            let z = ((cm + 180.0) / 6.0).floor() as i32 + 1;
            if (1..=60).contains(&z) {
                let north = !upper.contains("FALSE_NORTHING\",10000000");
                let hem = if north { "N" } else { "S" };
                return Some(format!("UTM_{z}{hem}"));
            }
        }
    }
    if upper.contains("3857") || upper.contains("PSEUDO-MERCATOR") {
        return Some("EPSG:3857".to_string());
    }
    if upper.contains("GCS_WGS_1984") || upper.contains("WGS 84") || upper.contains("4326") {
        return Some("WGS84".to_string());
    }
    None
}

fn load_prj_into(d: &mut Drawing, name: &str) {
    let prj_path = Path::new(name).with_extension("prj");
    if prj_path.is_file()
        && let Ok(prj_text) = std::fs::read_to_string(&prj_path)
        && let Some(crs) = parse_prj(&prj_text)
    {
        d.header.set("GEO_CRS", HVal::Str(crs));
    }
}

/// Read an ESRI Shapefile from bytes into a new CAD [`Drawing`].
pub fn read(bytes: &[u8], name: &str) -> Result<Drawing> {
    let mut d = Drawing::new_metric();
    let layer_name = sanitize_layer_name(name);
    let color = pick_color(&layer_name);

    d.layers.push(Layer { name: layer_name.clone(), color, ..Layer::default() });

    // Configure point display so point features are clearly visible markers.
    d.header.set("PDMODE", HVal::Int(35)); // Circle with cross
    d.header.set("PDSIZE", HVal::Real(0.0)); // Auto-scales relative to viewport
    d.header.set("INSUNITS", HVal::Int(0)); // Unitless / Geographic degrees

    load_prj_into(&mut d, name);

    import_shp_into(&mut d, bytes, &layer_name)?;
    Ok(d)
}

/// Import an ESRI Shapefile into an existing CAD [`Drawing`].
pub fn import_into(d: &mut Drawing, bytes: &[u8], name: &str) -> Result<usize> {
    let layer_name = sanitize_layer_name(name);
    if !d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(&layer_name)) {
        let color = pick_color(&layer_name);
        d.layers.push(Layer { name: layer_name.clone(), color, ..Layer::default() });
    }

    // Ensure points are visible if not already set.
    if d.header.i64("PDMODE", 0) == 0 {
        d.header.set("PDMODE", HVal::Int(35));
        d.header.set("PDSIZE", HVal::Real(0.0));
    }

    load_prj_into(d, name);

    import_shp_into(d, bytes, &layer_name)
}

fn import_shp_into(d: &mut Drawing, bytes: &[u8], layer_name: &str) -> Result<usize> {
    if bytes.len() < 100 {
        return Err(IoError::Format("SHP file is too small (header is less than 100 bytes)".into()));
    }

    let Some(header) = bytes.get(..100) else {
        return Err(IoError::Format("Invalid SHP header".into()));
    };

    // File code (Big-endian 9994)
    let file_code = i32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    if file_code != 9994 {
        return Err(IoError::Format(format!("Not a valid ESRI Shapefile (file code {file_code} != 9994)")));
    }

    // Bounding box from header (Little-endian f64s)
    let xmin = f64::from_le_bytes([header[36], header[37], header[38], header[39], header[40], header[41], header[42], header[43]]);
    let ymin = f64::from_le_bytes([header[44], header[45], header[46], header[47], header[48], header[49], header[50], header[51]]);
    let xmax = f64::from_le_bytes([header[52], header[53], header[54], header[55], header[56], header[57], header[58], header[59]]);
    let ymax = f64::from_le_bytes([header[60], header[61], header[62], header[63], header[64], header[65], header[66], header[67]]);

    if xmin.is_finite() && ymin.is_finite() && xmax.is_finite() && ymax.is_finite() {
        d.header.set("EXTMIN", HVal::Point(Vec3::new(xmin, ymin, 0.0)));
        d.header.set("EXTMAX", HVal::Point(Vec3::new(xmax, ymax, 0.0)));
    }

    let mut pos = 100usize;
    let mut count = 0usize;

    while pos + 8 <= bytes.len() && count < MAX_SHP_RECORDS {
        let Some(rec_hdr) = bytes.get(pos..pos + 8) else { break };
        pos += 8;

        let content_len_words = i32::from_be_bytes([rec_hdr[4], rec_hdr[5], rec_hdr[6], rec_hdr[7]]);
        if content_len_words <= 0 {
            continue;
        }
        let byte_len = (content_len_words as usize).saturating_mul(2);
        let Some(rec_data) = bytes.get(pos..pos + byte_len) else { break };
        pos += byte_len;

        if rec_data.len() < 4 {
            continue;
        }

        let shape_type = i32::from_le_bytes([rec_data[0], rec_data[1], rec_data[2], rec_data[3]]);
        let common = Common { layer: layer_name.to_string(), ..Common::default() };

        match shape_type {
            // Null Shape
            0 => {}

            // Point (1), PointZ (11), PointM (21)
            1 | 11 | 21 => {
                if rec_data.len() >= 20 {
                    let x = f64::from_le_bytes([
                        rec_data[4],
                        rec_data[5],
                        rec_data[6],
                        rec_data[7],
                        rec_data[8],
                        rec_data[9],
                        rec_data[10],
                        rec_data[11],
                    ]);
                    let y = f64::from_le_bytes([
                        rec_data[12],
                        rec_data[13],
                        rec_data[14],
                        rec_data[15],
                        rec_data[16],
                        rec_data[17],
                        rec_data[18],
                        rec_data[19],
                    ]);
                    let z = if shape_type == 11 && rec_data.len() >= 28 {
                        f64::from_le_bytes([
                            rec_data[20],
                            rec_data[21],
                            rec_data[22],
                            rec_data[23],
                            rec_data[24],
                            rec_data[25],
                            rec_data[26],
                            rec_data[27],
                        ])
                    } else {
                        0.0
                    };
                    if x.is_finite() && y.is_finite() {
                        let z_val = if z.is_finite() { z } else { 0.0 };
                        let _ = d.add(&Space::Model, common, EntityKind::Point(Point { p: Vec3::new(x, y, z_val), angle: 0.0 }));
                        count += 1;
                    }
                }
            }

            // MultiPoint (8), MultiPointZ (18), MultiPointM (28)
            8 | 18 | 28 => {
                if rec_data.len() >= 40 {
                    let num_points = i32::from_le_bytes([rec_data[36], rec_data[37], rec_data[38], rec_data[39]]) as usize;
                    let capped_pts = num_points.min(MAX_POINTS_PER_FEATURE);
                    let mut pt_offset = 40;
                    for _ in 0..capped_pts {
                        if pt_offset + 16 > rec_data.len() {
                            break;
                        }
                        let x = f64::from_le_bytes([
                            rec_data[pt_offset],
                            rec_data[pt_offset + 1],
                            rec_data[pt_offset + 2],
                            rec_data[pt_offset + 3],
                            rec_data[pt_offset + 4],
                            rec_data[pt_offset + 5],
                            rec_data[pt_offset + 6],
                            rec_data[pt_offset + 7],
                        ]);
                        let y = f64::from_le_bytes([
                            rec_data[pt_offset + 8],
                            rec_data[pt_offset + 9],
                            rec_data[pt_offset + 10],
                            rec_data[pt_offset + 11],
                            rec_data[pt_offset + 12],
                            rec_data[pt_offset + 13],
                            rec_data[pt_offset + 14],
                            rec_data[pt_offset + 15],
                        ]);
                        pt_offset += 16;
                        if x.is_finite() && y.is_finite() {
                            let _ = d.add(&Space::Model, common.clone(), EntityKind::Point(Point { p: Vec3::new(x, y, 0.0), angle: 0.0 }));
                            count += 1;
                        }
                    }
                }
            }

            // PolyLine (3), PolyLineZ (13), PolyLineM (23), Polygon (5), PolygonZ (15), PolygonM (25)
            3 | 13 | 23 | 5 | 15 | 25 if rec_data.len() >= 44 => {
                let num_parts = (i32::from_le_bytes([rec_data[36], rec_data[37], rec_data[38], rec_data[39]]) as usize).min(MAX_PARTS_PER_FEATURE);
                let num_points = (i32::from_le_bytes([rec_data[40], rec_data[41], rec_data[42], rec_data[43]]) as usize).min(MAX_POINTS_PER_FEATURE);
                let is_polygon = matches!(shape_type, 5 | 15 | 25);

                let parts_offset = 44;
                let points_offset = parts_offset + num_parts * 4;

                if points_offset <= rec_data.len() {
                    let mut parts = Vec::with_capacity(num_parts);
                    for p_idx in 0..num_parts {
                        let o = parts_offset + p_idx * 4;
                        if o + 4 <= rec_data.len() {
                            let part_start = i32::from_le_bytes([rec_data[o], rec_data[o + 1], rec_data[o + 2], rec_data[o + 3]]) as usize;
                            parts.push(part_start);
                        }
                    }
                    if parts.is_empty() {
                        parts.push(0);
                    }

                    for p_idx in 0..parts.len() {
                        let start = parts.get(p_idx).copied().unwrap_or(0);
                        let end = if p_idx + 1 < parts.len() { parts.get(p_idx + 1).copied().unwrap_or(num_points) } else { num_points };

                        if start >= end || end > num_points {
                            continue;
                        }

                        let mut vertices = Vec::with_capacity(end - start);
                        for pt_idx in start..end {
                            let pt_off = points_offset + pt_idx * 16;
                            if pt_off + 16 > rec_data.len() {
                                break;
                            }
                            let x = f64::from_le_bytes([
                                rec_data[pt_off],
                                rec_data[pt_off + 1],
                                rec_data[pt_off + 2],
                                rec_data[pt_off + 3],
                                rec_data[pt_off + 4],
                                rec_data[pt_off + 5],
                                rec_data[pt_off + 6],
                                rec_data[pt_off + 7],
                            ]);
                            let y = f64::from_le_bytes([
                                rec_data[pt_off + 8],
                                rec_data[pt_off + 9],
                                rec_data[pt_off + 10],
                                rec_data[pt_off + 11],
                                rec_data[pt_off + 12],
                                rec_data[pt_off + 13],
                                rec_data[pt_off + 14],
                                rec_data[pt_off + 15],
                            ]);
                            if x.is_finite() && y.is_finite() {
                                vertices.push(PolyVertex::new(Vec2::new(x, y)));
                            }
                        }

                        if vertices.len() < 2 {
                            continue;
                        }

                        // If polygon and last vertex equals first vertex, trim duplicate closing vertex
                        if is_polygon && vertices.len() >= 3 {
                            let first = vertices.first().map(|v| v.p).unwrap_or(Vec2::ZERO);
                            let last = vertices.last().map(|v| v.p).unwrap_or(Vec2::ZERO);
                            if (first - last).len2() < 1e-12 {
                                vertices.pop();
                            }
                        }

                        let poly = LwPolyline { vertices, closed: is_polygon, const_width: 0.0, elevation: 0.0, plinegen: true };

                        let _ = d.add(&Space::Model, common.clone(), EntityKind::LwPolyline(poly));
                        count += 1;
                    }
                }
            }

            _ => {}
        }
    }

    Ok(count)
}
