# The Windows build's smoke tests, run by release.yml straight after `tauri
# build`, from the repository root, as `pwsh -File`. $env:VERSION is the
# version the tag names; $env:RUNNER_TEMP is the runner's.
#
# The release exe is a GUI-subsystem program: PowerShell's `&` would not wait
# for it, and it has no console of its own. Start-Process -Wait gives the exit
# code, a redirect gives it a stdout handle (cli.rs attaches to a console only
# when it has none), and self-test.txt is the record anyway.
#
# Each check is a collapsed group in the log. A failure becomes an annotation
# naming the check, since job logs need a signed-in viewer and annotations do
# not.

# `pwsh -File` runs without the preamble Actions gives a `shell: pwsh` step,
# so a failing cmdlet has to be made to stop the script here.
$ErrorActionPreference = 'Stop'
$check = ''
trap {
    # ${check}, not $check: a colon straight after a variable name in a
    # string is read as a scope qualifier (`$env:VERSION`) and fails to parse.
    Write-Output "::error title=Smoke test failed: ${check}::$_"
    exit 1
}

function Begin-Check([string]$Name) {
    $script:check = $Name
    Write-Output "::group::$Name"
}

function End-Check { Write-Output '::endgroup::' }

# What `<exe> --version` printed, its exit code and its line checked.
function Assert-Version([string]$Exe, [string]$Who) {
    $out = Join-Path $env:RUNNER_TEMP "version-$([guid]::NewGuid()).txt"
    $p = Start-Process -FilePath $Exe -ArgumentList '--version' -Wait -PassThru -NoNewWindow -RedirectStandardOutput $out
    # Get-Content -Raw returns $null for an empty file, and $null.Trim()
    # throws something unreadable; the string cast keeps it a string.
    $line = "$(Get-Content $out -Raw)".Trim()
    Write-Output $line
    if ($p.ExitCode -ne 0) { throw "$Who --version exited with $($p.ExitCode)" }
    if (-not $line) { throw "$Who --version printed nothing" }
    if ($line -ne "mytube $env:VERSION") { throw "$Who`: expected 'mytube $env:VERSION', got '$line'" }
}

Begin-Check 'version'
$exe = Resolve-Path 'src-tauri\target\release\mytube.exe'
Assert-Version $exe 'mytube'
End-Check

# The real first-run path: yt-dlp, ffmpeg and deno downloaded fresh from their
# publishers, each checksum verified, each one run. The report is printed
# whatever the outcome.
Begin-Check 'self-test'
$dir = Join-Path $env:RUNNER_TEMP 'tools'
$out = Join-Path $env:RUNNER_TEMP 'self-test-stdout.txt'
# Start-Process joins ArgumentList with spaces and quotes nothing.
$p = Start-Process -FilePath $exe -ArgumentList '--self-test', "`"$dir`"" -Wait -PassThru -NoNewWindow -RedirectStandardOutput $out
$report = Join-Path $dir 'self-test.txt'
Write-Output "--- $report"
if (Test-Path $report) { Get-Content $report } else { Write-Output '::error::self-test.txt was not written'; Get-Content $out -ErrorAction SilentlyContinue }
if ($p.ExitCode -ne 0) { throw "mytube --self-test exited with $($p.ExitCode)" }
End-Check

Begin-Check 'installer'
$setup = Get-ChildItem "src-tauri\target\release\bundle\nsis\mytube_$($env:VERSION)_*-setup.exe" | Select-Object -First 1
if (-not $setup) { throw "no NSIS installer for $env:VERSION" }
$p = Start-Process -FilePath $setup.FullName -ArgumentList '/S' -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "installer exited with $($p.ExitCode)" }
# installMode currentUser: Tauri's NSIS template installs to $LOCALAPPDATA\<productName>.
$installed = Join-Path $env:LOCALAPPDATA 'mytube\mytube.exe'
if (-not (Test-Path $installed)) { throw "installer did not install $installed" }
Assert-Version $installed 'installed mytube'
End-Check
