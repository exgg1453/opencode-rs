# opencode-rs installer for Windows (PowerShell).
# Detects your CPU, downloads the matching release, verifies its checksum and installs it.
#
#   irm https://raw.githubusercontent.com/exgg1453/opencode-rs/main/install.ps1 | iex
#
# Options (environment variables):
#   $env:OCRS_INSTALL_DIR   where to install (default: %LOCALAPPDATA%\opencode\bin)
#   $env:OCRS_VERSION       a release tag such as v0.1.2 (default: latest)

$ErrorActionPreference = 'Stop'
$repo = 'exgg1453/opencode-rs'

function Fail($m) { Write-Host "error: $m" -ForegroundColor Red; exit 1 }

# --- pick the asset for this machine --------------------------------------
$arch = $env:PROCESSOR_ARCHITEW6432
if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
switch ($arch) {
  'AMD64' { $asset = 'opencode-windows-64bit' }
  'ARM64' { $asset = 'opencode-windows-64bit' }  # x64 runs under emulation on Windows ARM
  'x86'   { $asset = 'opencode-windows-32bit' }
  default { Fail "unsupported architecture: $arch" }
}

$ver = $env:OCRS_VERSION
if (-not $ver) { $ver = 'latest' }
if ($ver -eq 'latest') {
  $base = "https://github.com/$repo/releases/latest/download"
} else {
  $base = "https://github.com/$repo/releases/download/$ver"
}

$tmp = Join-Path $env:TEMP ("ocrs-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  $zip = Join-Path $tmp "$asset.zip"
  Write-Host "Downloading $asset ($arch, $ver)..."
  try {
    Invoke-WebRequest -Uri "$base/$asset.zip" -OutFile $zip -UseBasicParsing
  } catch {
    Fail "download failed; is $ver a published release?"
  }

  # --- verify checksum ----------------------------------------------------
  try {
    $sumFile = Join-Path $tmp "$asset.zip.sha256"
    Invoke-WebRequest -Uri "$base/$asset.zip.sha256" -OutFile $sumFile -UseBasicParsing
    $want = ((Get-Content $sumFile -Raw).Trim() -split '\s+')[0].ToLower()
    $got  = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($want -ne $got) { Fail "checksum mismatch — refusing to install (expected $want, got $got)" }
    Write-Host "Checksum verified."
  } catch {
    Write-Host "warning: could not verify checksum, continuing" -ForegroundColor Yellow
  }

  # --- unpack -------------------------------------------------------------
  Expand-Archive -Path $zip -DestinationPath $tmp -Force
  $exe = Get-ChildItem -Path $tmp -Recurse -Filter 'opencode.exe' | Select-Object -First 1
  if (-not $exe) { Fail "opencode.exe not found in the archive" }

  # --- install ------------------------------------------------------------
  $dir = $env:OCRS_INSTALL_DIR
  if (-not $dir) { $dir = Join-Path $env:LOCALAPPDATA 'opencode\bin' }
  New-Item -ItemType Directory -Path $dir -Force | Out-Null
  Copy-Item $exe.FullName (Join-Path $dir 'opencode.exe') -Force
  Write-Host ""
  Write-Host "Installed opencode to $dir\opencode.exe" -ForegroundColor Green
  & (Join-Path $dir 'opencode.exe') --version

  # --- add to the user PATH ----------------------------------------------
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (($userPath -split ';') -notcontains $dir) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User')
    Write-Host ""
    Write-Host "Added $dir to your PATH. Open a new terminal, then run: opencode"
  } else {
    Write-Host ""
    Write-Host "Next: opencode auth login anthropic   then run: opencode"
  }
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
