use cadcraft_engine::Session;
use cadcraft_ui_egui::{CadApp, Services, menus};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn test_insert_menu_has_ecw_and_shapefile() {
    let app = CadApp::new(Session::new(), Services::default());
    let tree = menus::tree(&app);
    let insert_menu = tree.iter().find(|(name, _)| name == "Insert").expect("Insert menu must exist");

    let labels: Vec<&str> = insert_menu
        .1
        .iter()
        .map(|e| match e {
            menus::Entry::Item { label, .. } => label.as_str(),
            menus::Entry::Sub { label, .. } => label.as_str(),
        })
        .collect();

    assert!(labels.contains(&"Raster Image (ECW)..."), "Insert menu must contain Raster Image (ECW)...: {labels:?}");
    assert!(labels.contains(&"Shapefile (SHP)..."), "Insert menu must contain Shapefile (SHP)...: {labels:?}");
}

#[test]
fn test_insert_picker_routing() {
    let shp_picked = Rc::new(RefCell::new(false));
    let raster_picked = Rc::new(RefCell::new(false));

    let shp_flag = shp_picked.clone();
    let raster_flag = raster_picked.clone();

    let services = Services {
        pick_shp: Some(Box::new(move || {
            *shp_flag.borrow_mut() = true;
            None
        })),
        pick_raster: Some(Box::new(move || {
            *raster_flag.borrow_mut() = true;
            None
        })),
        ..Services::default()
    };

    let mut app = CadApp::new(Session::new(), services);

    // Test clicking Shapefile (SHP)...
    menus::activate(&mut app, "mapimport");
    assert!(*shp_picked.borrow(), "mapimport should invoke pick_shp");

    // Test clicking Raster Image (ECW)...
    menus::activate(&mut app, "imageattach");
    assert!(*raster_picked.borrow(), "imageattach should invoke pick_raster");
}
