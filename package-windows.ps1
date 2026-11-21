# HiWave Windows Packaging Script
$ErrorActionPreference = "Stop"

Write-Host "Building HiWave with RustKit engine..." -ForegroundColor Cyan
cargo build --release -p hiwave-app

if ($LASTEXITCODE -ne 0) {
    Write-Host "Build failed!" -ForegroundColor Red
    exit 1
}

Write-Host "Creating release package..." -ForegroundColor Cyan

$version = "0.1.0-alpha"
$packageName = "hiwave-windows-x64-$version"
$packageDir = "release\$packageName"

# Create package directory
New-Item -ItemType Directory -Force -Path $packageDir | Out-Null

# Copy binary
Copy-Item "target\release\hiwave.exe" -Destination "$packageDir\"

# Create README
@"
# HiWave Browser - Windows x64

Version: $version
Built: $(Get-Date -Format "yyyy-MM-dd")
Engine: RustKit (custom Rust browser engine)

## Quick Start

1. Double-click hiwave.exe
2. Start browsing!

## What Makes HiWave Different

- **Custom Engine**: Built from scratch in Rust, not Chromium
- **The Shelf**: Tabs decay and archive automatically
- **Privacy First**: No tracking, built-in ad blocking
- **Workspaces**: Separate work/personal/research contexts
- **13 Days**: Entire engine built in under 2 weeks

## System Requirements

- Windows 10/11 64-bit
- 4GB RAM recommended
- 500MB disk space

## Known Issues (Alpha)

- Some websites may not render correctly
- No extension support yet
- Windows only (macOS/Linux coming)
- Expect bugs - this is alpha software!

## Links

- GitHub: https://github.com/hiwavebrowser/hiwave-windows
- Report bugs: https://github.com/hiwavebrowser/hiwave-windows/issues

## License

GNU Affero General Public License v3.0 or later (AGPL-3.0-or-later)
Copyright (C) 2025-2026 Aleph Null LLC
Commercial licenses available - contact via GitHub

---

Built with ❤️ in Rust
"@ | Out-File -FilePath "$packageDir\README.txt" -Encoding UTF8

# Copy license
Copy-Item "LICENSE" -Destination "$packageDir\"

# Create ZIP
Write-Host "Creating ZIP archive..." -ForegroundColor Cyan
Compress-Archive -Path $packageDir -DestinationPath "release\$packageName.zip" -Force

$size = [math]::Round((Get-Item "release\$packageName.zip").Length / 1MB, 2)
Write-Host "`nPackage created successfully!" -ForegroundColor Green
Write-Host "Location: release\$packageName.zip" -ForegroundColor Cyan
Write-Host "Size: $size MB" -ForegroundColor Cyan
Write-Host "`nNext steps:" -ForegroundColor Yellow
Write-Host "1. Test the package by extracting and running hiwave.exe"
Write-Host "2. Upload to GitHub Releases"
Write-Host "3. Update download links in README.md"
