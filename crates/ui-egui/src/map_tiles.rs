//! High-resolution Georeferenced Background Map & Satellite Tile renderer for MonarchCAD.
//!
//! Provides georeferenced satellite imagery (aerial view), hybrid satellite+labels,
//! and street maps (OpenStreetMap) seamlessly rendered under CAD vector geometry.
//! Supports automatic coordinate reprojection for UTM (Universal Transverse Mercator,
//! e.g. Zone 44N/43N for India and global surveys), Web Mercator (EPSG:3857),
//! and WGS84 Geographic Degrees (EPSG:4326).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cadcraft_geom::{Bounds2, Vec2};
use egui::{Color32, Pos2, TextureHandle, TextureOptions, pos2};

/// Background map imagery mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum MapMode {
    #[default]
    Satellite,
    Hybrid,
    Street,
}

impl MapMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            MapMode::Satellite => "satellite",
            MapMode::Hybrid => "hybrid",
            MapMode::Street => "osm",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            MapMode::Satellite => "Satellite (Aerial)",
            MapMode::Hybrid => "Hybrid (Satellite + Labels)",
            MapMode::Street => "Street Map (OpenStreetMap)",
        }
    }
}

/// Coordinate Reference System (CRS) for drawing geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordSystem {
    /// Lat/Lon degrees (EPSG:4326).
    Wgs84,
    /// Web Mercator meters (EPSG:3857).
    WebMercator,
    /// Universal Transverse Mercator (UTM, e.g. Zone 44N, 43N).
    Utm { zone: u8, north: bool },
}

impl CoordSystem {
    pub fn display_name(&self) -> String {
        match self {
            CoordSystem::Wgs84 => "WGS 84 (Degrees)".to_string(),
            CoordSystem::WebMercator => "Web Mercator (EPSG:3857)".to_string(),
            CoordSystem::Utm { zone, north } => {
                let hem = if *north { "N" } else { "S" };
                format!("UTM Zone {zone}{hem}")
            }
        }
    }

    pub fn short_name(&self) -> String {
        match self {
            CoordSystem::Wgs84 => "WGS84".to_string(),
            CoordSystem::WebMercator => "EPSG:3857".to_string(),
            CoordSystem::Utm { zone, north } => {
                let hem = if *north { "N" } else { "S" };
                format!("UTM {zone}{hem}")
            }
        }
    }
}

/// Slippy map tile coordinate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileCoord {
    pub mode: MapMode,
    pub z: u8,
    pub x: i32,
    pub y: i32,
}

/// Decoded tile ready for GPU texture upload.
pub struct LoadedTile {
    pub coord: TileCoord,
    pub image: egui::ColorImage,
}

pub fn lon2tile_x(lon_deg: f64, zoom: u8) -> i32 {
    let n = (1u32 << zoom) as f64;
    let x = (lon_deg + 180.0) / 360.0 * n;
    x.floor() as i32
}

pub fn lat2tile_y(lat_deg: f64, zoom: u8) -> i32 {
    let lat_rad = lat_deg.clamp(-85.0511, 85.0511).to_radians();
    let n = (1u32 << zoom) as f64;
    let y = (1.0 - (lat_rad.tan().asinh() / std::f64::consts::PI)) / 2.0 * n;
    y.floor() as i32
}

pub fn tile2lon(x: i32, zoom: u8) -> f64 {
    let n = (1u32 << zoom) as f64;
    (x as f64) / n * 360.0 - 180.0
}

pub fn tile2lat(y: i32, zoom: u8) -> f64 {
    let n = (1u32 << zoom) as f64;
    let sinh_val = (std::f64::consts::PI * (1.0 - 2.0 * (y as f64) / n)).sinh();
    sinh_val.atan().to_degrees()
}

pub fn lon2merc_x(lon_deg: f64) -> f64 {
    lon_deg * 20037508.34 / 180.0
}

pub fn lat2merc_y(lat_deg: f64) -> f64 {
    let lat_rad = lat_deg.clamp(-85.0511, 85.0511).to_radians();
    lat_rad.tan().asinh() * 20037508.34 / std::f64::consts::PI
}

pub fn merc2lon(x: f64) -> f64 {
    x * 180.0 / 20037508.34
}

