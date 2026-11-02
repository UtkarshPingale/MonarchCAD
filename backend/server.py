#!/usr/bin/env python3
"""
MonarchCAD Backend Service & Licensing Server.

Single-file server providing:
1. Heartbeat monitoring for MonarchCAD.exe (auto-kill if server stops).
2. SQLite persistent database for user email licenses.
3. Web Admin Dashboard at http://127.0.0.1:8000/admin to grant access (days or lifetime).
4. License verification API for MonarchCAD in-app login.
"""

import sys
import os
import json
import time
import sqlite3
from datetime import datetime, timedelta
from urllib.parse import urlparse, parse_qs
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler

PORT = 8000
DB_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "monarch_licenses.db")

# ---------------------------------------------------------------------------
# Database Management
# ---------------------------------------------------------------------------

def init_db():
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS licenses (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            email TEXT UNIQUE NOT NULL COLLATE NOCASE,
            is_lifetime INTEGER DEFAULT 0,
            expires_at TEXT,
            granted_at TEXT NOT NULL,
            status TEXT DEFAULT 'active',
            notes TEXT DEFAULT ''
        )
    """)
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS system_settings (
            key TEXT PRIMARY KEY,
            value TEXT
        )
    """)
    cursor.execute("INSERT OR IGNORE INTO system_settings (key, value) VALUES ('app_version', '1.0.1')")
    cursor.execute("INSERT OR IGNORE INTO system_settings (key, value) VALUES ('update_notes', 'Official MonarchCAD system update')")
    cursor.execute("INSERT OR IGNORE INTO system_settings (key, value) VALUES ('force_update', '1')")
    conn.commit()
    conn.close()

def grant_license(email: str, duration: str, notes: str = ""):
    email = email.strip().lower()
    if not email:
        return False, "Email cannot be empty"

    now = datetime.now()
    granted_at = now.strftime("%Y-%m-%d %H:%M:%S")

    if duration == "lifetime":
        is_lifetime = 1
        expires_at = None
    else:
        try:
            days = int(duration)
            is_lifetime = 0
            expires_at = (now + timedelta(days=days)).strftime("%Y-%m-%d %H:%M:%S")
        except ValueError:
            return False, f"Invalid duration value: {duration}"

    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    cursor.execute("""
        INSERT INTO licenses (email, is_lifetime, expires_at, granted_at, status, notes)
        VALUES (?, ?, ?, ?, 'active', ?)
        ON CONFLICT(email) DO UPDATE SET
            is_lifetime=excluded.is_lifetime,
            expires_at=excluded.expires_at,
            granted_at=excluded.granted_at,
            status='active',
            notes=excluded.notes
    """, (email, is_lifetime, expires_at, granted_at, notes))
    conn.commit()
    conn.close()
    return True, "License granted successfully"

def revoke_license(email: str):
    email = email.strip().lower()
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    cursor.execute("UPDATE licenses SET status = 'revoked' WHERE email = ?", (email,))
    affected = cursor.rowcount
    conn.commit()
    conn.close()
    return affected > 0

def activate_license(email: str):
    email = email.strip().lower()
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    cursor.execute("UPDATE licenses SET status = 'active' WHERE email = ?", (email,))
    affected = cursor.rowcount
    conn.commit()
    conn.close()
    return affected > 0

def delete_license(email: str):
    email = email.strip().lower()
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    cursor.execute("DELETE FROM licenses WHERE email = ?", (email,))
    affected = cursor.rowcount
    conn.commit()
    conn.close()
    return affected > 0

LAST_HEARTBEATS = {}  # email -> timestamp float

def get_all_licenses():
    conn = sqlite3.connect(DB_PATH)
    conn.row_factory = sqlite3.Row
    cursor = conn.cursor()
    cursor.execute("SELECT * FROM licenses ORDER BY id DESC")
    rows = cursor.fetchall()
    conn.close()

    now = datetime.now()
    now_ts = time.time()
    result = []
    for r in rows:
        item = dict(r)
        email_clean = item["email"].strip().lower()
        last_seen = LAST_HEARTBEATS.get(email_clean, 0)
        diff = now_ts - last_seen if last_seen > 0 else 999999
        is_online = (last_seen > 0) and (diff < 15)

        if last_seen == 0:
            last_seen_text = "Never"
        elif diff < 10:
            last_seen_text = "Just now"
        elif diff < 60:
            last_seen_text = f"{int(diff)}s ago"
        elif diff < 3600:
            last_seen_text = f"{int(diff // 60)}m ago"
        else:
            last_seen_text = f"{int(diff // 3600)}h ago"

        item["is_online"] = is_online
        item["last_seen_text"] = last_seen_text

        if item["is_lifetime"]:
            item["days_left"] = "Lifetime"
            item["is_expired"] = False
        elif item["expires_at"]:
            try:
                exp = datetime.strptime(item["expires_at"], "%Y-%m-%d %H:%M:%S")
                delta = exp - now
                if delta.total_seconds() > 0:
                    item["days_left"] = f"{delta.days}d {delta.seconds // 3600}h"
                    item["is_expired"] = False
                else:
                    item["days_left"] = "Expired"
                    item["is_expired"] = True
            except ValueError:
                item["days_left"] = "Unknown"
                item["is_expired"] = False
        else:
            item["days_left"] = "N/A"
            item["is_expired"] = False

        result.append(item)
    return result

