# Compila una volta transcribe.cpp (la libreria nativa di Handy, sorgente vendorizzato nella crate
# transcribe-cpp-sys scaricata da crates.io) in <radice>\build e la installa in <radice>\install.
# <radice> = parametro -Root, altrimenti la variabile d'ambiente TCPP_ROOT, altrimenti %LOCALAPPDATA%\tcpp.
# Conviene una radice corta (es. setx TCPP_ROOT D:\tcpp). Visual Studio 18 Build Tools nel percorso standard.
# Poi tools\build-app.ps1 imposta TRANSCRIBE_DIR su quel prefisso e cargo salta la compilazione C++.
# Perché: su alcune macchine la junction corta che la crate crea in %LOCALAPPDATA%\tcs non è attraversabile
# (CMake e Ninja non trovano i file scritti lì), quindi la build dentro cargo fallisce.
# Uso: powershell -NoProfile -ExecutionPolicy Bypass -File tools\build-transcribe-cpp.ps1 [-Version 0.4.0] [-Root <cartella>]
param(
    [string]$Version = "0.4.0",
    [string]$Root = "",
    [string]$Prefix = "",
    [string]$BuildDir = ""
)
if ($Root -eq "") { $Root = if ($env:TCPP_ROOT) { $env:TCPP_ROOT } else { Join-Path $env:LOCALAPPDATA "tcpp" } }
if ($Prefix -eq "") { $Prefix = Join-Path $Root "install" }
if ($BuildDir -eq "") { $BuildDir = Join-Path $Root "build" }
$ErrorActionPreference = "Continue"
$vs = "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools"
& (Join-Path $vs "Common7\Tools\Launch-VsDevShell.ps1") -Arch amd64 -HostArch amd64 -SkipAutomaticLocation | Out-Null
$env:Path = (Join-Path $vs "Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin") + ";" +
    (Join-Path $vs "Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja") + ";" + $env:Path

$src = Get-ChildItem "$env:USERPROFILE\.cargo\registry\src\*\transcribe-cpp-sys-$Version" -Directory | Select-Object -First 1
if (-not $src) { Write-Error "sorgente transcribe-cpp-sys-$Version non trovato nel registro cargo (lancia prima un cargo fetch in app\src-tauri)"; exit 2 }
Write-Output "[tcpp] sorgente: $($src.FullName)"
New-Item -ItemType Directory -Force $BuildDir | Out-Null
New-Item -ItemType Directory -Force $Prefix | Out-Null

$t0 = Get-Date
cmake -S $src.FullName -B $BuildDir -G Ninja -DCMAKE_BUILD_TYPE=Release `
    -DTRANSCRIBE_INSTALL=ON -DTRANSCRIBE_BUILD_TESTS=OFF -DTRANSCRIBE_BUILD_EXAMPLES=OFF `
    -DTRANSCRIBE_BUILD_TOOLS=OFF -DTRANSCRIBE_BUILD_SHARED=ON -DTRANSCRIBE_GGML_BACKEND_DL=ON `
    -DGGML_CPU_ALL_VARIANTS=ON -DTRANSCRIBE_X86_CONSERVATIVE=ON -DTRANSCRIBE_USE_OPENMP=OFF `
    "-DCMAKE_INSTALL_PREFIX=$Prefix"
if ($LASTEXITCODE -ne 0) { Write-Output "[tcpp] configure fallita EXIT=$LASTEXITCODE"; exit $LASTEXITCODE }
cmake --build $BuildDir --parallel
if ($LASTEXITCODE -ne 0) { Write-Output "[tcpp] build fallita EXIT=$LASTEXITCODE"; exit $LASTEXITCODE }
cmake --install $BuildDir --prefix $Prefix
if ($LASTEXITCODE -ne 0) { Write-Output "[tcpp] install fallita EXIT=$LASTEXITCODE"; exit $LASTEXITCODE }
$dt = [int]((Get-Date) - $t0).TotalSeconds
Write-Output "[tcpp] fatto in $dt s. Manifest: $(Test-Path (Join-Path $Prefix 'lib\transcribe-link.json'))"
Get-ChildItem -Recurse $Prefix -Include *.dll,*.exe,*.json | ForEach-Object { "[tcpp] " + $_.FullName.Substring($Prefix.Length) + "  " + $_.Length }
exit 0
