# visual_live_runner.ps1 - Show LIVE websites in a RustKit window (Windows)
#
# The Windows twin of scripts/visual_live_runner.sh on hiwave-macos: the
# live-site counterpart of visual_test_runner.ps1, which shows canned fixtures.
#
# Usage: .\scripts\visual_live_runner.ps1 [options]
#
# Options:
#   -Site <id>            One site from websuite\realsite-top20.json (e.g. wikipedia)
#   -Url <url>            Any URL (overrides -Site / the list)
#   -List                 Print the site ids and exit
#   -DurationMs <ms>      How long to show each page (default 10000)
#   -Resolution <preset>  fhd | macbook | laptop | ipad (default 1280x800, the board viewport)
#   -Compare              Also open Chrome beside it at the same size
#   -Fullscreen           RustKit window fullscreen (ignored with -Compare)
#   -Debug                Use the debug build instead of release
#
# Examples:
#   .\scripts\visual_live_runner.ps1                          # all 20 board sites
#   .\scripts\visual_live_runner.ps1 -Site wikipedia -Compare
#   .\scripts\visual_live_runner.ps1 -Url https://news.ycombinator.com -DurationMs 20000
#
# Chrome for -Compare: $env:PARITY_CHROME_PATH, else a pinned Chrome for
# Testing under .browsers\ (chrome\win64-*\chrome-win64\chrome.exe), else the
# installed Google Chrome. The installed Chrome is not the pinned parity
# oracle version, so treat a -Compare view as a visual aid, not a board number.
#
# Exit code: the number of sites that failed to open (0 = all shown).

param(
    [string]$Site = "",
    [string]$Url = "",
    [switch]$List,
    [Alias("Duration")] [int]$DurationMs = 10000,
    [string]$Resolution = "",
    [switch]$Compare,
    [switch]$Fullscreen,
    [switch]$Debug,
    [switch]$Help
)

$ErrorActionPreference = "Stop"
if ($Help) {
    # The comment block at the top of this file is the usage text.
    Get-Content $MyInvocation.MyCommand.Path | Select-Object -Skip 1 |
        ForEach-Object { if ($_ -match "^#") { $_ -replace "^# ?", "" } else { break } } | Write-Host
    exit 0
}
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir
$ListPath = Join-Path $ProjectRoot "websuite\realsite-top20.json"

$Presets = [ordered]@{
    fhd     = @(1920, 1080)
    macbook = @(1440, 900)
    laptop  = @(1366, 768)
    ipad    = @(1024, 768)
}
$Width = 1280
$Height = 800
if ($Resolution) {
    if (-not $Presets.Contains($Resolution)) {
        Write-Host "Unknown resolution preset: $Resolution (use: $($Presets.Keys -join ', '))" -ForegroundColor Red
        exit 1
    }
    $Width = $Presets[$Resolution][0]
    $Height = $Presets[$Resolution][1]
}

$Sites = (Get-Content -Raw -Encoding UTF8 $ListPath | ConvertFrom-Json).sites

if ($List) {
    foreach ($s in $Sites) { "{0,-12} {1}" -f $s.id, $s.url }
    exit 0
}

# Build the list of (id, url) targets.
$Targets = @()
if ($Url) {
    $Targets += @{ Id = "custom"; Url = $Url }
} elseif ($Site) {
    $match = @($Sites | Where-Object { $_.id -eq $Site })
    if ($match.Count -eq 0) { Write-Host "Unknown site: $Site (use -List)" -ForegroundColor Red; exit 1 }
    $Targets += @{ Id = $match[0].id; Url = $match[0].url }
} else {
    foreach ($s in $Sites) { $Targets += @{ Id = $s.id; Url = $s.url } }
}

# Locate Chrome for -Compare.
$Chrome = $null
if ($Compare) {
    $Chrome = $env:PARITY_CHROME_PATH
    if (-not $Chrome) {
        foreach ($root in @((Join-Path $ProjectRoot ".browsers"), (Join-Path (Split-Path -Parent $ProjectRoot) ".browsers"))) {
            $cand = Get-ChildItem -Path (Join-Path $root "chrome") -Directory -Filter "win64-*" -ErrorAction SilentlyContinue |
                Sort-Object Name -Descending |
                ForEach-Object { Join-Path $_.FullName "chrome-win64\chrome.exe" } |
                Where-Object { Test-Path $_ } | Select-Object -First 1
            if ($cand) { $Chrome = $cand; break }
        }
    }
    if (-not $Chrome) {
        foreach ($cand in @("$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
                            "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe",
                            "$env:LOCALAPPDATA\Google\Chrome\Application\chrome.exe")) {
            if (Test-Path $cand) { $Chrome = $cand; break }
        }
    }
    if (-not $Chrome -or -not (Test-Path $Chrome)) {
        Write-Host "-Compare: no Chrome found (set PARITY_CHROME_PATH)" -ForegroundColor Red
        exit 1
    }
    $Fullscreen = $false
}