def verify_license(email: str):
    email = email.strip().lower()
    if not email:
        return False, "Email address is required", None

    conn = sqlite3.connect(DB_PATH)
    conn.row_factory = sqlite3.Row
    cursor = conn.cursor()
    cursor.execute("SELECT * FROM licenses WHERE email = ?", (email,))
    row = cursor.fetchone()
    conn.close()

    if not row:
        return False, "Email not found in registered licenses. Please contact Administrator.", None

    if row["status"] != "active":
        return False, "Access has been revoked by Administrator.", None

    if row["is_lifetime"]:
        return True, "Active (Lifetime)", {
            "email": row["email"],
            "is_lifetime": True,
            "expires_at": "Lifetime",
            "days_left": 99999
        }

    if row["expires_at"]:
        try:
            exp = datetime.strptime(row["expires_at"], "%Y-%m-%d %H:%M:%S")
            now = datetime.now()
            if now > exp:
                return False, f"License expired on {row['expires_at']}. Contact Administrator to renew.", None
            days_left = max(1, (exp - now).days)
            return True, f"Active ({days_left} days remaining)", {
                "email": row["email"],
                "is_lifetime": False,
                "expires_at": row["expires_at"],
                "days_left": days_left
            }
        except ValueError:
            pass

    return False, "License record invalid.", None

# ---------------------------------------------------------------------------
# Updates & Version Management
# ---------------------------------------------------------------------------

UPDATES_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "updates")
if not os.path.exists(UPDATES_DIR):
    try:
        os.makedirs(UPDATES_DIR, exist_ok=True)
    except Exception:
        pass

def get_setting(key: str, default: str = "") -> str:
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    cursor.execute("SELECT value FROM system_settings WHERE key = ?", (key,))
    row = cursor.fetchone()
    conn.close()
    return row[0] if row else default

def set_setting(key: str, value: str):
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    cursor.execute("""
        INSERT INTO system_settings (key, value) VALUES (?, ?)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
    """, (key, str(value)))
    conn.commit()
    conn.close()

def get_latest_exe_path() -> str:
    # 1. updates directory
    p1 = os.path.join(UPDATES_DIR, "monarchcad.exe")
    if os.path.isfile(p1):
        return p1
    # 2. workspace root
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    p2 = os.path.join(root, "monarchcad.exe")
    if os.path.isfile(p2):
        return p2
    # 3. target/release
    p3 = os.path.join(root, "target", "release", "monarchcad.exe")
    if os.path.isfile(p3):
        return p3
    return ""

def ensure_zip_package() -> str:
    exe_path = get_latest_exe_path()
    if not exe_path or not os.path.isfile(exe_path):
        return ""
    zip_path = os.path.join(UPDATES_DIR, "MonarchCAD.zip")
    try:
        import zipfile
        with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as z:
            z.write(exe_path, "monarchcad.exe")
        return zip_path
    except Exception:
        return ""

def is_newer_version(latest: str, current: str) -> bool:
    try:
        def parse(v):
            return [int(x) for x in v.strip().split(".") if x.isdigit()]
        l_parts = parse(latest)
        c_parts = parse(current)
        while len(l_parts) < 3:
            l_parts.append(0)
        while len(c_parts) < 3:
            c_parts.append(0)
        return l_parts > c_parts
    except Exception:
        return False

def get_update_status(client_ver: str):
    latest_ver = get_setting("app_version", "1.0.2")
    notes = get_setting("update_notes", "Official MonarchCAD system update")
    force = get_setting("force_update", "1") == "1"
    
    exe_path = get_latest_exe_path()
    file_size = os.path.getsize(exe_path) if exe_path and os.path.isfile(exe_path) else 0
    mtime = os.path.getmtime(exe_path) if exe_path and os.path.isfile(exe_path) else 0

    update_available = (is_newer_version(latest_ver, client_ver) and file_size > 0)
    
    return {
        "latest_version": latest_ver,
        "client_version": client_ver,
        "update_available": update_available,
        "force_update": force,
        "download_url": "/api/update/download",
        "zip_url": "/download/MonarchCAD.zip",
        "file_size": file_size,
        "file_modified": datetime.fromtimestamp(mtime).strftime("%Y-%m-%d %H:%M:%S") if mtime else "",
        "notes": notes
    }

# ---------------------------------------------------------------------------
# HTML Admin Dashboard
# ---------------------------------------------------------------------------