pub fn merc2lat(y: f64) -> f64 {
    let rad = (y / 20037508.34 * std::f64::consts::PI).sinh().atan();
    rad.to_degrees()
}

/// Convert UTM Easting and Northing to WGS84 Latitude and Longitude (degrees).
pub fn utm_to_latlon(easting: f64, northing: f64, zone: u8, northern: bool) -> (f64, f64) {
    let a: f64 = 6378137.0; // WGS84 semi-major axis
    let f: f64 = 1.0 / 298.257223563; // flattening
    let b: f64 = a * (1.0 - f);
    let e: f64 = ((a * a - b * b) / (a * a)).sqrt();
    let e_prime_sq: f64 = (e * e) / (1.0 - e * e);
    let k0: f64 = 0.9996;

    let x: f64 = easting - 500000.0;
    let y: f64 = if northern { northing } else { northing - 10000000.0 };

    let m: f64 = y / k0;
    let mu: f64 = m / (a * (1.0 - e * e / 4.0 - 3.0 * e.powi(4) / 64.0 - 5.0 * e.powi(6) / 256.0));

    let e1: f64 = (1.0 - (1.0 - e * e).sqrt()) / (1.0 + (1.0 - e * e).sqrt());
    let j1: f64 = 3.0 * e1 / 2.0 - 27.0 * e1.powi(3) / 32.0;
    let j2: f64 = 21.0 * e1.powi(2) / 16.0 - 55.0 * e1.powi(4) / 32.0;
    let j3: f64 = 151.0 * e1.powi(3) / 96.0;
    let j4: f64 = 1097.0 * e1.powi(4) / 512.0;

    let fp: f64 = mu + j1 * (2.0 * mu).sin() + j2 * (4.0 * mu).sin() + j3 * (6.0 * mu).sin() + j4 * (8.0 * mu).sin();

    let c1: f64 = e_prime_sq * fp.cos().powi(2);
    let t1: f64 = fp.tan().powi(2);
    let sin_fp: f64 = fp.sin();
    let denom: f64 = (1.0 - e * e * sin_fp * sin_fp).sqrt();
    let r1: f64 = a * (1.0 - e * e) / (denom * denom * denom);
    let n1: f64 = a / denom;

    let d: f64 = x / (n1 * k0);

    let lat: f64 = fp
        - (n1 * fp.tan() / r1)
            * (d * d / 2.0 - (5.0 + 3.0 * t1 + 10.0 * c1 - 4.0 * c1 * c1 - 9.0 * e_prime_sq) * d.powi(4) / 24.0
                + (61.0 + 90.0 * t1 + 298.0 * c1 + 45.0 * t1 * t1 - 252.0 * e_prime_sq - 3.0 * c1 * c1) * d.powi(6) / 720.0);

    let lon_origin: f64 = (zone as f64 - 1.0) * 6.0 - 180.0 + 3.0;
    let lon: f64 = lon_origin.to_radians()
        + (d - (1.0 + 2.0 * t1 + c1) * d.powi(3) / 6.0
            + (5.0 - 2.0 * c1 + 28.0 * t1 - 3.0 * c1 * c1 + 8.0 * e_prime_sq + 24.0 * t1 * t1) * d.powi(5) / 120.0)
            / fp.cos();

    (lat.to_degrees(), lon.to_degrees())
}

