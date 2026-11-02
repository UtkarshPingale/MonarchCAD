//! ECW (Enhanced Compression Wavelet) orthomosaic and aerial raster reader.
//!
//! Maps an ECW orthophoto into a CAD [`Drawing`]:
//! - Places an AutoCAD-compliant [`EntityKind::Image`] entity with exact georeferenced insertion
//!   point, scale factors, pixel dimensions and file path.
//! - Creates a closed boundary [`LwPolyline`] showing the exact ground footprint.
//! - Places corner markers and coordinate callout annotations on layer `ORTHO_INFO`.
//! - Automatically resolves and configures the Coordinate Reference System (CRS)
//!   (e.g. UTM Zone 43N / WGS84) in drawing headers for seamless background map integration.

use std::path::Path;

use cadcraft_color::Color;
use cadcraft_doc::{Common, Drawing, EntityKind, HAlign, HVal, Image, Layer, LwPolyline, Point, Space, Text, VAlign};
use cadcraft_geom::{PolyVertex, Vec2, Vec3};

use crate::ecw::EcwImage;
use crate::{IoError, Result};

/// Read an ECW aerial image into a new georeferenced CAD [`Drawing`].
pub fn read(bytes: &[u8], name: &str) -> Result<Drawing> {
    let mut d = Drawing::new_metric();
    import_into(&mut d, bytes, name)?;
    Ok(d)
}

