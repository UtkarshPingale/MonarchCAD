param(
    [string[]]$Files = @("monarchcad.exe", "MonarchCAD_Latest.exe", "backend/updates/monarchcad.exe", "S:/Devloper/monarchcad.exe")
)

$ErrorActionPreference = "Stop"

Write-Host "=== MonarchCAD Authenticode Code Signing ===" -ForegroundColor Cyan

# 1. Locate or create MonarchCAD Code Signing Certificate
$subject = "CN=MonarchCAD, O=MonarchCAD, OU=Engineering, C=US"
$existingCert = Get-ChildItem -Path Cert:\CurrentUser\My | Where-Object { $_.Subject -match "CN=MonarchCAD" } | Select-Object -First 1

if (-not $existingCert) {
    Write-Host "[1/4] Generating new MonarchCAD Code Signing Certificate..." -ForegroundColor Yellow
    $cert = New-SelfSignedCertificate `
        -Type CodeSigningCert `
        -Subject $subject `
        -KeySpec Signature `
        -KeyLength 2048 `
        -KeyExportPolicy Exportable `
        -HashAlgorithm SHA256 `
        -CertStoreLocation Cert:\CurrentUser\My `
        -NotAfter (Get-Date).AddYears(15)
} else {
    Write-Host "[1/4] Found existing MonarchCAD Certificate: $($existingCert.Thumbprint)" -ForegroundColor Green
    $cert = $existingCert
}

# 2. Add certificate to CurrentUser\TrustedPublisher store (no interactive popup)
Write-Host "[2/4] Registering MonarchCAD in TrustedPublisher store..." -ForegroundColor Yellow
$pubStore = New-Object System.Security.Cryptography.X509Certificates.X509Store "TrustedPublisher", "CurrentUser"
$pubStore.Open([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite)
$alreadyInPub = $pubStore.Certificates | Where-Object { $_.Thumbprint -eq $cert.Thumbprint }
if (-not $alreadyInPub) {
    $pubStore.Add($cert)
    Write-Host "      Installed to CurrentUser\TrustedPublisher." -ForegroundColor Green
} else {
    Write-Host "      Already in CurrentUser\TrustedPublisher." -ForegroundColor Green
}
$pubStore.Close()

# 3. Export public .cer file for distribution to other machines
$distDir = "backend/updates"
if (-not (Test-Path $distDir)) {
    New-Item -ItemType Directory -Path $distDir -Force | Out-Null
}
$cerPath = Join-Path $distDir "MonarchCAD_Authority.cer"
$certBytes = $cert.Export([System.Security.Cryptography.X509Certificates.X509ContentType]::Cert)
[System.IO.File]::WriteAllBytes($cerPath, $certBytes)
Write-Host "[3/4] Exported public certificate for distribution to: $cerPath" -ForegroundColor Green

# 4. Sign target files
Write-Host "[4/4] Signing executables..." -ForegroundColor Yellow
foreach ($file in $Files) {
    if (Test-Path $file) {
        $resolved = (Resolve-Path $file).Path
        Write-Host "      Signing: $resolved ... " -NoNewline
        try {
            $sig = Set-AuthenticodeSignature -FilePath $resolved -Certificate $cert -HashAlgorithm SHA256 -TimestampServer "http://timestamp.digicert.com"
            if ($sig.Status -eq "Valid") {
                Write-Host "VALID (Signed & Timestamped)" -ForegroundColor Green
            } else {
                # Fallback without timestamp server if network timeout
                $sig = Set-AuthenticodeSignature -FilePath $resolved -Certificate $cert -HashAlgorithm SHA256
                Write-Host "SIGNED ($($sig.Status))" -ForegroundColor Cyan
            }
        } catch {
            Write-Host "FAILED: $_" -ForegroundColor Red
        }
    } else {
        Write-Host "      File not found: $file (skipped)" -ForegroundColor DarkGray
    }
}

Write-Host "=== Signing Complete ===" -ForegroundColor Green