/// Convert WGS84 Latitude and Longitude (degrees) to UTM Easting and Northing.
pub fn latlon_to_utm(lat_deg: f64, lon_deg: f64, zone: u8, northern: bool) -> (f64, f64) {
    let a: f64 = 6378137.0;
    let f: f64 = 1.0 / 298.257223563;
    let e: f64 = (2.0 * f - f * f).sqrt();
    let e_prime_sq: f64 = (e * e) / (1.0 - e * e);
    let k0: f64 = 0.9996;

    let lat_rad: f64 = lat_deg.to_radians();
    let lon_rad: f64 = lon_deg.to_radians();
    let lon_origin: f64 = (zone as f64 - 1.0) * 6.0 - 180.0 + 3.0;
    let lon_origin_rad: f64 = lon_origin.to_radians();

    let sin_lat: f64 = lat_rad.sin();
    let n: f64 = a / (1.0 - e * e * sin_lat * sin_lat).sqrt();
    let t = lat_rad.tan().powi(2);
    let c = e_prime_sq * lat_rad.cos().powi(2);
    let a_val = lat_rad.cos() * (lon_rad - lon_origin_rad);

    let m = a
        * ((1.0 - e * e / 4.0 - 3.0 * e.powi(4) / 64.0 - 5.0 * e.powi(6) / 256.0) * lat_rad
            - (3.0 * e * e / 8.0 + 3.0 * e.powi(4) / 32.0 + 45.0 * e.powi(6) / 1024.0) * (2.0 * lat_rad).sin()
            + (15.0 * e.powi(4) / 256.0 + 45.0 * e.powi(6) / 1024.0) * (4.0 * lat_rad).sin()
            - (35.0 * e.powi(6) / 3072.0) * (6.0 * lat_rad).sin());

    let easting =
        k0 * n * (a_val + (1.0 - t + c) * a_val.powi(3) / 6.0 + (5.0 - 18.0 * t + t * t + 72.0 * c - 58.0 * e_prime_sq) * a_val.powi(5) / 120.0)
            + 500000.0;

    let northing_raw = k0
        * (m + n
            * lat_rad.tan()
            * (a_val * a_val / 2.0
                + (5.0 - t + 9.0 * c + 4.0 * c * c) * a_val.powi(4) / 24.0
                + (61.0 - 58.0 * t + t * t + 600.0 * c - 330.0 * e_prime_sq) * a_val.powi(6) / 720.0));

    let northing = if northern { northing_raw } else { northing_raw + 10000000.0 };

    (easting, northing)
}

/// Project geographic latitude and longitude (degrees) into drawing world coordinates for the given CRS.
pub fn project_latlon(lat_deg: f64, lon_deg: f64, crs: CoordSystem) -> Vec2 {
    match crs {
        CoordSystem::Wgs84 => Vec2::new(lon_deg, lat_deg),
        CoordSystem::WebMercator => Vec2::new(lon2merc_x(lon_deg), lat2merc_y(lat_deg)),
        CoordSystem::Utm { zone, north } => {
            let (e, n) = latlon_to_utm(lat_deg, lon_deg, zone, north);
            Vec2::new(e, n)
        }
    }
}