/// Import an ECW aerial image into an existing CAD [`Drawing`].
pub fn import_into(d: &mut Drawing, bytes: &[u8], name: &str) -> Result<usize> {
    let p = Path::new(name);
    let img = if p.is_file() {
        EcwImage::open(p).map_err(|e| IoError::Format(format!("Failed to open ECW file `{name}`: {e}")))?
    } else {
        if bytes.len() < 89 {
            return Err(IoError::Format("ECW file is too small (header is less than 89 bytes)".into()));
        }
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("cadcraft_temp_{}.ecw", std::process::id()));
        std::fs::write(&temp_file, bytes).map_err(|e| IoError::Format(format!("Failed to buffer ECW: {e}")))?;
        let res = EcwImage::open(&temp_file).map_err(|e| IoError::Format(format!("Failed to parse ECW: {e}")));
        let _ = std::fs::remove_file(&temp_file);
        res?
    };

    let info = img.info();

    // Check dimensions
    if info.width == 0 || info.height == 0 {
        return Err(IoError::Format("ECW image has zero dimensions".into()));
    }

    let cell_x = if info.cell_size_x.abs() > 1e-12 { info.cell_size_x.abs() } else { 1.0 };
    let cell_y = if info.cell_size_y.abs() > 1e-12 { info.cell_size_y.abs() } else { 1.0 };

    let w_m = (info.width as f64) * cell_x;
    let h_m = (info.height as f64) * cell_y;

    // Origin (top-left in standard ECW geospatial coordinates)
    let x0 = info.origin_x;
    let y0 = info.origin_y;

    // Corner coordinates in world drawing space:
    // Top-Left: (x0, y0)
    // Top-Right: (x0 + w_m, y0)
    // Bottom-Right: (x0 + w_m, y0 - h_m)
    // Bottom-Left: (x0, y0 - h_m)
    let ll_x = x0;
    let ll_y = y0 - h_m;
    let lr_x = x0 + w_m;
    let lr_y = y0 - h_m;
    let ur_x = x0 + w_m;
    let ur_y = y0;
    let ul_x = x0;
    let ul_y = y0;

    // Configure Coordinate Reference System (CRS)
    let crs_str = parse_crs(&info.projection, &info.datum);
    d.header.set("GEO_CRS", HVal::Str(crs_str.clone()));
    d.header.set("CRS", HVal::Str(crs_str.clone()));
    d.header.set("INSUNITS", HVal::Int(4)); // Meters
    d.header.set("MEASUREMENT", HVal::Int(1)); // Metric
    d.header.set("PDMODE", HVal::Int(35)); // Point marker style (circle + cross)
    d.header.set("PDSIZE", HVal::Real(0.0)); // Auto-scaling point markers
    d.header.set("EXTMIN", HVal::Point(Vec3::new(ll_x, ll_y, 0.0)));
    d.header.set("EXTMAX", HVal::Point(Vec3::new(ur_x, ur_y, 0.0)));

    // Ensure layers exist
    ensure_layer(d, "ORTHO_IMAGE", Color::Index(4)); // Cyan
    ensure_layer(d, "ORTHO_BOUNDARY", Color::Index(3)); // Green
    ensure_layer(d, "ORTHO_INFO", Color::Index(2)); // Yellow

    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("ORTHO");

    // 1. Add AutoCAD Image Entity (on layer ORTHO_IMAGE)
    // AutoCAD IMAGE insertion point is the lower-left corner (DXF group 10)
    let img_common = Common { layer: "ORTHO_IMAGE".to_string(), ..Default::default() };

    let (boundary_vertices, clip_vertices) = if let Some(poly) = detect_survey_boundary(&img) {
        let world_pts = poly
            .iter()
            .map(|pt| {
                let wx = x0 + pt.x * cell_x;
                let wy = y0 - pt.y * cell_y;
                PolyVertex::new(Vec2::new(wx, wy))
            })
            .collect();
        (world_pts, poly)
    } else {
        (
            vec![
                PolyVertex::new(Vec2::new(ll_x, ll_y)),
                PolyVertex::new(Vec2::new(lr_x, lr_y)),
                PolyVertex::new(Vec2::new(ur_x, ur_y)),
                PolyVertex::new(Vec2::new(ul_x, ul_y)),
            ],
            Vec::new(),
        )
    };

    let clipping = !clip_vertices.is_empty();
    let image = Image {
        insert: Vec3::new(ll_x, ll_y, 0.0),
        u: Vec3::new(cell_x, 0.0, 0.0),
        v: Vec3::new(0.0, cell_y, 0.0),
        size: Vec2::new(info.width as f64, info.height as f64),
        path: name.to_string(),
        name: format!("ECW_{stem}"),
        clip: clip_vertices,
        clipping,
        display: if clipping { 15 } else { 7 },
        brightness: 50,
        contrast: 50,
        fade: 0,
        pixel_size: Vec2::new(cell_x, cell_y),
        resolution_units: 0,
    };
    let _ = d.add(&Space::Model, img_common, EntityKind::Image(image));

    // 2. Add Orthophoto Ground Footprint Polyline (on layer ORTHO_BOUNDARY)
    let bnd_common = Common { layer: "ORTHO_BOUNDARY".to_string(), ..Default::default() };

    let boundary = LwPolyline { vertices: boundary_vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: true };
    let _ = d.add(&Space::Model, bnd_common, EntityKind::LwPolyline(boundary));

    // 3. Add Corner Markers and Coordinate Callouts (on layer ORTHO_INFO)
    let info_common = Common { layer: "ORTHO_INFO".to_string(), ..Default::default() };

    let corner_pts = [(ll_x, ll_y), (lr_x, lr_y), (ur_x, ur_y), (ul_x, ul_y)];
    for &(cx, cy) in &corner_pts {
        let _ = d.add(&Space::Model, info_common.clone(), EntityKind::Point(Point { p: Vec3::new(cx, cy, 0.0), angle: 0.0 }));
    }

    let text_h = (w_m.max(h_m) / 100.0).clamp(2.0, 45.0);
    let offset = text_h * 0.8;

    // Corner text callouts
    let callouts = [
        (ul_x, ul_y + offset, format!("TL: E {ul_x:.2}, N {ul_y:.2}"), HAlign::Left, VAlign::Baseline),
        (ur_x, ur_y + offset, format!("TR: E {ur_x:.2}, N {ur_y:.2}"), HAlign::Right, VAlign::Baseline),
        (lr_x, lr_y - offset, format!("BR: E {lr_x:.2}, N {lr_y:.2}"), HAlign::Right, VAlign::Top),
        (ll_x, ll_y - offset, format!("BL: E {ll_x:.2}, N {ll_y:.2}"), HAlign::Left, VAlign::Top),
    ];

    for (tx, ty, val, halign, valign) in callouts {
        let _ = d.add(
            &Space::Model,
            info_common.clone(),
            EntityKind::Text(Text {
                insert: Vec3::new(tx, ty, 0.0),
                align_pt: Some(Vec3::new(tx, ty, 0.0)),
                height: text_h,
                value: val,
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "STANDARD".to_string(),
                halign,
                valign,
            }),
        );
    }

    // 4. Add Information Header Banner Annotation
    let info_title = format!(
        "AERIAL ORTHOPHOTO: {} | {}x{} px ({:.1} MP) | GSD: {:.3} m/px | CRS: {} ({})",
        stem,
        info.width,
        info.height,
        (info.width as f64 * info.height as f64) / 1_000_000.0,
        cell_x,
        crs_str,
        info.datum
    );
    let title_x = (ll_x + lr_x) / 2.0;
    let title_y = ur_y + offset * 2.5;

    let _ = d.add(
        &Space::Model,
        info_common,
        EntityKind::Text(Text {
            insert: Vec3::new(title_x, title_y, 0.0),
            align_pt: Some(Vec3::new(title_x, title_y, 0.0)),
            height: text_h * 1.3,
            value: info_title,
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "STANDARD".to_string(),
            halign: HAlign::Center,
            valign: VAlign::Baseline,
        }),
    );

    Ok(1)
}

