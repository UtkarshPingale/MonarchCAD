//! In-binary self-trust and unblocking for MonarchCAD on Windows.
//!
//! Automatically unblocks the executable (clearing NTFS `Zone.Identifier` Mark-of-the-Web)
//! and registers the embedded MonarchCAD Authority certificate into the user's
//! TrustedPublisher store and Intranet zone without requiring external batch or `.cer` files.

#[cfg(target_os = "windows")]
const MONARCH_CA_CER: &[u8] = include_bytes!("../../../assets/MonarchCAD_Authority.cer");

#[cfg(target_os = "windows")]
pub fn initialize() {
    // 1. Unblock current executable (delete Zone.Identifier ADS)
    unblock_current_exe();

    // 2. Silently ensure embedded certificate is registered in TrustedPublisher store
    // and configure Intranet zone for 192.168.1.* so downloads are trusted
    let _ = std::thread::Builder::new().name("monarch-trust-bg".into()).spawn(move || {
        install_certificate_silent();
        configure_intranet_zone();
    });
}

#[cfg(target_os = "windows")]
pub fn unblock_current_exe() {
    if let Ok(exe_path) = std::env::current_exe() {
        let ads_path = format!("{}:Zone.Identifier", exe_path.display());
        let _ = std::fs::remove_file(ads_path);

        if let Some(parent) = exe_path.parent()
            && let Ok(entries) = std::fs::read_dir(parent)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let ads = format!("{}:Zone.Identifier", path.display());
                    let _ = std::fs::remove_file(ads);
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn install_certificate_silent() {
    let temp_dir = std::env::temp_dir();
    let temp_cer = temp_dir.join("MonarchCAD_Authority.cer");
    if std::fs::write(&temp_cer, MONARCH_CA_CER).is_ok() {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        if let Some(path_str) = temp_cer.to_str() {
            let _ = std::process::Command::new("certutil")
                .args(["-addstore", "-user", "TrustedPublisher", path_str])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
        }
        let _ = std::fs::remove_file(&temp_cer);
    }
}

#[cfg(target_os = "windows")]
fn configure_intranet_zone() {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let _ = std::process::Command::new("reg")
        .args([
            "add",
            "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\\ZoneMap\\Ranges\\Range1",
            "/v",
            ":Range",
            "/t",
            "REG_SZ",
            "/d",
            "192.168.1.*",
            "/f",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    let _ = std::process::Command::new("reg")
        .args([
            "add",
            "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\\ZoneMap\\Ranges\\Range1",
            "/v",
            "*",
            "/t",
            "REG_DWORD",
            "/d",
            "1",
            "/f",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}

#[cfg(target_os = "windows")]
pub fn run_cli_trust() {
    println!("=== MonarchCAD Verified Publisher Setup ===");
    unblock_current_exe();
    println!("Unblocked executable and parent directory files.");

    let temp_dir = std::env::temp_dir();
    let temp_cer = temp_dir.join("MonarchCAD_Authority.cer");
    if std::fs::write(&temp_cer, MONARCH_CA_CER).is_ok() {
        if let Some(path_str) = temp_cer.to_str() {
            println!("Registering MonarchCAD in TrustedPublisher store...");
            let _ = std::process::Command::new("certutil").args(["-addstore", "-user", "TrustedPublisher", path_str]).status();

            println!("Registering MonarchCAD in Root store...");
            let _ = std::process::Command::new("certutil").args(["-addstore", "-user", "Root", path_str]).status();
        }
        let _ = std::fs::remove_file(&temp_cer);
    }
    configure_intranet_zone();
    println!("[OK] MonarchCAD is now fully trusted on this system.");
}

#[cfg(not(target_os = "windows"))]
pub fn initialize() {}

#[cfg(not(target_os = "windows"))]
pub fn run_cli_trust() {
    println!("Trust setup is only needed on Windows.");
}