ADMIN_HTML = """<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>MonarchCAD Admin - License Manager</title>
    <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap" rel="stylesheet">
    <style>
        :root {
            --bg: #0f141c;
            --card: #181f2a;
            --border: #263346;
            --primary: #3b82f6;
            --primary-hover: #2563eb;
            --success: #10b981;
            --warning: #f59e0b;
            --danger: #ef4444;
            --text: #f1f5f9;
            --text-dim: #94a3b8;
        }
        * { box-sizing: border-box; margin: 0; padding: 0; font-family: 'Inter', sans-serif; }
        body { background: var(--bg); color: var(--text); padding: 28px; }
        .container { max-width: 1200px; margin: 0 auto; }
        header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 24px; padding-bottom: 20px; border-bottom: 1px solid var(--border); }
        .logo-box { display: flex; align-items: center; gap: 14px; }
        .logo-icon { width: 38px; height: 38px; background: linear-gradient(135deg, #3b82f6, #6366f1); border-radius: 8px; display: flex; align-items: center; justify-content: center; font-weight: bold; font-size: 20px; }
        h1 { font-size: 22px; font-weight: 700; letter-spacing: -0.5px; }
        p.subtitle { color: var(--text-dim); font-size: 13px; margin-top: 2px; }
        .stats-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 16px; margin-bottom: 24px; }
        .stat-card { background: var(--card); border: 1px solid var(--border); padding: 18px; border-radius: 12px; }
        .stat-num { font-size: 28px; font-weight: 700; margin-top: 6px; }
        .stat-label { font-size: 12px; text-transform: uppercase; color: var(--text-dim); font-weight: 600; }
        .panel { background: var(--card); border: 1px solid var(--border); border-radius: 12px; padding: 22px; margin-bottom: 24px; }
        .panel h2 { font-size: 17px; margin-bottom: 16px; font-weight: 600; }
        .form-row { display: grid; grid-template-columns: 2fr 1.2fr 1.5fr auto; gap: 12px; align-items: end; }
        .form-group label { display: block; font-size: 12px; font-weight: 600; color: var(--text-dim); margin-bottom: 6px; }
        input, select { width: 100%; background: #0f141c; border: 1px solid var(--border); color: var(--text); padding: 10px 14px; border-radius: 8px; font-size: 14px; outline: none; }
        input:focus, select:focus { border-color: var(--primary); }
        button.btn { background: var(--primary); color: white; border: none; padding: 11px 20px; border-radius: 8px; font-weight: 600; cursor: pointer; transition: 0.15s; font-size: 14px; }
        button.btn:hover { background: var(--primary-hover); }
        button.btn-danger { background: rgba(239, 68, 68, 0.15); color: #f87171; border: 1px solid rgba(239, 68, 68, 0.3); padding: 6px 12px; border-radius: 6px; font-size: 12px; cursor: pointer; }
        button.btn-danger:hover { background: var(--danger); color: white; }
        button.btn-success { background: rgba(16, 185, 129, 0.15); color: #34d399; border: 1px solid rgba(16, 185, 129, 0.3); padding: 6px 12px; border-radius: 6px; font-size: 12px; cursor: pointer; margin-right: 6px; }
        button.btn-success:hover { background: var(--success); color: white; }
        button.btn-delete { background: transparent; color: var(--text-dim); border: none; font-size: 12px; cursor: pointer; margin-left: 8px; text-decoration: underline; }
        button.btn-delete:hover { color: var(--danger); }
        table { width: 100%; border-collapse: collapse; margin-top: 14px; }
        th { text-align: left; padding: 12px 14px; font-size: 12px; text-transform: uppercase; color: var(--text-dim); border-bottom: 1px solid var(--border); font-weight: 600; }
        td { padding: 14px; border-bottom: 1px solid var(--border); font-size: 14px; }
        tr:last-child td { border-bottom: none; }
        .badge { display: inline-block; padding: 3px 8px; border-radius: 999px; font-size: 11px; font-weight: 600; }
        .badge-active { background: rgba(16, 185, 129, 0.15); color: #34d399; border: 1px solid rgba(16, 185, 129, 0.3); }
        .badge-lifetime { background: rgba(99, 102, 241, 0.2); color: #818cf8; border: 1px solid rgba(99, 102, 241, 0.4); }
        .badge-expired { background: rgba(245, 158, 11, 0.15); color: #fbbf24; border: 1px solid rgba(245, 158, 11, 0.3); }
        .badge-revoked { background: rgba(239, 68, 68, 0.15); color: #f87171; border: 1px solid rgba(239, 68, 68, 0.3); }
        .badge-online { background: rgba(16, 185, 129, 0.2); color: #34d399; border: 1px solid rgba(16, 185, 129, 0.4); display: inline-flex; align-items: center; gap: 6px; }
        .badge-offline { background: rgba(148, 163, 184, 0.1); color: #94a3b8; border: 1px solid rgba(148, 163, 184, 0.2); }
        .pulse-dot { width: 7px; height: 7px; border-radius: 50%; background: #10b981; box-shadow: 0 0 8px #10b981; animation: pulse 1.5s infinite; }
        @keyframes pulse {
            0% { transform: scale(0.95); opacity: 0.8; }
            50% { transform: scale(1.25); opacity: 1; }
            100% { transform: scale(0.95); opacity: 0.8; }
        }
        .search-box { margin-bottom: 14px; }
        #toast { position: fixed; bottom: 24px; right: 24px; background: #1e293b; color: white; border: 1px solid var(--border); padding: 12px 20px; border-radius: 8px; font-size: 14px; display: none; z-index: 100; box-shadow: 0 10px 25px rgba(0,0,0,0.5); }
    </style>
</head>
<body>
    <div class="container">
        <header>
            <div class="logo-box">
                <img src="https://static.wixstatic.com/media/3b9e2e_869e2d96e1bc443992b03361e46d6f30~mv2.png" alt="MonarchCAD Logo" style="height: 52px; width: auto; object-fit: contain;">
                <div>
                    <h1>MonarchCAD Access Manager</h1>
                    <p class="subtitle">Grant and revoke user access for MonarchCAD (.exe)</p>
                </div>
            </div>
            <div>
                <span class="badge badge-active" style="padding: 6px 12px; font-size: 13px;">Server Online &bull; Port 8000</span>
            </div>
        </header>

        <div class="stats-grid">
            <div class="stat-card">
                <div class="stat-label">Total Registered</div>
                <div class="stat-num" id="stat-total">0</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Currently Online</div>
                <div class="stat-num" style="color: #34d399; display: flex; align-items: center; gap: 8px;">
                    <span class="pulse-dot"></span>
                    <span id="stat-online">0</span>
                </div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Active Subscriptions</div>
                <div class="stat-num" style="color: #60a5fa;" id="stat-active">0</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Lifetime Licenses</div>
                <div class="stat-num" style="color: #818cf8;" id="stat-lifetime">0</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Expired / Revoked</div>
                <div class="stat-num" style="color: var(--warning);" id="stat-expired">0</div>
            </div>
        </div>

        <div class="panel" style="background: linear-gradient(135deg, rgba(30, 41, 59, 0.85), rgba(15, 23, 42, 0.95)); border: 1px solid rgba(59, 130, 246, 0.35); box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);">
            <div style="display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 16px;">
                <div>
                    <h2 style="margin: 0 0 6px 0; color: #60a5fa; display: flex; align-items: center; gap: 8px;">
                        <span>📥</span> Download MonarchCAD Client Package
                    </h2>
                    <p style="margin: 0; color: var(--text-dim); font-size: 13px;">
                        Self-trusting standalone executable Authenticode-signed by verified publisher <strong>MonarchCAD</strong>. Automatically unblocks and registers publisher trust on run.
                    </p>
                </div>
                <div style="display: flex; gap: 10px; flex-wrap: wrap;">
                    <a href="/download" style="text-decoration: none; padding: 10px 22px; font-size: 14px; font-weight: 700; background: linear-gradient(135deg, #10b981, #059669); color: white; border-radius: 8px; display: inline-flex; align-items: center; gap: 8px; box-shadow: 0 4px 14px rgba(16, 185, 129, 0.4);">
                        ⚡ Download MonarchCAD (.EXE)
                    </a>
                    <a href="/download/zip" style="text-decoration: none; padding: 10px 18px; font-size: 14px; font-weight: 600; background: #334155; color: white; border-radius: 8px; display: inline-flex; align-items: center; gap: 8px;">
                        📦 ZIP Package
                    </a>
                </div>
            </div>
        </div>

        <div class="panel">
            <h2>Grant / Extend User Access</h2>
            <form id="grant-form" onsubmit="handleGrant(event)">
                <div class="form-row">
                    <div class="form-group">
                        <label>User Email Address</label>
                        <input type="email" id="email" required placeholder="user@company.com">
                    </div>
                    <div class="form-group">
                        <label>Duration</label>
                        <select id="duration">
                            <option value="7">7 Days (Trial)</option>
                            <option value="30" selected>30 Days (1 Month)</option>
                            <option value="90">90 Days (3 Months)</option>
                            <option value="180">180 Days (6 Months)</option>
                            <option value="365">365 Days (1 Year)</option>
                            <option value="lifetime">Lifetime (No Expiry)</option>
                        </select>
                    </div>
                    <div class="form-group">
                        <label>Notes / Client Name (Optional)</label>
                        <input type="text" id="notes" placeholder="e.g. Design Team">
                    </div>
                    <div>
                        <button type="submit" class="btn">Grant Access</button>
                    </div>
                </div>
            </form>
        </div>

        <div class="panel">
            <h2>Live System Updates & Client .EXE Synchronization</h2>
            <p style="color: var(--text-dim); font-size: 13px; margin-bottom: 16px;">
                When you deploy an update here, all running or newly launched MonarchCAD (.exe) client machines will automatically detect the new version and update their .exe files!
            </p>
            <form id="update-form" onsubmit="handlePublishUpdate(event)">
                <div class="form-row">
                    <div class="form-group">
                        <label>Target Version (e.g., 1.0.2)</label>
                        <input type="text" id="target-version" required placeholder="1.0.2">
                    </div>
                    <div class="form-group">
                        <label>Update Notes / Changelog</label>
                        <input type="text" id="update-notes" placeholder="New features & branding update">
                    </div>
                    <div class="form-group">
                        <label>Binary Status on Server</label>
                        <div id="binary-status" style="font-size: 13px; padding-top: 10px; color: #34d399; font-weight: 600;">Checking...</div>
                    </div>
                    <div>
                        <button type="submit" class="btn" style="background: linear-gradient(135deg, #10b981, #059669);">Push Update to All Clients</button>
                    </div>
                </div>
            </form>
        </div>

        <div class="panel">
            <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 12px;">
                <h2>All Licenses</h2>
                <input type="text" id="search" placeholder="Search email or notes..." oninput="filterLicenses()" style="width: 280px; padding: 8px 12px;">
            </div>
            <div style="overflow-x: auto;">
                <table>
                    <thead>
                        <tr>
                            <th>User Email</th>
                            <th>Live Status</th>
                            <th>License Type</th>
                            <th>Granted Date</th>
                            <th>Expiration</th>
                            <th>Time Left</th>
                            <th>Notes</th>
                            <th style="text-align: right;">Action</th>
                        </tr>
                    </thead>
                    <tbody id="licenses-table">
                        <tr><td colspan="8" style="text-align: center; color: var(--text-dim);">Loading licenses...</td></tr>
                    </tbody>
                </table>
            </div>
        </div>
    </div>

    <div id="toast"></div>

    <script>
        let allLicenses = [];

        async function loadLicenses() {
            try {
                const res = await fetch('/api/admin/licenses');
                allLicenses = await res.json();
                renderTable(allLicenses);
                updateStats(allLicenses);
            } catch (err) {
                console.error(err);
            }
        }

        function updateStats(data) {
            document.getElementById('stat-total').textContent = data.length;
            const online = data.filter(d => d.is_online).length;
            const activeSubscriptions = data.filter(d => !d.is_lifetime && d.status === 'active' && !d.is_expired).length;
            const lifetime = data.filter(d => d.is_lifetime && d.status === 'active').length;
            const expired = data.filter(d => d.status === 'revoked' || d.is_expired).length;

            document.getElementById('stat-online').textContent = online;
            document.getElementById('stat-active').textContent = activeSubscriptions;
            document.getElementById('stat-lifetime').textContent = lifetime;
            document.getElementById('stat-expired').textContent = expired;
        }

        function renderTable(data) {
            const tbody = document.getElementById('licenses-table');
            if (data.length === 0) {
                tbody.innerHTML = '<tr><td colspan="8" style="text-align: center; color: var(--text-dim); padding: 30px;">No licenses found. Use the form above to grant access.</td></tr>';
                return;
            }

            tbody.innerHTML = data.map(item => {
                let badge = '<span class="badge badge-active">Active</span>';
                if (item.status === 'revoked') {
                    badge = '<span class="badge badge-revoked">Revoked</span>';
                } else if (item.is_expired) {
                    badge = '<span class="badge badge-expired">Expired</span>';
                } else if (item.is_lifetime) {
                    badge = '<span class="badge badge-lifetime">Lifetime</span>';
                }

                const liveBadge = item.is_online
                    ? `<span class="badge badge-online"><span class="pulse-dot"></span> Online</span>`
                    : `<span class="badge badge-offline">Offline (${item.last_seen_text})</span>`;

                const expiryDisplay = item.is_lifetime ? '<span style="color:#818cf8; font-weight:600;">Never (Lifetime)</span>' : (item.expires_at || '-');

                let actionBtn = '';
                if (item.status === 'active') {
                    actionBtn = `<button class="btn-danger" onclick="revokeUser('${item.email}')">Revoke</button>`;
                } else {
                    actionBtn = `<button class="btn-success" onclick="activateUser('${item.email}')">Reactivate</button>`;
                }

                actionBtn += `<button class="btn-delete" onclick="deleteUser('${item.email}')">Delete</button>`;

                return `
                    <tr>
                        <td style="font-weight: 600;">${item.email}</td>
                        <td>${liveBadge}</td>
                        <td>${badge}</td>
                        <td style="color: var(--text-dim);">${item.granted_at}</td>
                        <td>${expiryDisplay}</td>
                        <td style="font-weight: 500;">${item.days_left}</td>
                        <td style="color: var(--text-dim);">${item.notes || '-'}</td>
                        <td style="text-align: right;">${actionBtn}</td>
                    </tr>
                `;
            }).join('');
        }

        function filterLicenses() {
            const q = document.getElementById('search').value.toLowerCase();
            const filtered = allLicenses.filter(l => l.email.toLowerCase().includes(q) || (l.notes && l.notes.toLowerCase().includes(q)));
            renderTable(filtered);
        }

        async function handleGrant(e) {
            e.preventDefault();
            const email = document.getElementById('email').value;
            const duration = document.getElementById('duration').value;
            const notes = document.getElementById('notes').value;

            const res = await fetch('/api/admin/grant', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ email, duration, notes })
            });
            const data = await res.json();
            showToast(data.message || 'Saved');
            document.getElementById('email').value = '';
            document.getElementById('notes').value = '';
            loadLicenses();
        }

        async function revokeUser(email) {
            if (!confirm(`Revoke access for ${email}? MonarchCAD will terminate on their machine.`)) return;
            const res = await fetch('/api/admin/revoke', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ email })
            });
            showToast(`Revoked access for ${email}`);
            loadLicenses();
        }

        async function activateUser(email) {
            const res = await fetch('/api/admin/activate', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ email })
            });
            showToast(`Reactivated access for ${email}`);
            loadLicenses();
        }

        async function deleteUser(email) {
            if (!confirm(`Permanently delete license record for ${email}?`)) return;
            const res = await fetch('/api/admin/delete', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ email })
            });
            showToast(`Deleted ${email}`);
            loadLicenses();
        }

        function showToast(msg) {
            const t = document.getElementById('toast');
            t.textContent = msg;
            t.style.display = 'block';
            setTimeout(() => { t.style.display = 'none'; }, 3000);
        }

        async function loadUpdateStatus() {
            try {
                const res = await fetch('/api/update/check?version=0');
                const data = await res.json();
                document.getElementById('target-version').value = data.latest_version;
                document.getElementById('update-notes').value = data.notes;
                const bs = document.getElementById('binary-status');
                if (data.file_size > 0) {
                    const mb = (data.file_size / (1024*1024)).toFixed(1);
                    bs.innerHTML = `Ready: ${mb} MB (${data.file_modified || 'Active'})`;
                    bs.style.color = '#34d399';
                } else {
                    bs.innerHTML = 'No .exe found in backend/updates or workspace';
                    bs.style.color = '#f87171';
                }
            } catch (e) {}
        }

        async function handlePublishUpdate(e) {
            e.preventDefault();
            const version = document.getElementById('target-version').value;
            const notes = document.getElementById('update-notes').value;
            const res = await fetch('/api/admin/update/publish', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ version, notes })
            });
            const data = await res.json();
            showToast(data.message || 'Update published!');
            loadUpdateStatus();
        }

        loadLicenses();
        loadUpdateStatus();
        setInterval(loadLicenses, 4000);
        setInterval(loadUpdateStatus, 15000);
    </script>
</body>
</html>
"""