/// Render a textured quad mesh evaluated at exact shared corner points.
///
/// Because adjacent tiles evaluate the exact same latitude and longitude vertices,
/// the screen coordinates are bit-for-bit identical, completely eliminating
/// any seams, gaps, or displacement steps between map tiles in all projections (UTM, Mercator, WGS84).
fn draw_tile_mesh(
    painter: &egui::Painter,
    texture_id: egui::TextureId,
    p_tl: Pos2,
    p_tr: Pos2,
    p_br: Pos2,
    p_bl: Pos2,
    uv_min: Pos2,
    uv_max: Pos2,
    tint: Color32,
) {
    let mut mesh = egui::Mesh::with_texture(texture_id);
    let base = mesh.vertices.len() as u32;
    mesh.vertices.push(egui::epaint::Vertex { pos: p_tl, uv: pos2(uv_min.x, uv_min.y), color: tint });
    mesh.vertices.push(egui::epaint::Vertex { pos: p_tr, uv: pos2(uv_max.x, uv_min.y), color: tint });
    mesh.vertices.push(egui::epaint::Vertex { pos: p_br, uv: pos2(uv_max.x, uv_max.y), color: tint });
    mesh.vertices.push(egui::epaint::Vertex { pos: p_bl, uv: pos2(uv_min.x, uv_max.y), color: tint });
    mesh.add_triangle(base, base + 1, base + 2);
    mesh.add_triangle(base, base + 2, base + 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// Calculate appropriate map zoom level (0..19) based on viewport and screen size.
pub fn calculate_zoom(wb: &Bounds2, screen_width_px: f32, crs: CoordSystem) -> u8 {
    let dw = (wb.max.x - wb.min.x).abs();
    if dw <= 1e-9 {
        return 16;
    }

    let dw_deg = match crs {
        CoordSystem::Wgs84 => dw,
        CoordSystem::WebMercator => dw * 180.0 / 20037508.34,
        CoordSystem::Utm { .. } => dw / 100_000.0,
    };

    let w_px = (screen_width_px as f64).max(100.0);
    let ratio = (w_px * 360.0) / (256.0 * dw_deg.max(1e-6));
    let z = ratio.log2().round() as i32;
    z.clamp(1, 19) as u8
}

/// Fetch tile data from disk, local backend proxy, or direct fallback without spawning console windows.
fn fetch_and_decode_tile(tile: TileCoord, worker_dir: &Path) -> Option<egui::ColorImage> {
    let tile_name = format!("{}_{}_{}_{}.png", tile.mode.as_str(), tile.z, tile.x, tile.y);
    let tile_file = worker_dir.join(&tile_name);

    // 1. If file already exists in local disk cache, read it
    let bytes = if tile_file.exists() {
        std::fs::read(&tile_file).ok()?
    } else if let Ok(b) = std::fs::read(PathBuf::from("backend/tiles_cache").join(&tile_name)) {
        let _ = std::fs::write(&tile_file, &b);
        b
    } else {
        // 2. Fetch from backend service (127.0.0.1:8000 then 192.168.1.111:8000)
        let mut downloaded_bytes = None;
        for host in &["127.0.0.1", "192.168.1.111"] {
            let Ok(ip) = host.parse::<IpAddr>() else { continue };
            let addr = SocketAddr::new(ip, 8000);
            let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(6)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
            let req = format!(
                "GET /tiles/{}/{}/{}/{}.png HTTP/1.1\r\nHost: {host}:8000\r\nConnection: close\r\n\r\n",
                tile.mode.as_str(),
                tile.z,
                tile.x,
                tile.y
            );
            if stream.write_all(req.as_bytes()).is_err() {
                continue;
            }
            let mut resp = Vec::new();
            let _ = stream.read_to_end(&mut resp);
            if let Some(pos) = resp.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&resp[..pos]);
                if header.contains("200 OK") || header.starts_with("HTTP/1.1 200") || header.starts_with("HTTP/1.0 200") {
                    let body = resp[pos + 4..].to_vec();
                    if body.len() > 100 {
                        downloaded_bytes = Some(body);
                        break;
                    }
                }
            }
        }

        // 3. Fallback: direct download without visible CMD window
        let final_bytes = if let Some(b) = downloaded_bytes {
            b
        } else {
            let urls: Vec<String> = match tile.mode {
                MapMode::Satellite => vec![
                    format!("https://mt1.google.com/vt/lyrs=s&x={}&y={}&z={}", tile.x, tile.y, tile.z),
                    format!("https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{}/{}/{}", tile.z, tile.y, tile.x),
                ],
                MapMode::Hybrid => vec![
                    format!("https://mt1.google.com/vt/lyrs=y&x={}&y={}&z={}", tile.x, tile.y, tile.z),
                    format!("https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{}/{}/{}", tile.z, tile.y, tile.x),
                ],
                MapMode::Street => vec![format!("https://tile.openstreetmap.org/{}/{}/{}.png", tile.z, tile.x, tile.y)],
            };

            let mut fetched = None;
            for url in urls {
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    const CREATE_NO_WINDOW: u32 = 0x08000000;
                    let mut cmd = std::process::Command::new("curl.exe");
                    cmd.creation_flags(CREATE_NO_WINDOW);
                    cmd.args(["-s", "-A", "MonarchCAD/1.0 (Windows)", &url]);
                    if let Ok(out) = cmd.output()
                        && out.status.success()
                        && out.stdout.len() > 100
                    {
                        fetched = Some(out.stdout);
                        break;
                    }
                }
                #[cfg(not(windows))]
                {
                    let mut cmd = std::process::Command::new("curl");
                    cmd.args(["-s", "-A", "MonarchCAD/1.0", &url]);
                    if let Ok(out) = cmd.output()
                        && out.status.success()
                        && out.stdout.len() > 100
                    {
                        fetched = Some(out.stdout);
                        break;
                    }
                }
            }
            fetched?
        };

        // Cache locally for future instant loads
        let _ = std::fs::write(&tile_file, &final_bytes);
        final_bytes
    };

    // Decode universally (PNG or JPEG)
    let dyn_img = image::load_from_memory(&bytes).ok()?;
    let rgba = dyn_img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// Manages Georeferenced Background Map & Satellite imagery fetching, caching and rendering.