# Run a native command with both streams captured to files. On PowerShell 5.1
# a native command's stderr that reaches the pipeline becomes ErrorRecords,
# which terminate the script under $ErrorActionPreference = "Stop" (cargo's
# warnings, hiwave-smoke's tracing output). Returns the exit code and echoes
# the lines matching $pattern (at most $limit).
function Invoke-Native([string]$exe, [string[]]$nativeArgs, [string]$pattern, [int]$limit = 3) {
    $out = [System.IO.Path]::GetTempFileName()
    $err = [System.IO.Path]::GetTempFileName()
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $exe @nativeArgs 1> $out 2> $err
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
    }
    Get-Content $out, $err -ErrorAction SilentlyContinue |
        Where-Object { $_ -match $pattern } |
        Select-Object -First $limit |
        ForEach-Object { Write-Host "  $_" -ForegroundColor Yellow }
    Remove-Item $out, $err -Force -ErrorAction SilentlyContinue
    return $code
}

function Invoke-Smoke([string]$bin, [string[]]$smokeArgs) {
    return (Invoke-Native $bin $smokeArgs "ERROR|Failed|panicked" 3)
}

Write-Host "=============================================="
Write-Host "Visual Live Runner (Windows) - $($Targets.Count) site(s), ${Width}x${Height}, ${DurationMs}ms each"
if ($Compare) { Write-Host "Comparing against: $Chrome" }
Write-Host "=============================================="

$Profile = if ($Debug) { "debug" } else { "release" }
Push-Location $ProjectRoot
try {
    Write-Host "Building hiwave-smoke ($Profile)..." -ForegroundColor Cyan
    $buildArgs = @("build", "-p", "hiwave-smoke")
    if (-not $Debug) { $buildArgs += "--release" }
    if ((Invoke-Native "cargo" $buildArgs "^error" 20) -ne 0) { Write-Host "Build failed" -ForegroundColor Red; exit 1 }
    $SmokeBin = Join-Path $ProjectRoot "target\$Profile\hiwave-smoke.exe"

    $Shown = 0
    $Failed = 0
    foreach ($t in $Targets) {
        Write-Host ""
        Write-Host "--- $($t.Id) - $($t.Url) ---"
        $profileDir = $null
        if ($Compare) {
            $profileDir = Join-Path $env:TEMP ("visual-live-chrome-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
            New-Item -ItemType Directory -Force -Path $profileDir | Out-Null
            $chromeArgs = @("--user-data-dir=$profileDir", "--no-first-run", "--no-default-browser-check",
                            "--lang=en-US", "--force-color-profile=srgb",
                            "--window-size=$Width,$Height", "--window-position=$($Width + 20),40",
                            "--app=$($t.Url)")
            Start-Process -FilePath $Chrome -ArgumentList $chromeArgs | Out-Null
        }
        $smokeArgs = @("--url", $t.Url, "--width", "$Width", "--height", "$Height", "--duration-ms", "$DurationMs")
        if ($Fullscreen) { $smokeArgs += "--fullscreen" }
        $status = Invoke-Smoke $SmokeBin $smokeArgs
        if ($profileDir) {
            # --app spawns a browser process tree; stop everything rooted in this profile.
            Get-CimInstance Win32_Process -Filter "Name = 'chrome.exe'" |
                Where-Object { $_.CommandLine -like "*$profileDir*" } |
                ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
            Start-Sleep -Milliseconds 300
            Remove-Item -Recurse -Force -Path $profileDir -ErrorAction SilentlyContinue
        }
        if ($status -eq 0) { Write-Host "  OK shown" -ForegroundColor Green; $Shown++ }
        else { Write-Host "  FAILED (exit $status)" -ForegroundColor Red; $Failed++ }
    }

    Write-Host ""
    Write-Host "=============================================="
    Write-Host "Shown: $Shown, errors: $Failed"
    Write-Host "=============================================="
    exit $Failed
} finally {
    Pop-Location
}
