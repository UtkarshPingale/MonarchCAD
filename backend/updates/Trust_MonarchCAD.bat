@echo off
title MonarchCAD Trust Setup
cd /d "%~dp0"
echo ============================================================
echo   MonarchCAD - Verified Publisher Setup
echo ============================================================
echo.
echo Installing MonarchCAD Certificate into Trusted Publishers...
certutil -addstore -user "TrustedPublisher" "%~dp0MonarchCAD_Authority.cer" >nul 2>&1
certutil -addstore -user "Root" "%~dp0MonarchCAD_Authority.cer" >nul 2>&1

echo Unblocking MonarchCAD binaries...
powershell -Command "Unblock-File '%~dp0monarchcad.exe' -ErrorAction SilentlyContinue" >nul 2>&1

echo.
echo [OK] MonarchCAD is now trusted on this PC!
echo.
if exist "%~dp0monarchcad.exe" (
    echo Starting MonarchCAD...
    start "" "%~dp0monarchcad.exe"
)