pub struct TileManager {
    pub show_map: bool,
    pub mode: MapMode,
    pub opacity: f32,
    pub manual_crs: Option<CoordSystem>,
    pub current_crs: CoordSystem,
    tiles_dir: PathBuf,
    textures: HashMap<TileCoord, TextureHandle>,
    in_flight: Arc<Mutex<HashSet<TileCoord>>>,
    visible: Arc<Mutex<HashSet<TileCoord>>>,
    req_tx: Sender<TileCoord>,
    done_rx: Receiver<LoadedTile>,
}

impl Default for TileManager {
    fn default() -> Self {
        Self::new(None)
    }
}

impl TileManager {
    pub fn new(config_dir: Option<PathBuf>) -> Self {
        let tiles_dir = match config_dir {
            Some(dir) => dir.join("tiles"),
            None => {
                if let Ok(appdata) = std::env::var("APPDATA") {
                    PathBuf::from(appdata).join("CADCraft").join("tiles")
                } else {
                    PathBuf::from(".cadcraft_tiles")
                }
            }
        };

        let _ = std::fs::create_dir_all(&tiles_dir);

        let (req_tx, req_rx) = mpsc::channel::<TileCoord>();
        let (done_tx, done_rx) = mpsc::channel::<LoadedTile>();
        let in_flight = Arc::new(Mutex::new(HashSet::new()));
        let visible = Arc::new(Mutex::new(HashSet::new()));
        let req_rx = Arc::new(Mutex::new(req_rx));

        // Spawn 4 background tile fetcher threads for fast parallel downloads
        for i in 0..4 {
            let worker_dir = tiles_dir.clone();
            let worker_flight = Arc::clone(&in_flight);
            let worker_visible = Arc::clone(&visible);
            let worker_rx = Arc::clone(&req_rx);
            let worker_done = done_tx.clone();

            thread::Builder::new()
                .name(format!("map-tile-fetcher-{i}"))
                .spawn(move || {
                    loop {
                        let tile = {
                            let Ok(rx) = worker_rx.lock() else { break };
                            match rx.recv() {
                                Ok(t) => t,
                                Err(_) => break,
                            }
                        };

                        // Check if tile is still in visible viewport
                        let is_visible = worker_visible.lock().map(|vis| vis.contains(&tile)).unwrap_or(true);
                        if !is_visible {
                            if let Ok(mut lock) = worker_flight.lock() {
                                lock.remove(&tile);
                            }
                            continue;
                        }

                        if let Some(color_img) = fetch_and_decode_tile(tile, &worker_dir) {
                            let _ = worker_done.send(LoadedTile { coord: tile, image: color_img });
                        }

                        if let Ok(mut lock) = worker_flight.lock() {
                            lock.remove(&tile);
                        }
                    }
                })
                .ok();
        }

        Self {
            show_map: false,
            mode: MapMode::Satellite,
            opacity: 0.90,
            manual_crs: None,
            current_crs: CoordSystem::Utm { zone: 44, north: true },
            tiles_dir,
            textures: HashMap::new(),
            in_flight,
            visible,
            req_tx,
            done_rx,
        }
    }

    pub fn tiles_dir(&self) -> &Path {
        &self.tiles_dir
    }

