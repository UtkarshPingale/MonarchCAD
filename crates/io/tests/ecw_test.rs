use cadcraft_io::ecw::{EcwImage, Window};
use std::path::Path;

#[test]
fn test_open_ratnagiri_ecw() {
    let p = Path::new(r"C:\Users\uttkarsh.pingle\Downloads\RATNAGIRI-ORTHO-CN.ecw");
    if !p.exists() {
        return;
    }
    let img = EcwImage::open(p).expect("should open ECW image");
    let info = img.info();
    println!("ECW Info: {:?}", info);
    assert_eq!(info.width, 316448);
    assert_eq!(info.height, 182423);
    assert_eq!(info.bands, 4);

    let win = Window { x0: 0, y0: 0, x1: info.width - 1, y1: info.height - 1, out_width: 1024, out_height: 590 };
    println!("Requesting overview window: {:?}", win);
    let mut reader = img.rows(win, 0, win.out_height).expect("should create row reader");
    let mut row_buf = vec![0u8; win.out_width as usize * info.channels()];
    let w = win.out_width as usize;
    let h = win.out_height as usize;
    let mut raw_rgb = Vec::with_capacity(w * h * 3);
    for _ in 0..win.out_height {
        reader.read_row(&mut row_buf).expect("read row");
        for rgb in row_buf.chunks_exact(3) {
            raw_rgb.push(rgb[0]);
            raw_rgb.push(rgb[1]);
            raw_rgb.push(rgb[2]);
        }
    }

    let t0 = std::time::Instant::now();
    // 1. Identify candidate collar pixels (near-white or nodata black)
    let is_candidate = |r: u8, g: u8, b: u8| -> bool {
        let min_c = r.min(g).min(b);
        let max_c = r.max(g).max(b);
        let delta = max_c - min_c;
        (min_c >= 215 && delta <= 12) || (r >= 250 && g >= 250 && b >= 250) || (r == 0 && g == 0 && b == 0)
    };

    let mut is_collar = vec![false; w * h];
    let mut queue = std::collections::VecDeque::new();

    // Seed from border pixels
    for x in 0..w {
        // Top border (y = 0)
        let idx_top = x;
        if is_candidate(raw_rgb[idx_top * 3], raw_rgb[idx_top * 3 + 1], raw_rgb[idx_top * 3 + 2]) {
            is_collar[idx_top] = true;
            queue.push_back((x, 0));
        }
        // Bottom border (y = h - 1)
        let idx_bot = (h - 1) * w + x;
        if is_candidate(raw_rgb[idx_bot * 3], raw_rgb[idx_bot * 3 + 1], raw_rgb[idx_bot * 3 + 2]) {
            is_collar[idx_bot] = true;
            queue.push_back((x, h - 1));
        }
    }
    for y in 0..h {
        // Left border (x = 0)
        let idx_left = y * w;
        if !is_collar[idx_left] && is_candidate(raw_rgb[idx_left * 3], raw_rgb[idx_left * 3 + 1], raw_rgb[idx_left * 3 + 2]) {
            is_collar[idx_left] = true;
            queue.push_back((0, y));
        }
        // Right border (x = w - 1)
        let idx_right = y * w + (w - 1);
        if !is_collar[idx_right] && is_candidate(raw_rgb[idx_right * 3], raw_rgb[idx_right * 3 + 1], raw_rgb[idx_right * 3 + 2]) {
            is_collar[idx_right] = true;
            queue.push_back((w - 1, y));
        }
    }

    // BFS Flood Fill
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
    let elapsed = t0.elapsed();
    let collar_count = is_collar.iter().filter(|&&c| c).count();
    println!(
        "BFS collar detection completed in {:?}: {} / {} pixels ({:.1}%)",
        elapsed,
        collar_count,
        w * h,
        collar_count as f64 * 100.0 / (w * h) as f64
    );

    let mut all_pixels = Vec::with_capacity(w * h * 4);
    for i in 0..(w * h) {
        all_pixels.push(raw_rgb[i * 3]);
        all_pixels.push(raw_rgb[i * 3 + 1]);
        all_pixels.push(raw_rgb[i * 3 + 2]);
        all_pixels.push(if is_collar[i] { 0u8 } else { 255u8 });
    }

    let pixmap = tiny_skia::PixmapRef::from_bytes(&all_pixels, win.out_width, win.out_height).expect("pixmap");
    let png_bytes = pixmap.encode_png().expect("encode png");
    let _ = std::fs::create_dir_all("../../scratch");
    std::fs::write("../../scratch/ecw_overview.png", png_bytes).expect("write png");
    println!("Wrote scratch/ecw_overview.png!");
}