# ---------------------------------------------------------------------------
# Request Handler
# ---------------------------------------------------------------------------

class MonarchHandler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        # Clean terminal output (safe against non-ASCII chars from network scanners)
        try:
            msg = f"[{time.strftime('%H:%M:%S')}] {args[0]} - {args[1]}\n"
            sys.stdout.write(msg.encode("ascii", errors="replace").decode("ascii"))
            sys.stdout.flush()
        except Exception:
            pass

    def handle_one_request(self):
        try:
            super().handle_one_request()
        except (ConnectionError, BrokenPipeError):
            pass

    def finish(self):
        try:
            super().finish()
        except (ConnectionError, BrokenPipeError):
            pass

    def send_json(self, status_code: int, data: dict):
        try:
            body = json.dumps(data).encode("utf-8")
            self.send_response(status_code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
            self.wfile.flush()
        except (ConnectionError, BrokenPipeError):
            pass

    def send_html(self, html_content: str):
        try:
            body = html_content.encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
            self.wfile.flush()
        except (ConnectionError, BrokenPipeError):
            pass

    def read_json_body(self):
        try:
            length = int(self.headers.get("Content-Length", 0))
            if length <= 0:
                return {}
            raw = self.rfile.read(length)
            return json.loads(raw.decode("utf-8"))
        except Exception:
            return {}

    def do_HEAD(self):
        self.do_GET(head_only=True)

    def do_GET(self, head_only: bool = False):
        parsed = urlparse(self.path)
        path = parsed.path
        query = parse_qs(parsed.query)

        if path == "/admin" or path == "/":
            self.send_html(ADMIN_HTML)

        elif path == "/api/admin/licenses":
            licenses = get_all_licenses()
            self.send_json(200, licenses)

        elif path == "/heartbeat":
            # If email query parameter is provided, verify active license
            email = query.get("email", [None])[0]
            if email:
                email_clean = email.strip().lower()
                valid, reason, info = verify_license(email_clean)
                if valid:
                    LAST_HEARTBEATS[email_clean] = time.time()
                    self.send_json(200, {"status": "ok", "valid": True, "info": info})
                else:
                    self.send_json(403, {"status": "error", "valid": False, "reason": reason})
            else:
                # Basic server alive check
                self.send_json(200, {"status": "ok", "valid": True})

        elif path == "/health":
            self.send_json(200, {"status": "healthy"})

        elif path.startswith("/tiles/") or path.startswith("/api/tiles/"):
            parts = path.strip("/").split("/")
            if len(parts) >= 4:
                try:
                    z = int(parts[-3])
                    x = int(parts[-2])
                    y_str = parts[-1].split(".")[0]
                    y = int(y_str)
                    provider = "satellite"
                    if len(parts) >= 5:
                        p_name = parts[-4].lower()
                        if p_name in ("satellite", "sat", "aerial"):
                            provider = "satellite"
                        elif p_name in ("hybrid", "hyb"):
                            provider = "hybrid"
                        elif p_name in ("osm", "street", "road"):
                            provider = "osm"

                    tiles_cache_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "tiles_cache")
                    os.makedirs(tiles_cache_dir, exist_ok=True)
                    tile_file = os.path.join(tiles_cache_dir, f"{provider}_{z}_{x}_{y}.png")

                    if not os.path.isfile(tile_file) or os.path.getsize(tile_file) < 100:
                        import urllib.request
                        if provider == "satellite":
                            urls_to_try = [
                                f"https://mt1.google.com/vt/lyrs=s&x={x}&y={y}&z={z}",
                                f"https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}",
                            ]
                        elif provider == "hybrid":
                            urls_to_try = [
                                f"https://mt1.google.com/vt/lyrs=y&x={x}&y={y}&z={z}",
                                f"https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}",
                            ]
                        else:
                            urls_to_try = [
                                f"https://tile.openstreetmap.org/{z}/{x}/{y}.png",
                            ]

                        downloaded_bytes = None
                        for candidate_url in urls_to_try:
                            try:
                                req = urllib.request.Request(
                                    candidate_url,
                                    headers={"User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64)"}
                                )
                                with urllib.request.urlopen(req, timeout=8) as resp:
                                    b = resp.read()
                                if len(b) > 200:
                                    downloaded_bytes = b
                                    with open(tile_file, "wb") as f:
                                        f.write(b)
                                    break
                            except Exception:
                                continue

                        if not downloaded_bytes:
                            self.send_json(404, {"error": "Tile not found"})
                            return
                        tile_bytes = downloaded_bytes
                    else:
                        with open(tile_file, "rb") as f:
                            tile_bytes = f.read()

                    self.send_response(200)
                    if tile_bytes.startswith(b"\xff\xd8\xff"):
                        self.send_header("Content-Type", "image/jpeg")
                    else:
                        self.send_header("Content-Type", "image/png")
                    self.send_header("Content-Length", str(len(tile_bytes)))
                    self.send_header("Cache-Control", "public, max-age=604800")
                    self.send_header("Connection", "close")
                    self.end_headers()
                    if not head_only:
                        self.wfile.write(tile_bytes)
                        self.wfile.flush()
                    return
                except Exception as e:
                    self.send_json(404, {"error": "Tile fetch error", "detail": str(e)})
                    return

        elif path == "/api/update/check":
            client_ver = query.get("version", ["1.0.0"])[0]
            info = get_update_status(client_ver)
            self.send_json(200, info)

        elif path in ("/download/zip", "/download/MonarchCAD.zip", "/api/update/download/zip", "/api/update/download.zip"):
            zip_path = os.path.join(UPDATES_DIR, "MonarchCAD.zip")
            if not os.path.isfile(zip_path):
                zip_path = ensure_zip_package()
            if not zip_path or not os.path.isfile(zip_path):
                self.send_json(404, {"error": "MonarchCAD.zip package not found on server"})
                return
            size = os.path.getsize(zip_path)
            self.send_response(200)
            self.send_header("Content-Type", "application/zip")
            self.send_header("Content-Disposition", 'attachment; filename="MonarchCAD.zip"')
            self.send_header("Content-Length", str(size))
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Connection", "close")
            self.end_headers()
            if not head_only:
                with open(zip_path, "rb") as f:
                    while True:
                        chunk = f.read(65536)
                        if not chunk:
                            break
                        try:
                            self.wfile.write(chunk)
                        except (ConnectionError, BrokenPipeError):
                            break
                try:
                    self.wfile.flush()
                except Exception:
                    pass

        elif path in ("/download/cer", "/download/certificate", "/api/update/certificate", "/download/MonarchCAD_Authority.cer"):
            cer_path = os.path.join(UPDATES_DIR, "MonarchCAD_Authority.cer")
            if not os.path.isfile(cer_path):
                self.send_json(404, {"error": "MonarchCAD certificate not found on server"})
                return
            size = os.path.getsize(cer_path)
            self.send_response(200)
            self.send_header("Content-Type", "application/x-x509-ca-cert")
            self.send_header("Content-Disposition", 'attachment; filename="MonarchCAD_Authority.cer"')
            self.send_header("Content-Length", str(size))
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Connection", "close")
            self.end_headers()
            if not head_only:
                with open(cer_path, "rb") as f:
                    while True:
                        chunk = f.read(65536)
                        if not chunk:
                            break
                        try:
                            self.wfile.write(chunk)
                        except (ConnectionError, BrokenPipeError):
                            break
                try:
                    self.wfile.flush()
                except Exception:
                    pass

        elif path in ("/download", "/download/exe", "/download/monarchcad.exe", "/api/update/download"):
            fmt = query.get("format", [""])[0].lower()
            if fmt == "zip":
                self.send_response(302)
                self.send_header("Location", "/download/zip")
                self.end_headers()
                return

            exe_path = get_latest_exe_path()
            if not exe_path or not os.path.isfile(exe_path):
                self.send_json(404, {"error": "Update binary not found on server"})
                return
            size = os.path.getsize(exe_path)
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Disposition", 'attachment; filename="monarchcad.exe"')
            self.send_header("Content-Length", str(size))
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Connection", "close")
            self.end_headers()
            if not head_only:
                with open(exe_path, "rb") as f:
                    while True:
                        chunk = f.read(65536)
                        if not chunk:
                            break
                        try:
                            self.wfile.write(chunk)
                        except (ConnectionError, BrokenPipeError):
                            break
                try:
                    self.wfile.flush()
                except Exception:
                    pass

        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        parsed = urlparse(self.path)
        path = parsed.path
        body = self.read_json_body()

        if path == "/api/license/verify":
            # Verification called by MonarchCAD (.exe) in-app login
            email = body.get("email", "")
            valid, reason, info = verify_license(email)
            if valid:
                self.send_json(200, {"valid": True, "message": reason, "license": info})
            else:
                self.send_json(401, {"valid": False, "message": reason})

        elif path == "/api/admin/update/publish":
            new_version = str(body.get("version", "")).strip()
            notes = str(body.get("notes", "")).strip()
            force = body.get("force", True)
            if not new_version:
                self.send_json(400, {"success": False, "message": "Version string required"})
                return
            set_setting("app_version", new_version)
            if notes:
                set_setting("update_notes", notes)
            set_setting("force_update", "1" if force else "0")

            exe_path = get_latest_exe_path()
            ensure_zip_package()
            self.send_json(200, {
                "success": True,
                "message": f"Version {new_version} published! All connected client .exe files will update.",
                "latest_version": new_version,
                "exe_path": exe_path
            })

        elif path == "/api/admin/grant":
            email = body.get("email", "")
            duration = str(body.get("duration", "30"))
            notes = body.get("notes", "")
            success, msg = grant_license(email, duration, notes)
            self.send_json(200 if success else 400, {"success": success, "message": msg})

        elif path == "/api/admin/revoke":
            email = body.get("email", "")
            success = revoke_license(email)
            self.send_json(200, {"success": success})

        elif path == "/api/admin/activate":
            email = body.get("email", "")
            success = activate_license(email)
            self.send_json(200, {"success": success})

        elif path == "/api/admin/delete":
            email = body.get("email", "")
            success = delete_license(email)
            self.send_json(200, {"success": success})

        else:
            self.send_response(404)
            self.end_headers()

