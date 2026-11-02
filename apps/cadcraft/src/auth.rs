//! In-app authentication and license management for MonarchCAD.
//!
//! Prompts user for their registered email address, verifies it against
//! the backend (`/api/license/verify`), and stores it locally
//! in `monarch_license.json` so the user stays logged in.

use egui::{Color32, RichText, vec2};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Candidate backend servers for dual-network operation:
/// - 192.168.1.111:8000 (Local Office LAN network)
/// - 175.100.175.42:8000 (External WAN / Internet network)
/// - 127.0.0.1:8000 (Localhost fallback)
pub const DEFAULT_SERVER_HOST: &str = match option_env!("MONARCH_DEFAULT_SERVER") {
    Some(val) => val,
    None => "192.168.1.111:8000",
};

pub const FALLBACK_SERVERS: &[&str] = &["192.168.1.111:8000", "175.100.175.42:8000", "127.0.0.1:8000"];

pub fn normalize_server_addr(raw: &str, default_port: u16) -> String {
    let clean = raw.trim().trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/');
    if clean.is_empty() {
        return format!("127.0.0.1:{default_port}");
    }
    if clean.contains(':') { clean.to_string() } else { format!("{clean}:{default_port}") }
}

fn find_default_server(fallback: &str) -> String {
    if let Ok(env_host) = std::env::var("MONARCH_BACKEND_HOST") {
        let trimmed = env_host.trim();
        if !trimmed.is_empty() {
            return normalize_server_addr(trimmed, 8000);
        }
    }

    if let Ok(txt) = fs::read_to_string("monarch_server.txt") {
        let trimmed = txt.trim();
        if !trimmed.is_empty() {
            return normalize_server_addr(trimmed, 8000);
        }
    }

    if let Ok(exe_path) = std::env::current_exe()
        && let Some(parent) = exe_path.parent()
    {
        let txt_path = parent.join("monarch_server.txt");
        if let Ok(txt) = fs::read_to_string(txt_path) {
            let trimmed = txt.trim();
            if !trimmed.is_empty() {
                return normalize_server_addr(trimmed, 8000);
            }
        }
    }

    normalize_server_addr(fallback, 8000)
}

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct AuthState {
    pub email_input: String,
    pub server_input: String,
    pub active_email: Option<String>,
    pub active_server: String,
    pub is_authenticated: bool,
    pub is_verifying: bool,
    pub error_message: Option<String>,
    pub success_message: Option<String>,
    pub license_file: PathBuf,
    pub update_available: bool,
    pub update_version: String,
    pub update_notes: String,
    pub is_downloading_update: bool,
    pub update_status: Option<String>,
}

impl AuthState {
    pub fn new(config_dir: Option<&Path>, default_server: String) -> Self {
        let license_file = match config_dir {
            Some(dir) => dir.join("monarch_license.json"),
            None => PathBuf::from("monarch_license.json"),
        };

        let server = find_default_server(&default_server);

        let mut auth = Self {
            email_input: String::new(),
            server_input: server.clone(),
            active_email: None,
            active_server: server,
            is_authenticated: false,
            is_verifying: false,
            error_message: None,
            success_message: None,
            license_file,
            update_available: false,
            update_version: String::new(),
            update_notes: String::new(),
            is_downloading_update: false,
            update_status: None,
        };

        // Attempt to auto-login using saved license first to find active server
        auth.try_load_saved();

        // Check for updates from server
        auth.check_update();
        auth
    }

    pub fn check_update(&mut self) {
        if let Some((latest, notes)) = request_update_check(&self.active_server, APP_VERSION) {
            log::info!("MonarchCAD: system update available v{latest} ({notes})");
            self.update_available = true;
            self.update_version = latest;
            self.update_notes = notes;
        }
    }

    fn try_load_saved(&mut self) {
        let Ok(data) = fs::read_to_string(&self.license_file) else {
            return;
        };
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) else {
            return;
        };
        let Some(email_str) = val.get("email").and_then(|v| v.as_str()) else {
            return;
        };
        let email = email_str.trim().to_string();
        if email.is_empty() {
            return;
        }

        if let Some(srv_str) = val.get("server").and_then(|v| v.as_str()) {
            let srv = srv_str.trim();
            if !srv.is_empty() {
                self.server_input = srv.to_string();
                self.active_server = srv.to_string();
            }
        }

        self.email_input = email.clone();