#[test]
fn test_cadcraft_io_read_ecw() {
    let path_str = r"C:\Users\uttkarsh.pingle\Downloads\RATNAGIRI-ORTHO-CN.ecw";
    let p = Path::new(path_str);
    if !p.exists() {
        return;
    }
    // We pass minimal header bytes (e.g. empty or small slice) because the path exists on disk
    let d = cadcraft_io::read(&[], path_str).expect("cadcraft_io::read should succeed for ECW");
    println!("Drawing layers: {:?}", d.layers.iter().map(|l| &l.name).collect::<Vec<_>>());
    assert!(d.layers.iter().any(|l| l.name == "ORTHO_IMAGE"));
    assert!(d.layers.iter().any(|l| l.name == "ORTHO_BOUNDARY"));
    assert!(d.layers.iter().any(|l| l.name == "ORTHO_INFO"));
    assert_eq!(d.header.get("GEO_CRS").and_then(|v| v.as_str()), Some("UTM_43N"));
    assert_eq!(d.header.get("CRS").and_then(|v| v.as_str()), Some("UTM_43N"));

    let entities: Vec<_> = d.model.iter().collect();
    println!("Total model entities created: {}", entities.len());
    let img_ent = entities.iter().find(|e| matches!(e.kind, cadcraft_doc::EntityKind::Image(_))).expect("image entity");
    if let cadcraft_doc::EntityKind::Image(ref img) = img_ent.kind {
        println!("Image clipping: {}, clip vertices: {}", img.clipping, img.clip.len());
        assert!(img.clipping);
        assert!(img.clip.len() > 10);
    }
    let poly_ent = entities.iter().find(|e| matches!(e.kind, cadcraft_doc::EntityKind::LwPolyline(_))).expect("polyline entity");
    if let cadcraft_doc::EntityKind::LwPolyline(ref poly) = poly_ent.kind {
        println!("Boundary polyline vertices: {}, closed: {}", poly.vertices.len(), poly.closed);
        assert!(poly.closed);
        assert!(poly.vertices.len() > 10);
    }
    assert!(entities.iter().any(|e| matches!(e.kind, cadcraft_doc::EntityKind::Text(_))));
}

#[test]
fn test_zoom_ratnagiri_ecw() {
    let p = Path::new(r"C:\Users\uttkarsh.pingle\Downloads\RATNAGIRI-ORTHO-CN.ecw");
    if !p.exists() {
        return;
    }
    let img = EcwImage::open(p).expect("open ECW");
    let info = img.info();

    let cell_x = info.cell_size_x.abs();
    let cell_y = info.cell_size_y.abs();

    // Target coordinate: 322437.6, 1878774.6
    let center_px = ((322437.6 - info.origin_x) / cell_x) as u32;
    let center_py = ((info.origin_y - 1878774.6) / cell_y) as u32;
    println!("Center pixel in ECW: ({center_px}, {center_py})");

    let half_w = 960u32;
    let half_h = 540u32;
    let x0 = center_px.saturating_sub(half_w);
    let x1 = (center_px + half_w).min(info.width - 1);
    let y0 = center_py.saturating_sub(half_h);
    let y1 = (center_py + half_h).min(info.height - 1);

    let win = Window { x0, y0, x1, y1, out_width: x1 - x0 + 1, out_height: y1 - y0 + 1 };
    println!("Zoomed window: {:?}", win);
    let t0 = std::time::Instant::now();
    let mut reader = img.rows(win, 0, win.out_height).expect("reader");
    let channels = info.channels();
    let mut row = vec![0u8; win.out_width as usize * channels];
    let mut rgba = Vec::with_capacity((win.out_width * win.out_height * 4) as usize);
    for _ in 0..win.out_height {
        reader.read_row(&mut row).expect("row");
        for rgb in row.chunks_exact(channels) {
            rgba.push(rgb[0]);
            rgba.push(rgb[1]);
            rgba.push(rgb[2]);
            rgba.push(255);
        }
    }
    println!("Decoded {}x{} in {:?}", win.out_width, win.out_height, t0.elapsed());
    let pixmap = tiny_skia::PixmapRef::from_bytes(&rgba, win.out_width, win.out_height).expect("pixmap");
    let png = pixmap.encode_png().expect("png");
    let _ = std::fs::create_dir_all("../../scratch");
    std::fs::write("../../scratch/ecw_zoomed.png", png).expect("write");
    println!("Wrote scratch/ecw_zoomed.png");
}

#[test]
fn test_parallel_ecw_tiles() {
    use std::sync::Arc;
    let p = Path::new(r"C:\Users\uttkarsh.pingle\Downloads\RATNAGIRI-ORTHO-CN.ecw");
    if !p.exists() {
        return;
    }
    let img = Arc::new(EcwImage::open(p).expect("open ECW"));
    let info = img.info();

    let cell_x = info.cell_size_x.abs();
    let cell_y = info.cell_size_y.abs();
    let center_px = ((322437.6 - info.origin_x) / cell_x) as u32;
    let center_py = ((info.origin_y - 1878774.6) / cell_y) as u32;

    // 4 adjacent tiles of 1024x1024
    let tiles = vec![
        (center_px.saturating_sub(1024), center_py.saturating_sub(1024)),
        (center_px, center_py.saturating_sub(1024)),
        (center_px.saturating_sub(1024), center_py),
        (center_px, center_py),
    ];

    let t0 = std::time::Instant::now();
    let mut handles = Vec::new();
    for (x0, y0) in tiles {
        let img_clone = Arc::clone(&img);
        let channels = info.channels();
        handles.push(std::thread::spawn(move || {
            let win = Window { x0, y0, x1: x0 + 1023, y1: y0 + 1023, out_width: 1024, out_height: 1024 };
            let mut reader = img_clone.rows(win, 0, 1024).expect("reader");
            let mut row = vec![0u8; 1024 * channels];
            let mut rgba = Vec::with_capacity(1024 * 1024 * 4);
            for _ in 0..1024 {
                reader.read_row(&mut row).expect("row");
                for rgb in row.chunks_exact(channels) {
                    rgba.push(rgb[0]);
                    rgba.push(rgb[1]);
                    rgba.push(rgb[2]);
                    rgba.push(255);
                }
            }
            rgba.len()
        }));
    }

    for h in handles {
        let len = h.join().expect("join");
        assert_eq!(len, 1024 * 1024 * 4);
    }
    let elapsed = t0.elapsed();
    println!(">>> Parallel 4x (1024x1024) tiles total time: {:?}", elapsed);
    println!(">>> Effective speed per tile: {:?}", elapsed / 4);
}
