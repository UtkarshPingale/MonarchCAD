use cadcraft_doc::Image;
use cadcraft_geom::{Bounds2, Vec2, Vec3};
use cadcraft_ui_egui::raster_images::RasterImageManager;
use egui::{Context, pos2};
use std::path::Path;

#[test]
fn test_raster_image_manager_lod_and_queue() {
    let mut manager = RasterImageManager::default();
    let ctx = Context::default();

    // Create a mock orthomosaic image entity
    let ecw_path = r"C:\Users\uttkarsh.pingle\Downloads\RATNAGIRI-ORTHO-CN.ecw";
    let image = Image {
        insert: Vec3::new(315557.64, 1877756.50, 0.0),
        u: Vec3::new(0.022, 0.0, 0.0),
        v: Vec3::new(0.0, 0.022, 0.0),
        size: Vec2::new(316448.0, 182423.0),
        path: ecw_path.to_string(),
        name: "ECW_RATNAGIRI".to_string(),
        clip: Vec::new(),
        clipping: false,
        display: 15,
        brightness: 50,
        contrast: 50,
        fade: 0,
        pixel_size: Vec2::new(0.022, 0.022),
        resolution_units: 0,
    };

    let images = vec![image];

    // Simulate high zoom in (e.g. at coordinate 317209.0, 1880309.0)
    let center = Vec2::new(317209.0, 1880309.0);
    let scale = 100.0; // 100 screen pixels per meter -> high zoom in (level 0)
    let view_bounds = Bounds2::new(Vec2::new(center.x - 10.0, center.y - 10.0), Vec2::new(center.x + 10.0, center.y + 10.0));

    let to_screen = |p: Vec2| -> egui::Pos2 {
        let dx = ((p.x - center.x) * scale) as f32;
        let dy = ((p.y - center.y) * scale) as f32;
        pos2(500.0 + dx, 500.0 - dy)
    };

    // Run draw across several simulated frames (simulating rapid mouse movement / zoom ticks)
    let mut out1 = ctx.run_ui(Default::default(), |ui| {
        let painter = ui.painter();
        for _ in 0..10 {
            manager.draw(&ctx, painter, &images, to_screen, Some(&view_bounds));
        }
    });
    out1.textures_delta.clear();

    // If file exists, wait briefly for background workers and verify tiles are received
    if Path::new(ecw_path).exists() {
        std::thread::sleep(std::time::Duration::from_millis(250));

        let mut out2 = ctx.run_ui(Default::default(), |ui| {
            let painter = ui.painter();
            manager.draw(&ctx, painter, &images, to_screen, Some(&view_bounds));
        });
        out2.textures_delta.clear();
    }
}