        // Verify with backend
        match request_verification(&self.active_server, &email) {
            Ok((true, msg, connected_srv)) => {
                log::info!("MonarchCAD: license verified for {email} ({msg}) on server {connected_srv}");
                self.active_server = connected_srv.clone();
                self.server_input = connected_srv;
                self.active_email = Some(email);
                self.is_authenticated = true;
                self.success_message = Some(msg);
                self.error_message = None;
            }
            Ok((false, reason, _)) => {
                log::warn!("MonarchCAD: saved license for {email} invalid: {reason}");
                self.error_message = Some(reason);
            }
            Err(e) => {
                log::warn!("MonarchCAD: cannot verify saved license: {e}");
                self.error_message = Some("Can't open, contact developer team.".to_string());
            }
        }
    }

    pub fn verify_input(&mut self) {
        let email = self.email_input.trim().to_string();
        if email.is_empty() {
            self.error_message = Some("Please enter a valid email address.".to_string());
            return;
        }

        self.is_verifying = true;
        self.error_message = None;

        match request_verification(&self.active_server, &email) {
            Ok((true, msg, connected_srv)) => {
                log::info!("MonarchCAD: activated {email} via {connected_srv}");
                self.active_server = connected_srv.clone();
                self.server_input = connected_srv;
                self.active_email = Some(email.clone());
                self.is_authenticated = true;
                self.success_message = Some(msg);
                self.error_message = None;

                // Save to local file
                let saved = serde_json::json!({
                    "email": email,
                    "server": self.active_server
                });
                if let Ok(json) = serde_json::to_string_pretty(&saved) {
                    if let Some(parent) = self.license_file.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::write(&self.license_file, json);
                }

                self.check_update();
            }
            Ok((false, reason, _)) => {
                self.is_authenticated = false;
                self.error_message = Some(reason);
            }
            Err(e) => {
                self.is_authenticated = false;
                log::warn!("MonarchCAD: license verification failed: {e}");
                self.error_message = Some("Can't open, contact developer team.".to_string());
            }
        }

        self.is_verifying = false;
    }

    pub fn render_modal(&mut self, ctx: &egui::Context) -> bool {
        let mut quit_clicked = false;

        // If an update is available, prompt user to update
        if self.update_available {
            egui::Modal::new(egui::Id::new("monarch_update_modal")).show(ctx, |ui| {
                ui.set_width(450.0);
                ui.heading(RichText::new("🚀 System Update Available").strong().size(18.0).color(Color32::from_rgb(52, 211, 153)));
                ui.add_space(8.0);
                ui.label(format!("A new version of MonarchCAD (v{}) is available from the server.", self.update_version));
                if !self.update_notes.is_empty() {
                    ui.label(RichText::new(format!("Notes: {}", self.update_notes)).color(Color32::from_rgb(148, 163, 184)));
                }
                ui.add_space(14.0);

                if self.is_downloading_update {
                    ui.label(
                        RichText::new("Downloading update and preparing restart... please wait!").strong().color(Color32::from_rgb(96, 165, 250)),
                    );
                    ui.spinner();
                } else {
                    if let Some(err) = &self.update_status {
                        ui.label(RichText::new(err).color(Color32::from_rgb(248, 113, 113)));
                        ui.add_space(6.0);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(RichText::new("Update & Restart Now").strong()).clicked() {
                            self.is_downloading_update = true;
                            let srv = self.active_server.clone();
                            let _ = std::thread::Builder::new().name("updater-bg".into()).spawn(move || {
                                let _ = download_and_apply_update(&srv);
                            });
                        }
                        if ui.button("Later").clicked() {
                            self.update_available = false;
                        }
                    });
                }
            });
            return false;
        }

        if self.is_authenticated {
            return false;
        }

        egui::Modal::new(egui::Id::new("monarch_activation_modal")).show(ctx, |ui| {
            ui.set_width(420.0);

            ui.heading(RichText::new("MonarchCAD Activation").strong().size(18.0));
            ui.add_space(6.0);
            ui.label("Enter your licensed email address to use MonarchCAD.");
            ui.label(RichText::new("Please activate your license to continue.").color(Color32::from_rgb(148, 163, 184)));

            ui.add_space(14.0);
            ui.label(RichText::new("Email Address:").strong());

            let email_response =
                ui.add_sized([ui.available_width(), 28.0], egui::TextEdit::singleline(&mut self.email_input).hint_text("user@example.com"));

            if email_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.verify_input();
            }

            if let Some(err) = &self.error_message {
                ui.add_space(8.0);
                ui.label(RichText::new(err).color(Color32::from_rgb(248, 113, 113)).size(12.5));
            }

            ui.add_space(16.0);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let btn_size = vec2(90.0, 26.0);
                let activate_text = if self.is_verifying { "Verifying..." } else { "Activate" };

                if ui.add_sized(btn_size, egui::Button::new(RichText::new(activate_text).strong())).clicked() {
                    self.verify_input();
                }

                if ui.add_sized(btn_size, egui::Button::new("Quit")).clicked() {
                    quit_clicked = true;
                }
            });
        });

        quit_clicked
    }

    pub fn render_top_right_badge(&self, ctx: &egui::Context) {
        if !self.is_authenticated {
            return;
        }
        let Some(email) = &self.active_email else {
            return;
        };

        egui::Area::new(egui::Id::new("monarch_top_right_account_badge"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-10.0, 2.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.scope(|ui| {
                    egui::Frame::new()
                        .fill(Color32::from_rgba_premultiplied(15, 23, 42, 230))
                        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(8, 2))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 5.0;
                                // Green active indicator dot
                                ui.label(RichText::new("●").color(Color32::from_rgb(34, 197, 94)).size(9.0));
                                // User Icon & Email
                                let label_resp = ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("👤 {email}")).color(Color32::from_rgb(241, 245, 249)).size(11.0).strong(),
                                    )
                                    .sense(egui::Sense::hover()),
                                );
                                label_resp.on_hover_ui(|ui| {
                                    ui.set_max_width(260.0);
                                    ui.heading(RichText::new("MonarchCAD License").size(13.0).color(Color32::from_rgb(96, 165, 250)));
                                    ui.add_space(4.0);
                                    ui.label(RichText::new(format!("User: {email}")).strong());
                                    ui.label(RichText::new(format!("Server: {}", self.active_server)).color(Color32::from_rgb(148, 163, 184)));
                                    ui.label(RichText::new("Status: Activated & Online").color(Color32::from_rgb(34, 197, 94)));
                                });
                            });
                        });
                });
            });
    }
}