fn ensure_layer(d: &mut Drawing, name: &str, color: Color) {
    if !d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(name)) {
        d.layers.push(Layer { name: name.to_string(), color, ..Default::default() });
    }
}

/// Parse ECW projection and datum into standard MonarchCAD CRS (e.g. `UTM_43N`).
fn parse_crs(projection: &str, datum: &str) -> String {
    let proj_upper = projection.trim().to_ascii_uppercase();
    let dat_upper = datum.trim().to_ascii_uppercase();

    // Look for UTM zone specification: NUTM43, SUTM43, UTM43N, ZONE43N, etc.
    if let Some(rest) = proj_upper.strip_prefix("NUTM")
        && let Ok(zone) = rest.parse::<u8>()
    {
        return format!("UTM_{zone}N");
    }
    if let Some(rest) = proj_upper.strip_prefix("SUTM")
        && let Ok(zone) = rest.parse::<u8>()
    {
        return format!("UTM_{zone}S");
    }
    if proj_upper.contains("UTM") {
        // Find digits in projection string
        let digits: String = proj_upper.chars().filter(|c| c.is_ascii_digit()).collect();
        if let Ok(zone) = digits.parse::<u8>()
            && (1..=60).contains(&zone)
        {
            let is_south = proj_upper.contains('S') && !proj_upper.contains('N');
            let hem = if is_south { "S" } else { "N" };
            return format!("UTM_{zone}{hem}");
        }
    }

    if proj_upper == "GEODETIC" || dat_upper.contains("WGS84") || proj_upper.contains("WGS84") {
        return "WGS84".to_string();
    }

    if proj_upper.is_empty() { "UTM_43N".to_string() } else { proj_upper }
}

