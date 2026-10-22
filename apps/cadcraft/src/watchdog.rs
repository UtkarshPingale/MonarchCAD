//! Heartbeat watchdog for MonarchCAD.
//!
//! Pings a local backend service (default `127.0.0.1:8000/heartbeat`).
//! If the backend stops responding for consecutive intervals (or is not
//! reachable after initial startup grace period), signals the application to terminate.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Watchdog {
    should_quit: Arc<AtomicBool>,
}

impl Watchdog {
    pub fn new(port: u16, egui_ctx: egui::Context) -> Self {
        let should_quit = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&should_quit);

        let _ = thread::Builder::new()
            .name("monarch-watchdog".into())
            .spawn(move || {
                run_watchdog_loop(port, flag, egui_ctx);
            });

        Self { should_quit }
    }

    pub fn disabled() -> Self {
        Self {
            should_quit: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit.load(Ordering::Relaxed)
    }
}

fn check_heartbeat(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(1500)) else {
        return false;
    };

    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(1500)));

    let request = format!("GET /heartbeat HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut buf = [0u8; 512];
    let ok = match stream.read(&mut buf) {
        Ok(n) if n > 0 => {
            let resp = String::from_utf8_lossy(&buf[..n]);
            resp.contains("200") || resp.contains("OK") || resp.contains("ok")
        }
        _ => false,
    };

    // Gracefully drain and shut down socket so remote server receives FIN, not RST
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut drain = [0u8; 256];
    while let Ok(n) = stream.read(&mut drain) {
        if n == 0 {
            break;
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Read);

    ok
}

fn run_watchdog_loop(port: u16, should_quit: Arc<AtomicBool>, ctx: egui::Context) {
    log::info!("MonarchCAD watchdog started: monitoring backend on 127.0.0.1:{port}/heartbeat");
    // Initial startup grace period (wait 6 seconds to give the backend time to initialize)
    thread::sleep(Duration::from_secs(6));

    let mut consecutive_failures: u32 = 0;
    const MAX_FAILURES: u32 = 3;
    const INTERVAL: Duration = Duration::from_secs(2);

    loop {
        if check_heartbeat(port) {
            if consecutive_failures > 0 {
                log::info!("MonarchCAD watchdog: backend reconnected successfully on port {port}.");
            }
            consecutive_failures = 0;
        } else {
            consecutive_failures = consecutive_failures.saturating_add(1);
            log::warn!(
                "MonarchCAD watchdog: backend heartbeat failed ({consecutive_failures}/{MAX_FAILURES})"
            );

            if consecutive_failures >= MAX_FAILURES {
                log::error!(
                    "MonarchCAD watchdog: backend on port {port} is stopped or unreachable. Shutting down."
                );
                should_quit.store(true, Ordering::Relaxed);
                ctx.request_repaint();
                break;
            }
        }

        thread::sleep(INTERVAL);
    }
}