    /// Resolve active CRS from manual override, drawing header (`GEO_CRS`), title/path keywords, or coordinates range.
    pub fn resolve_crs(&self, doc_crs: Option<&str>, title_info: Option<&str>, bounds: &Bounds2) -> CoordSystem {
        if let Some(m) = self.manual_crs {
            return m;
        }
        if let Some(s) = doc_crs {
            let upper = s.to_ascii_uppercase();
            for z in 1..=60 {
                let n1 = format!("UTM_{z}N");
                let n2 = format!("UTM {z}N");
                let n3 = format!("ZONE_{z}N");
                let epsg_n = format!("326{z:02}");
                let u_raw = format!("UTM{z}");
                if upper.contains(&n1) || upper.contains(&n2) || upper.contains(&n3) || upper.contains(&epsg_n) || upper == u_raw {
                    return CoordSystem::Utm { zone: z, north: true };
                }
                let s1 = format!("UTM_{z}S");
                let s2 = format!("UTM {z}S");
                let s3 = format!("ZONE_{z}S");
                let epsg_s = format!("327{z:02}");
                if upper.contains(&s1) || upper.contains(&s2) || upper.contains(&s3) || upper.contains(&epsg_s) {
                    return CoordSystem::Utm { zone: z, north: false };
                }
            }
            if upper.contains("WGS84") || upper.contains("4326") || upper.contains("DEG") {
                return CoordSystem::Wgs84;
            }
            if upper.contains("3857") || upper.contains("MERC") {
                return CoordSystem::WebMercator;
            }
        }

        // Auto-detect based on drawing title, file path or project keywords
        if let Some(t) = title_info {
            let upper = t.to_ascii_uppercase();
            if upper.contains("AMBALA")
                || upper.contains("DELHI")
                || upper.contains("HARYANA")
                || upper.contains("PUNJAB")
                || upper.contains("CHANDIGARH")
                || upper.contains("LUDHIANA")
                || upper.contains("JALANDHAR")
                || upper.contains("AMRITSAR")
                || upper.contains("PANIPAT")
                || upper.contains("SONIPAT")
                || upper.contains("KARNAL")
                || upper.contains("KURUKSHETRA")
                || upper.contains("JAMMU")
                || upper.contains("KASHMIR")
                || upper.contains("J_K")
                || upper.contains("KATRA")
                || upper.contains("UDHAMPUR")
                || upper.contains("SRINAGAR")
                || upper.contains("HIMACHAL")
                || upper.contains("SHIMLA")
                || upper.contains("MUMBAI")
                || upper.contains("MAHARASHTRA")
                || upper.contains("PUNE")
                || upper.contains("SOLAPUR")
                || upper.contains("AKKALKOT")
                || upper.contains("AHMEDABAD")
                || upper.contains("GUJARAT")
                || upper.contains("JAIPUR")
                || upper.contains("RAJASTHAN")
                || upper.contains("BANGALORE")
                || upper.contains("BENGALURU")
                || upper.contains("KARNATAKA")
                || upper.contains("KERALA")
                || upper.contains("ZONE 43")
                || upper.contains("ZONE43")
                || upper.contains("UTM 43")
                || upper.contains("UTM43")
            {
                return CoordSystem::Utm { zone: 43, north: true };
            }
            if upper.contains("LUCKNOW")
                || upper.contains("UNNAO")
                || upper.contains("MANAKNAGAR")
                || upper.contains("KANPUR")
                || upper.contains("VARANASI")
                || upper.contains("ALLAHABAD")
                || upper.contains("PRAYAGRAJ")
                || upper.contains("AGRA")
                || upper.contains("MEERUT")
                || upper.contains("NOIDA")
                || upper.contains("GHAZIABAD")
                || upper.contains("MATHURA")
                || upper.contains("ALIGARH")
                || upper.contains("BAREILLY")
                || upper.contains("GORAKHPUR")
                || upper.contains("AYODHYA")
                || upper.contains("JHANSI")
                || upper.contains("GWALIOR")
                || upper.contains("NAGPUR")
                || upper.contains("CHHATTISGARH")
                || upper.contains("RAIPUR")
                || upper.contains("BILASPUR")
                || upper.contains("BIHAR")
                || upper.contains("PATNA")
                || upper.contains("HYDERABAD")
                || upper.contains("ANDHRA")
                || upper.contains("TELANGANA")
                || upper.contains("ODISHA")
                || upper.contains("BHUBANESWAR")
                || upper.contains("BHOPAL")
                || upper.contains("ZONE 44")
                || upper.contains("ZONE44")
                || upper.contains("UTM 44")
                || upper.contains("UTM44")
            {
                return CoordSystem::Utm { zone: 44, north: true };
            }
            if upper.contains("KOLKATA")
                || upper.contains("BENGAL")
                || upper.contains("CALCUTTA")
                || upper.contains("HOWRAH")
                || upper.contains("SEALDAH")
                || upper.contains("SILIGURI")
                || upper.contains("DURGAPUR")
                || upper.contains("ASANSOL")
                || upper.contains("DHANBAD")
                || upper.contains("JAMSHEDPUR")
                || upper.contains("ASSAM")
                || upper.contains("GUWAHATI")
                || upper.contains("JHARKHAND")
                || upper.contains("RANCHI")
                || upper.contains("ZONE 45")
                || upper.contains("ZONE45")
                || upper.contains("UTM 45")
                || upper.contains("UTM45")
            {
                return CoordSystem::Utm { zone: 45, north: true };
            }
        }

        // Auto-detection based on viewport / drawing extents
        let cx = (bounds.min.x + bounds.max.x) * 0.5;
        let cy = (bounds.min.y + bounds.max.y) * 0.5;
        if bounds.min.x.abs() <= 180.0 && bounds.max.x.abs() <= 180.0 && bounds.min.y.abs() <= 90.0 && bounds.max.y.abs() <= 90.0 {
            CoordSystem::Wgs84
        } else if (100_000.0..=900_000.0).contains(&cx.abs()) && (0.0..=10_000_000.0).contains(&cy.abs()) {
            // In North/West India (Northing >= 3,000,000 m: Delhi, Haryana, Punjab, Ambala corridor),
            // Easting in 600,000..800,000 is UTM Zone 43N (Zone 44N at this northing would land in Tibet).
            if (3_000_000.0..=3_800_000.0).contains(&cy) {
                CoordSystem::Utm { zone: 43, north: true }
            } else {
                CoordSystem::Utm { zone: 43, north: cy >= 0.0 }
            }
        } else {
            CoordSystem::WebMercator
        }
    }

