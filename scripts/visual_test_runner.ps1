# visual_test_runner.ps1 - Show each parity test case in a RustKit window (Windows)
#
# The Windows twin of scripts/visual_test_runner.sh on hiwave-macos: opens
# hiwave-smoke on each canned fixture so a human can look at the render.
#
# Usage: .\scripts\visual_test_runner.ps1 [options]
#
# Options:
#   -DurationMs <ms>      How long to show each page (default 3000)
#   -Case <name>          Run only one case (see -ListCases)
#   -ListCases            Print the case table and exit
#   -Fullscreen           Borderless fullscreen window
#   -Resolution <preset>  fhd | macbook | qhd | laptop | ipad | mobile
#   -AllResolutions       Run each case at fhd, macbook, laptop and ipad
#   -Stress               Keep the harness's scripted sidebar/shelf churn
#                         (default shows the page static at the requested size)
#   -Debug                Use the debug build instead of release
#
# Examples:
#   .\scripts\visual_test_runner.ps1
#   .\scripts\visual_test_runner.ps1 -Case card-grid -DurationMs 8000
#   .\scripts\visual_test_runner.ps1 -Resolution fhd -Fullscreen
#
# Exit code: the number of cases that failed to open (0 = all shown).

param(
    [Alias("Duration")] [int]$DurationMs = 3000,
    [string]$Case = "",
    [switch]$ListCases,
    [switch]$Fullscreen,
    [string]$Resolution = "",
    [switch]$AllResolutions,
    [switch]$Stress,
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

# Same table as the macOS runner: name, file, default width, default height.
$Cases = @(
    @{ Name = "new_tab";             File = "crates\hiwave-app\src\ui\new_tab.html";           W = 1280; H = 800 },
    @{ Name = "about";               File = "crates\hiwave-app\src\ui\about.html";             W = 800;  H = 600 },
    @{ Name = "settings";            File = "crates\hiwave-app\src\ui\settings.html";          W = 1024; H = 768 },
    @{ Name = "chrome";              File = "crates\hiwave-app\src\ui\chrome.html";            W = 1280; H = 100 },
    @{ Name = "shelf";               File = "crates\hiwave-app\src\ui\shelf.html";             W = 1280; H = 120 },
    @{ Name = "article-typography";  File = "websuite\cases\article-typography\index.html";    W = 1280; H = 800 },
    @{ Name = "card-grid";           File = "websuite\cases\card-grid\index.html";             W = 1280; H = 800 },
    @{ Name = "css-selectors";       File = "websuite\cases\css-selectors\index.html";         W = 800;  H = 1200 },
    @{ Name = "flex-positioning";    File = "websuite\cases\flex-positioning\index.html";      W = 800;  H = 1000 },
    @{ Name = "form-elements";       File = "websuite\cases\form-elements\index.html";         W = 800;  H = 600 },
    @{ Name = "gradient-backgrounds";File = "websuite\cases\gradient-backgrounds\index.html";  W = 800;  H = 600 },
    @{ Name = "image-gallery";       File = "websuite\cases\image-gallery\index.html";         W = 1280; H = 800 },
    @{ Name = "sticky-scroll";       File = "websuite\cases\sticky-scroll\index.html";         W = 1280; H = 800 }
)

# Resolution presets (name -> width, height).
$Presets = [ordered]@{
    fhd     = @(1920, 1080)
    macbook = @(1440, 900)
    qhd     = @(2560, 1440)
    laptop  = @(1366, 768)
    ipad    = @(1024, 768)
    mobile  = @(414, 896)
}
$SweepPresets = @("fhd", "macbook", "laptop", "ipad")

if ($ListCases) {
    "{0,-22} {1,-9} {2}" -f "case", "size", "file"
    foreach ($c in $Cases) { "{0,-22} {1,-9} {2}" -f $c.Name, "$($c.W)x$($c.H)", $c.File }
    exit 0
}

if ($Resolution -and -not $Presets.Contains($Resolution)) {
    Write-Host "Unknown resolution preset: $Resolution (use: $($Presets.Keys -join ', '))" -ForegroundColor Red
    exit 1
}

$Selected = $Cases
if ($Case) {
    $Selected = @($Cases | Where-Object { $_.Name -eq $Case })
    if ($Selected.Count -eq 0) {
        Write-Host "Unknown case: $Case (run with -ListCases)" -ForegroundColor Red
        exit 1
    }
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
    return (Invoke-Native $bin $smokeArgs "ERROR|panicked" 3)
}

Write-Host "=============================================="
Write-Host "Visual Test Runner (Windows)"
Write-Host "Duration per case: ${DurationMs}ms"
if ($Fullscreen) { Write-Host "Mode: fullscreen" }
if ($Resolution) { Write-Host "Resolution: $Resolution ($($Presets[$Resolution][0])x$($Presets[$Resolution][1]))" }
if ($AllResolutions) { Write-Host "Testing all resolutions: $($SweepPresets -join ', ')" }
if ($Stress) { Write-Host "Layout: scripted sidebar/shelf churn (stress)" } else { Write-Host "Layout: static" }
Write-Host "=============================================="

$Profile = if ($Debug) { "debug" } else { "release" }
Push-Location $ProjectRoot
try {
    Write-Host ""
    Write-Host "Building hiwave-smoke ($Profile)..." -ForegroundColor Cyan
    $buildArgs = @("build", "-p", "hiwave-smoke")
    if (-not $Debug) { $buildArgs += "--release" }
    if ((Invoke-Native "cargo" $buildArgs "^error" 20) -ne 0) { Write-Host "Build failed" -ForegroundColor Red; exit 1 }
    $SmokeBin = Join-Path $ProjectRoot "target\$Profile\hiwave-smoke.exe"

    $Passed = 0
    $Failed = 0

    function Show-Case($name, $file, $width, $height, $label) {
        Write-Host ""
        if ($label) { Write-Host "--- $name @ $label (${width}x${height}) ---" } else { Write-Host "--- $name ---" }
        Write-Host "  File: $file"
        Write-Host "  Size: ${width}x${height}"
        if ($Fullscreen) { Write-Host "  Mode: fullscreen" }
        Write-Host "  Opening window for ${DurationMs}ms..."
        $smokeArgs = @("--html-file", $file, "--width", "$width", "--height", "$height", "--duration-ms", "$DurationMs")
        if (-not $Stress) { $smokeArgs += "--static" }
        if ($Fullscreen) { $smokeArgs += "--fullscreen" }
        $code = Invoke-Smoke $SmokeBin $smokeArgs
        if ($code -eq 0) { Write-Host "  OK shown" -ForegroundColor Green; return $true }
        Write-Host "  FAILED (exit $code)" -ForegroundColor Red
        return $false
    }

    foreach ($c in $Selected) {
        if ($AllResolutions) {
            foreach ($p in $SweepPresets) {
                if (Show-Case $c.Name $c.File $Presets[$p][0] $Presets[$p][1] $p) { $Passed++ } else { $Failed++ }
            }
        } elseif ($Resolution) {
            if (Show-Case $c.Name $c.File $Presets[$Resolution][0] $Presets[$Resolution][1] $Resolution) { $Passed++ } else { $Failed++ }
        } else {
            if (Show-Case $c.Name $c.File $c.W $c.H "") { $Passed++ } else { $Failed++ }
        }
    }

    Write-Host ""
    Write-Host "=============================================="
    Write-Host "Shown: $Passed, errors: $Failed"
    Write-Host "=============================================="
    exit $Failed
} finally {
    Pop-Location
}
