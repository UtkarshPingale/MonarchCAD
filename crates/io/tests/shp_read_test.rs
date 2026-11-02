use std::fs;
use std::path::Path;

#[test]
fn test_read_bridge_end_shp() {
    let path = r"C:\Users\uttkarsh.pingle\Downloads\Inside_Only\Inside_Only\Bridge End.shp";
    if !Path::new(path).exists() {
        return;
    }
    let bytes = fs::read(path).expect("read Bridge End.shp");
    let d = cadcraft_io::read(&bytes, "Bridge End.shp").expect("parse Bridge End.shp");
    assert!(!d.model.is_empty(), "model space should contain entities");
    assert!(d.layers.iter().any(|l| l.name == "Bridge End"), "should have Bridge End layer");
}

#[test]
fn test_read_yard_lines_shp() {
    let path = r"C:\Users\uttkarsh.pingle\Downloads\Inside_Only\Inside_Only\Yard Lines.shp";
    if !Path::new(path).exists() {
        return;
    }
    let bytes = fs::read(path).expect("read Yard Lines.shp");
    let d = cadcraft_io::read(&bytes, "Yard Lines.shp").expect("parse Yard Lines.shp");
    assert_eq!(d.model.len(), 183, "Yard Lines should have 183 polylines");
}

#[test]
fn test_import_multiple_shp_layers() {
    let folder = Path::new(r"C:\Users\uttkarsh.pingle\Downloads\Inside_Only\Inside_Only");
    if !folder.exists() {
        return;
    }
    let be_path = folder.join("Bridge End.shp");
    let yl_path = folder.join("Yard Lines.shp");
    if !be_path.exists() || !yl_path.exists() {
        return;
    }
    let be_bytes = fs::read(&be_path).expect("read Bridge End");
    let mut d = cadcraft_io::read(&be_bytes, "Bridge End.shp").expect("parse Bridge End");

    let yl_bytes = fs::read(&yl_path).expect("read Yard Lines");
    let imported = cadcraft_io::import_shp(&mut d, &yl_bytes, "Yard Lines.shp").expect("import Yard Lines");
    assert_eq!(imported, 183);

    assert!(d.layers.iter().any(|l| l.name == "Bridge End"));
    assert!(d.layers.iter().any(|l| l.name == "Yard Lines"));
}