    /// Render visible map / satellite tiles onto the canvas.
    pub fn draw(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        bounds: &Bounds2,
        to_screen: impl Fn(Vec2) -> Pos2,
        screen_width: f32,
        doc_crs: Option<&str>,
        title_info: Option<&str>,
    ) {
        if !self.show_map {
            return;
        }

        let crs = self.resolve_crs(doc_crs, title_info, bounds);
        self.current_crs = crs;

        // Receive newly decoded tiles from background workers and register textures
        while let Ok(loaded) = self.done_rx.try_recv() {
            let handle = ctx.load_texture(
                format!("{}_{}_{}_{}", loaded.coord.mode.as_str(), loaded.coord.z, loaded.coord.x, loaded.coord.y),
                loaded.image,
                TextureOptions::LINEAR,
            );
            self.textures.insert(loaded.coord, handle);
            ctx.request_repaint();
        }

        let (lon_min, lon_max, lat_min, lat_max) = match crs {
            CoordSystem::Wgs84 => (
                bounds.min.x.clamp(-180.0, 180.0),
                bounds.max.x.clamp(-180.0, 180.0),
                bounds.min.y.clamp(-85.0511, 85.0511),
                bounds.max.y.clamp(-85.0511, 85.0511),
            ),
            CoordSystem::WebMercator => (
                merc2lon(bounds.min.x).clamp(-180.0, 180.0),
                merc2lon(bounds.max.x).clamp(-180.0, 180.0),
                merc2lat(bounds.min.y).clamp(-85.0511, 85.0511),
                merc2lat(bounds.max.y).clamp(-85.0511, 85.0511),
            ),
            CoordSystem::Utm { zone, north } => {
                let (lat1, lon1) = utm_to_latlon(bounds.min.x, bounds.min.y, zone, north);
                let (lat2, lon2) = utm_to_latlon(bounds.max.x, bounds.min.y, zone, north);
                let (lat3, lon3) = utm_to_latlon(bounds.min.x, bounds.max.y, zone, north);
                let (lat4, lon4) = utm_to_latlon(bounds.max.x, bounds.max.y, zone, north);
                (
                    lon1.min(lon2).min(lon3).min(lon4).clamp(-180.0, 180.0),
                    lon1.max(lon2).max(lon3).max(lon4).clamp(-180.0, 180.0),
                    lat1.min(lat2).min(lat3).min(lat4).clamp(-85.0511, 85.0511),
                    lat1.max(lat2).max(lat3).max(lat4).clamp(-85.0511, 85.0511),
                )
            }
        };

        if (lon_max - lon_min).abs() <= 1e-9 || (lat_max - lat_min).abs() <= 1e-9 {
            return;
        }

        let z = calculate_zoom(bounds, screen_width, crs);
        let n = (1i32 << z).max(1);

        let mut x_start = (lon2tile_x(lon_min, z) - 1).clamp(0, n - 1);
        let mut x_end = (lon2tile_x(lon_max, z) + 1).clamp(0, n - 1);
        let mut y_start = (lat2tile_y(lat_max, z) - 1).clamp(0, n - 1);
        let mut y_end = (lat2tile_y(lat_min, z) + 1).clamp(0, n - 1);

        if x_start > x_end {
            std::mem::swap(&mut x_start, &mut x_end);
        }
        if y_start > y_end {
            std::mem::swap(&mut y_start, &mut y_end);
        }

        // Cap to at most 320 tiles per frame to maintain high frame rate
        let count_x = (x_end - x_start + 1).min(20);
        let count_y = (y_end - y_start + 1).min(16);
        x_end = x_start + count_x - 1;
        y_end = y_start + count_y - 1;

        let mode = self.mode;
        let mut current_visible = HashSet::new();
        for y in y_start..=y_end {
            for x in x_start..=x_end {
                current_visible.insert(TileCoord { mode, z, x, y });
            }
        }
        if let Ok(mut lock) = self.visible.lock() {
            *lock = current_visible;
        }

        let alpha = (self.opacity.clamp(0.0, 1.0) * 255.0) as u8;
        let tint = Color32::from_rgba_unmultiplied(255, 255, 255, alpha);

        for y in y_start..=y_end {
            for x in x_start..=x_end {
                let coord = TileCoord { mode, z, x, y };

                // Queue background load if texture not yet cached
                if !self.textures.contains_key(&coord) {
                    let inserted = self.in_flight.lock().map(|mut lock| lock.insert(coord)).unwrap_or(false);
                    if inserted {
                        let _ = self.req_tx.send(coord);
                    }
                }

                // Compute exact shared corner coordinates for this tile in lat/lon
                let tile_l = tile2lon(x, z);
                let tile_r = tile2lon(x + 1, z);
                let tile_t = tile2lat(y, z);
                let tile_b = tile2lat(y + 1, z);

                // Project 4 corners into world coordinates and then to screen
                let p_tl = to_screen(project_latlon(tile_t, tile_l, crs));
                let p_tr = to_screen(project_latlon(tile_t, tile_r, crs));
                let p_br = to_screen(project_latlon(tile_b, tile_r, crs));
                let p_bl = to_screen(project_latlon(tile_b, tile_l, crs));

                // 1. Draw exact tile if loaded
                if let Some(t) = self.textures.get(&coord) {
                    draw_tile_mesh(painter, t.id(), p_tl, p_tr, p_br, p_bl, pos2(0.0, 0.0), pos2(1.0, 1.0), tint);
                    continue;
                }

                // 2. Fallback to Any Loaded Ancestor tile (z - 1 down to 0) for seamless zooming without blanking
                let mut drawn = false;
                for az in (0..z).rev() {
                    let dz = z - az;
                    if dz > 10 {
                        break;
                    }
                    let ax = x >> dz;
                    let ay = y >> dz;
                    let anc_coord = TileCoord { mode, z: az, x: ax, y: ay };
                    if let Some(anc_tex) = self.textures.get(&anc_coord) {
                        let step = 1.0 / ((1i32 << dz) as f32);
                        let mask = (1i32 << dz) - 1;
                        let u_min = ((x & mask) as f32) * step;
                        let v_min = ((y & mask) as f32) * step;
                        draw_tile_mesh(painter, anc_tex.id(), p_tl, p_tr, p_br, p_bl, pos2(u_min, v_min), pos2(u_min + step, v_min + step), tint);
                        drawn = true;
                        break;
                    }
                }

                // 3. Fallback to Child tiles (z + 1) when zooming out
                if !drawn && z < 19 {
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let child_coord = TileCoord { mode, z: z + 1, x: (x << 1) + dx, y: (y << 1) + dy };
                        if let Some(child_tex) = self.textures.get(&child_coord) {
                            let cl = tile2lon((x << 1) + dx, z + 1);
                            let cr = tile2lon((x << 1) + dx + 1, z + 1);
                            let ct = tile2lat((y << 1) + dy, z + 1);
                            let cb = tile2lat((y << 1) + dy + 1, z + 1);

                            let cp_tl = to_screen(project_latlon(ct, cl, crs));
                            let cp_tr = to_screen(project_latlon(ct, cr, crs));
                            let cp_br = to_screen(project_latlon(cb, cr, crs));
                            let cp_bl = to_screen(project_latlon(cb, cl, crs));

                            draw_tile_mesh(painter, child_tex.id(), cp_tl, cp_tr, cp_br, cp_bl, pos2(0.0, 0.0), pos2(1.0, 1.0), tint);
                        }
                    }
                }
            }
        }
    }
}
