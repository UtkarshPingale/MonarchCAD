//! Heartbeat watchdog for MonarchCAD.
//!
//! Pings the backend service (`/heartbeat?email=...`).
//! If the backend stops responding for consecutive intervals, or if the
//! user's email access is revoked or expired by the administrator,
//! signals the application to terminate.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Watchdog {
    should_quit: Arc<AtomicBool>,
    email: Arc<RwLock<Option<String>>>,
    server: Arc<RwLock<String>>,
}

impl Watchdog {
    pub fn new(initial_server: String, egui_ctx: egui::Context) -> Self {
        let should_quit = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&should_quit);
        let email = Arc::new(RwLock::new(None));
        let email_clone = Arc::clone(&email);
        let server = Arc::new(RwLock::new(initial_server));
        let server_clone = Arc::clone(&server);

        let _ = thread::Builder::new().name("monarch-watchdog".into()).spawn(move || {
            run_watchdog_loop(server_clone, flag, email_clone, egui_ctx);
        });

        Self { should_quit, email, server }
    }

    pub fn disabled() -> Self {
        Self { should_quit: Arc::new(AtomicBool::new(false)), email: Arc::new(RwLock::new(None)), server: Arc::new(RwLock::new(String::new())) }
    }

    pub fn set_email(&self, new_email: String) {
        if let Ok(mut lock) = self.email.write() {
            *lock = Some(new_email);
        }
    }

    pub fn set_server(&self, new_server: String) {
        if let Ok(mut lock) = self.server.write() {
            *lock = new_server;
        }
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit.load(Ordering::Relaxed)
    }
}

fn check_heartbeat(server: &str, email: Option<&str>) -> Option<String> {
    let clean = server.trim().trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/');
    let primary = if clean.is_empty() {
        "192.168.1.111:8000".to_string()
    } else if clean.contains(':') {
        clean.to_string()
    } else {
        format!("{clean}:8000")
    };

    let mut candidates = vec![primary];
    for &fb in &["192.168.1.111:8000", "175.100.175.42:8000", "127.0.0.1:8000"] {
        let norm = fb.to_string();
        if !candidates.contains(&norm) {
            candidates.push(norm);
        }
    }

    for target in candidates {
        let Ok(addrs) = target.to_socket_addrs() else {
            continue;
        };
        for addr in addrs {
            let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(800)) else {
                continue;
            };

            let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(1000)));

            let request = match email {
                Some(e) => format!("GET /heartbeat?email={e} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n"),
                None => format!("GET /heartbeat HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n"),
            };

            if stream.write_all(request.as_bytes()).is_err() {
                continue;
            }

            let mut buf = [0u8; 512];
            let ok = match stream.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let resp = String::from_utf8_lossy(&buf[..n]);
                    resp.contains("200") && !resp.contains("403") && !resp.contains("401")
                }
                _ => false,
            };

            let _ = stream.shutdown(std::net::Shutdown::Both);

            if ok {
                return Some(target);
            }
        }
    }

    None
}

fn run_watchdog_loop(server_lock: Arc<RwLock<String>>, should_quit: Arc<AtomicBool>, email_lock: Arc<RwLock<Option<String>>>, ctx: egui::Context) {
    let initial_srv = server_lock.read().ok().map(|g| g.clone()).unwrap_or_else(|| "127.0.0.1:8000".to_string());
    log::info!("MonarchCAD watchdog started: monitoring backend on {initial_srv}/heartbeat");
    // Initial startup grace period (wait 6 seconds to give the backend time to initialize)
    thread::sleep(Duration::from_secs(6));

    let mut consecutive_failures: u32 = 0;
    const MAX_FAILURES: u32 = 4;
    const INTERVAL: Duration = Duration::from_secs(3);

    loop {
        let current_email = email_lock.read().ok().and_then(|guard| guard.clone());
        let Some(email) = current_email else {
            // User is at the activation modal or not yet logged in. Do not enforce watchdog quit.
            thread::sleep(Duration::from_millis(500));
            continue;
        };

        let server = server_lock.read().ok().map(|g| g.clone()).unwrap_or_else(|| "127.0.0.1:8000".to_string());

        if let Some(working_target) = check_heartbeat(&server, Some(&email)) {
            if working_target != server
                && let Ok(mut lock) = server_lock.write()
            {
                *lock = working_target.clone();
            }
            if consecutive_failures > 0 {
                log::info!("MonarchCAD watchdog: backend reconnected successfully to {working_target}.");
            }
            consecutive_failures = 0;
        } else {
            consecutive_failures = consecutive_failures.saturating_add(1);
            log::warn!("MonarchCAD watchdog: backend heartbeat to {server} failed ({consecutive_failures}/{MAX_FAILURES})");

            if consecutive_failures >= MAX_FAILURES {
                log::error!("MonarchCAD watchdog: backend on {server} stopped or access revoked. Shutting down.");
                should_quit.store(true, Ordering::Relaxed);
                ctx.request_repaint();
                break;
            }
        }

        thread::sleep(INTERVAL);
    }
}
