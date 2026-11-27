# Ambiente di compilazione per app\ (fork di Handy) su Windows con Visual Studio Build Tools.
# Visual Studio 18 Build Tools nel percorso standard; transcribe.cpp precompilato in $env:TCPP_ROOT\install
# (default %LOCALAPPDATA%\tcpp\install, la stessa radice di tools\build-transcribe-cpp.ps1).
# Uso:  powershell -NoProfile -ExecutionPolicy Bypass -File tools\build-app.ps1 [-Command "<comando>"] [-WorkDir <cartella>]
# Default: compila l'eseguibile di debug senza bundle in app\src-tauri\target\debug\eaglevoice.exe.
# Perché esiste: il CMake di transcribe-cpp con il generatore Visual Studio fallisce su VS 2026
# (MSB3491 in VCTargetsPath.vcxproj); con Ninja dentro la Developer Shell funziona.
param(
    [string]$Command = "bun run tauri build --debug --no-bundle",
    [string]$WorkDir = ""
)

$ErrorActionPreference = "Continue"
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
if ($WorkDir -eq "") { $WorkDir = Join-Path $root "app" }

$vs = "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools"
$devShell = Join-Path $vs "Common7\Tools\Launch-VsDevShell.ps1"
if (-not (Test-Path $devShell)) { Write-Error "Developer Shell non trovata: $devShell"; exit 2 }
# vswhere.exe sta nella cartella dell'installer: senza questa riga Launch-VsDevShell stampa un avviso innocuo
$env:Path = "C:\Program Files (x86)\Microsoft Visual Studio\Installer;" + $env:Path
& $devShell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation | Out-Null

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:APPDATA\npm;" +
    (Join-Path $vs "Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin") + ";" +
    (Join-Path $vs "Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja") + ";" + $env:Path
$env:CMAKE_GENERATOR = "Ninja"
$env:CARGO_INCREMENTAL = "1"

# transcribe.cpp precompilato da tools\build-transcribe-cpp.ps1: cargo salta la build C++
# (su alcune macchine la junction corta della crate non funziona).
$tcppRoot = if ($env:TCPP_ROOT) { $env:TCPP_ROOT } else { Join-Path $env:LOCALAPPDATA "tcpp" }
$tcpp = Join-Path $tcppRoot "install"
if (Test-Path (Join-Path $tcpp "lib\transcribe-link.json")) {
    $env:TRANSCRIBE_DIR = $tcpp
    Write-Output "[build-app] TRANSCRIBE_DIR=$tcpp"
} else {
    Write-Output "[build-app] ATTENZIONE: $tcpp non trovato, cargo compilerà transcribe.cpp da solo (può fallire per la junction)"
}

Set-Location $WorkDir
Write-Output "[build-app] cartella: $WorkDir"
Write-Output "[build-app] cmake: $((Get-Command cmake).Source)  ninja: $((Get-Command ninja).Source)  cl: $((Get-Command cl -ErrorAction SilentlyContinue).Source)"
Write-Output "[build-app] comando: $Command"
cmd /c "$Command 2>&1"
$code = $LASTEXITCODE
Write-Output "[build-app] EXIT=$code"
exit $code