/// Detect and trace the outer survey boundary polygon from an ECW overview.
/// Returns polygon vertices in full ECW pixel coordinates (origin top-left, y down).
pub fn detect_survey_boundary(img: &EcwImage) -> Option<Vec<Vec2>> {
    let info = img.info();
    if info.width == 0 || info.height == 0 {
        return None;
    }

    let aspect = (info.width as f64) / (info.height as f64);
    let out_w = 512u32.min(info.width);
    let out_h = ((f64::from(out_w) / aspect).round() as u32).clamp(16, info.height);

    let win = crate::ecw::Window { x0: 0, y0: 0, x1: info.width - 1, y1: info.height - 1, out_width: out_w, out_height: out_h };

    let mut reader = img.rows(win, 0, out_h).ok()?;
    let channels = info.channels();
    let mut row_buf = vec![0u8; (out_w as usize) * channels];
    let w = out_w as usize;
    let h = out_h as usize;
    let mut raw_rgb = Vec::with_capacity(w * h * 3);

    for _ in 0..out_h {
        if reader.read_row(&mut row_buf).is_err() {
            return None;
        }
        if channels >= 3 {
            for rgb in row_buf.chunks_exact(channels) {
                raw_rgb.push(rgb[0]);
                raw_rgb.push(rgb[1]);
                raw_rgb.push(rgb[2]);
            }
        } else if channels == 1 {
            for &grey in &row_buf {
                raw_rgb.push(grey);
                raw_rgb.push(grey);
                raw_rgb.push(grey);
            }
        }
    }

    if raw_rgb.len() != w * h * 3 {
        return None;
    }

    // 1. Identify candidate collar pixels (near-white background padding)
    let is_candidate = |r: u8, g: u8, b: u8| -> bool {
        let min_c = r.min(g).min(b);
        let max_c = r.max(g).max(b);
        let delta = max_c - min_c;
        (min_c >= 215 && delta <= 12) || (r >= 250 && g >= 250 && b >= 250) || (r == 0 && g == 0 && b == 0)
    };

    let mut is_collar = vec![false; w * h];
    let mut queue = std::collections::VecDeque::new();

    for x in 0..w {
        let idx_top = x;
        if is_candidate(raw_rgb[idx_top * 3], raw_rgb[idx_top * 3 + 1], raw_rgb[idx_top * 3 + 2]) {
            is_collar[idx_top] = true;
            queue.push_back((x, 0));
        }
        let idx_bot = (h - 1) * w + x;
        if is_candidate(raw_rgb[idx_bot * 3], raw_rgb[idx_bot * 3 + 1], raw_rgb[idx_bot * 3 + 2]) {
            is_collar[idx_bot] = true;
            queue.push_back((x, h - 1));
        }
    }
    for y in 0..h {
        let idx_left = y * w;
        if !is_collar[idx_left] && is_candidate(raw_rgb[idx_left * 3], raw_rgb[idx_left * 3 + 1], raw_rgb[idx_left * 3 + 2]) {
            is_collar[idx_left] = true;
            queue.push_back((0, y));
        }
        let idx_right = y * w + (w - 1);
        if !is_collar[idx_right] && is_candidate(raw_rgb[idx_right * 3], raw_rgb[idx_right * 3 + 1], raw_rgb[idx_right * 3 + 2]) {
            is_collar[idx_right] = true;
            queue.push_back((w - 1, y));
        }
    }

    // BFS Flood Fill from outer edges
    while let Some((cx, cy)) = queue.pop_front() {
        let neighbors = [(cx.wrapping_sub(1), cy), (cx + 1, cy), (cx, cy.wrapping_sub(1)), (cx, cy + 1)];
        for (nx, ny) in neighbors {
            if nx < w && ny < h {
                let nidx = ny * w + nx;
                if !is_collar[nidx] && is_candidate(raw_rgb[nidx * 3], raw_rgb[nidx * 3 + 1], raw_rgb[nidx * 3 + 2]) {
                    is_collar[nidx] = true;
                    queue.push_back((nx, ny));
                }
            }
        }
    }

    let collar_count = is_collar.iter().filter(|&&c| c).count();
    // Only clip if a substantial collar exists (> 2% of the total bounding box)
    if collar_count < (w * h) / 50 {
        return None;
    }

    // 2. Find start pixel for boundary tracing (topmost, then leftmost non-collar pixel)
    let mut start = None;
    for y in 0..h {
        for x in 0..w {
            if !is_collar[y * w + x] {
                start = Some((x, y));
                break;
            }
        }
        if start.is_some() {
            break;
        }
    }

    let (sx, sy) = start?;

    // 3. Moore-Neighbor boundary tracing (clockwise from North)
    let dirs = [(0isize, -1isize), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1)];
    let mut boundary = Vec::new();
    let mut curr = (sx, sy);
    let mut backtrack = 0usize;
    boundary.push(Vec2::new(sx as f64, sy as f64));

    for _ in 0..10_000 {
        let mut found = false;
        for step in 0..8 {
            let d_idx = (backtrack + step) % 8;
            let (dx, dy) = dirs[d_idx];
            let nx = curr.0 as isize + dx;
            let ny = curr.1 as isize + dy;
            if nx >= 0 && nx < w as isize && ny >= 0 && ny < h as isize {
                let unx = nx as usize;
                let uny = ny as usize;
                if !is_collar[uny * w + unx] {
                    curr = (unx, uny);
                    backtrack = (d_idx + 5) % 8;
                    found = true;
                    break;
                }
            }
        }
        if !found || curr == (sx, sy) {
            break;
        }
        boundary.push(Vec2::new(curr.0 as f64, curr.1 as f64));
    }

    if boundary.len() < 3 {
        return None;
    }

    // 4. Simplify boundary with Ramer-Douglas-Peucker (RDP)
    let simplified = rdp_simplify(&boundary, 2.5);
    if simplified.len() < 3 {
        return None;
    }

    // 5. Scale from overview resolution to full ECW image coordinates
    let scale_x = (info.width as f64) / (out_w as f64);
    let scale_y = (info.height as f64) / (out_h as f64);
    let full_coords = simplified.into_iter().map(|pt| Vec2::new(pt.x * scale_x, pt.y * scale_y)).collect();

    Some(full_coords)
}

fn rdp_simplify(pts: &[Vec2], eps: f64) -> Vec<Vec2> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let p1 = pts.first().copied().unwrap_or(Vec2::ZERO);
    let p2 = pts.last().copied().unwrap_or(Vec2::ZERO);
    let mut max_d = 0.0;
    let mut idx = 0;

    for (i, &pt) in pts.iter().enumerate().skip(1).take(pts.len().saturating_sub(2)) {
        let d = perpendicular_distance(pt, p1, p2);
        if d > max_d {
            max_d = d;
            idx = i;
        }
    }

    if max_d > eps && idx > 0 && idx < pts.len() {
        let mut left = rdp_simplify(&pts[..=idx], eps);
        let right = rdp_simplify(&pts[idx..], eps);
        left.pop();
        left.extend(right);
        left
    } else {
        vec![p1, p2]
    }
}

fn perpendicular_distance(pt: Vec2, p1: Vec2, p2: Vec2) -> f64 {
    let dx = p2.x - p1.x;
    let dy = p2.y - p1.y;
    let len = dx.hypot(dy);
    if len < 1e-9 { (pt - p1).len() } else { ((dy * pt.x - dx * pt.y + p2.x * p1.y - p2.y * p1.x).abs()) / len }
}