fn do_single_verify(target: &str, email: &str) -> Result<(bool, String), String> {
    let Ok(addrs) = target.to_socket_addrs() else {
        return Err(format!("DNS resolution failed for {target}"));
    };
    for addr in addrs {
        let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(1500)) else {
            continue;
        };

        let _ = stream.set_read_timeout(Some(Duration::from_millis(2500)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(1500)));

        let payload = serde_json::json!({ "email": email }).to_string();
        let request = format!(
            "POST /api/license/verify HTTP/1.1\r\nHost: {target}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            payload.len(),
            payload
        );

        if stream.write_all(request.as_bytes()).is_err() {
            continue;
        }

        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        while let Ok(n) = stream.read(&mut chunk) {
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > 16 * 1024 {
                break;
            }
        }

        let _ = stream.shutdown(std::net::Shutdown::Both);

        let raw = String::from_utf8_lossy(&buf);
        let parts: Vec<&str> = raw.split("\r\n\r\n").collect();
        if parts.len() < 2 {
            continue;
        }

        let body = parts[1];
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) else {
            continue;
        };

        let valid = parsed.get("valid").and_then(|v| v.as_bool()).unwrap_or(false);
        let message = parsed.get("message").and_then(|v| v.as_str()).unwrap_or("No details provided").to_string();

        return Ok((valid, message));
    }
    Err(format!("Could not connect to {target}"))
}

fn request_verification(server: &str, email: &str) -> Result<(bool, String, String), String> {
    let mut candidates = vec!["192.168.1.111:8000".to_string(), "175.100.175.42:8000".to_string(), "127.0.0.1:8000".to_string()];
    if !server.is_empty() {
        let norm = normalize_server_addr(server, 8000);
        if !candidates.contains(&norm) {
            candidates.insert(0, norm);
        }
    }

    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let num_candidates = candidates.len();

    for target in candidates {
        let tx = tx.clone();
        let target_str = target.clone();
        let email_str = email.to_string();
        let _ = std::thread::Builder::new().name("verify-probe".into()).spawn(move || {
            let res = do_single_verify(&target_str, &email_str);
            let _ = tx.send((target_str, res));
        });
    }
    drop(tx);

    let mut last_err = "No address found for server".to_string();
    let mut received = 0;

    while received < num_candidates {
        match rx.recv_timeout(Duration::from_millis(2500)) {
            Ok((tgt, Ok((valid, msg)))) => {
                return Ok((valid, msg, tgt));
            }
            Ok((_tgt, Err(e))) => {
                last_err = e;
                received += 1;
            }
            Err(_) => {
                break;
            }
        }
    }

    Err(last_err)
}

fn do_single_update_check(target: &str, current_ver: &str) -> Option<(String, String)> {
    let Ok(addrs) = target.to_socket_addrs() else {
        return None;
    };
    for addr in addrs {
        let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(1500)) else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(2000)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(1500)));

        let req = format!("GET /api/update/check?version={current_ver} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
        if stream.write_all(req.as_bytes()).is_err() {
            continue;
        }

        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        while let Ok(n) = stream.read(&mut chunk) {
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > 16 * 1024 {
                break;
            }
        }

        let _ = stream.shutdown(std::net::Shutdown::Both);

        let raw = String::from_utf8_lossy(&buf);
        let parts: Vec<&str> = raw.split("\r\n\r\n").collect();
        if parts.len() < 2 {
            continue;
        }

        if let Ok(val) = serde_json::from_str::<serde_json::Value>(parts[1])
            && val.get("update_available").and_then(|v| v.as_bool()).unwrap_or(false)
        {
            let latest = val.get("latest_version").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let notes = val.get("notes").and_then(|v| v.as_str()).unwrap_or("").to_string();
            return Some((latest, notes));
        }
        return None;
    }
    None
}

