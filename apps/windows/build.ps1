$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $Root

cargo build -p openpdf-ffi --release
$Dll = Join-Path $Root "target\release\openpdf_ffi.dll"
if (-not (Test-Path $Dll)) { throw "openpdf_ffi.dll missing (Windows-only build)" }

$Gen = Join-Path $Root "apps\windows\src\OpenPDF\Generated"
New-Item -ItemType Directory -Force -Path $Gen | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $Root "apps\windows\lib") | Out-Null

if (-not (Get-Command uniffi-bindgen-cs -ErrorAction SilentlyContinue)) {
    cargo install uniffi-bindgen-cs `
        --git https://github.com/NordSecurity/uniffi-bindgen-cs `
        --tag v0.10.0+v0.29.4
}

uniffi-bindgen-cs --library --out-dir $Gen $Dll
Copy-Item $Dll (Join-Path $Root "apps\windows\lib\openpdf_ffi.dll") -Force
if ($env:OPENPDF_PDFIUM -and (Test-Path $env:OPENPDF_PDFIUM)) {
    Copy-Item $env:OPENPDF_PDFIUM (Join-Path $Root "apps\windows\lib\pdfium.dll") -Force
}

dotnet build (Join-Path $Root "apps\windows\src\OpenPDF.Measure\OpenPDF.Measure.csproj") -c Release
dotnet build (Join-Path $Root "apps\windows\src\OpenPDF\OpenPDF.csproj") -c Release
Write-Host "built OpenPDF + OpenPDF.Measure"