class SilentServer(ThreadingHTTPServer):
    daemon_threads = True
    def handle_error(self, request, client_address):
        ex_type, _, _ = sys.exc_info()
        if ex_type and issubclass(ex_type, (ConnectionError, BrokenPipeError)):
            return
        super().handle_error(request, client_address)

# ---------------------------------------------------------------------------
# Main Entry Point
# ---------------------------------------------------------------------------

def main():
    init_db()
    port = PORT
    if len(sys.argv) > 1:
        try:
            port = int(sys.argv[1])
        except ValueError:
            print(f"Invalid port: {sys.argv[1]}, using default {PORT}")

    import socket
    local_ip = "127.0.0.1"
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect(("8.8.8.8", 80))
        local_ip = s.getsockname()[0]
        s.close()
    except Exception:
        pass

    server_address = ("0.0.0.0", port)
    httpd = SilentServer(server_address, MonarchHandler)

    print("=" * 70)
    print("  MonarchCAD Backend & License Server Running")
    print(f"  - Local Dashboard   : http://127.0.0.1:{port}/admin")
    print(f"  - Network Dashboard : http://{local_ip}:{port}/admin (for other PCs)")
    print(f"  - License API       : http://{local_ip}:{port}/api/license/verify")
    print(f"  - Heartbeat Check   : http://{local_ip}:{port}/heartbeat?email=<email>")
    print("  - Database          : SQLite (monarch_licenses.db)")
    print("  - Accessible from   : Local PC, LAN network, and Port-forwarded Internet")
    print("  - Press Ctrl+C to stop server (MonarchCAD will exit if stopped)")
    print("=" * 70)

    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\n[Backend] Shutting down server...")
    finally:
        httpd.server_close()
        print("[Backend] Server stopped.")

if __name__ == "__main__":
    main()