fn request_update_check(server: &str, current_ver: &str) -> Option<(String, String)> {
    let mut candidates = vec!["192.168.1.111:8000".to_string(), "175.100.175.42:8000".to_string(), "127.0.0.1:8000".to_string()];
    if !server.is_empty() {
        let norm = normalize_server_addr(server, 8000);
        if !candidates.contains(&norm) {
            candidates.insert(0, norm);
        }
    }

    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let num_candidates = candidates.len();

    for target in candidates {
        let tx = tx.clone();
        let target_str = target.clone();
        let cur = current_ver.to_string();
        let _ = std::thread::Builder::new().name("update-chk".into()).spawn(move || {
            let res = do_single_update_check(&target_str, &cur);
            let _ = tx.send(res);
        });
    }
    drop(tx);

    let mut received = 0;
    while received < num_candidates {
        match rx.recv_timeout(Duration::from_millis(2000)) {
            Ok(Some((ver, notes))) => return Some((ver, notes)),
            Ok(None) => {
                received += 1;
            }
            Err(_) => break,
        }
    }
    None
}

pub fn download_and_apply_update(server: &str) -> Result<(), String> {
    let mut candidates = vec![normalize_server_addr(server, 8000)];
    for &fb in FALLBACK_SERVERS {
        let norm = normalize_server_addr(fb, 8000);
        if !candidates.contains(&norm) {
            candidates.push(norm);
        }
    }

    let current_exe = std::env::current_exe().map_err(|e| format!("Current exe path error: {e}"))?;
    let parent = current_exe.parent().ok_or_else(|| "Cannot determine exe parent directory".to_string())?;
    let new_exe = parent.join("monarchcad.exe.new");

    for target in &candidates {
        let Ok(addrs) = target.to_socket_addrs() else {
            continue;
        };
        for addr in addrs {
            let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(1500)) else {
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

            let req = format!("GET /api/update/download HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
            if stream.write_all(req.as_bytes()).is_err() {
                continue;
            }

            // Read headers byte-by-byte until \r\n\r\n
            let mut header_buf = Vec::new();
            let mut byte = [0u8; 1];
            let mut found_header = false;

            while let Ok(1) = stream.read(&mut byte) {
                header_buf.push(byte[0]);
                if header_buf.ends_with(b"\r\n\r\n") {
                    found_header = true;
                    break;
                }
                if header_buf.len() > 8192 {
                    break;
                }
            }

            if !found_header {
                continue;
            }

            // Stream body to new_exe file
            let mut file = fs::File::create(&new_exe).map_err(|e| format!("Failed to create new exe file: {e}"))?;
            let mut chunk = [0u8; 65536];
            let mut total_downloaded = 0usize;

            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                file.write_all(&chunk[..n]).map_err(|e| format!("Write failed: {e}"))?;
                total_downloaded += n;
            }

            let _ = stream.shutdown(std::net::Shutdown::Both);

            if total_downloaded < 100_000 {
                let _ = fs::remove_file(&new_exe);
                return Err("Downloaded update binary is incomplete or corrupted".to_string());
            }

            drop(file);

            // Execute Windows batch updater script
            #[cfg(target_os = "windows")]
            {
                let bat_path = parent.join("monarch_update.bat");
                let bat_content = format!(
                    "@echo off\r\n\
                     :retry\r\n\
                     timeout /t 1 /nobreak >nul\r\n\
                     move /y \"{}\" \"{}\" >nul 2>&1\r\n\
                     if exist \"{}\" (\r\n\
                         goto retry\r\n\
                     )\r\n\
                     start \"\" \"{}\"\r\n\
                     del \"%~f0\"\r\n",
                    new_exe.display(),
                    current_exe.display(),
                    new_exe.display(),
                    current_exe.display()
                );
                fs::write(&bat_path, bat_content).map_err(|e| format!("Failed to write updater script: {e}"))?;
                let _ = std::process::Command::new("cmd").args(["/c", &bat_path.to_string_lossy()]).spawn();
                std::process::exit(0);
            }

            #[cfg(not(target_os = "windows"))]
            {
                let _ = fs::rename(&new_exe, &current_exe);
                std::process::exit(0);
            }
        }
    }

    Err("Failed to download update from server".to_string())
}
