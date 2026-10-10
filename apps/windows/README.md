# openPDF for Windows

Native WinUI 3 + C# viewer. Bindings from `uniffi-bindgen-cs` (v0.10.0+v0.29.4).
No webviews.

Tiles are core RGBA (`PdfDocument.RenderTile`) presented on a `SwapChainPanel`
(D3D11 composition swap chain). The LRU cache stays in `openpdf-core`. Opening
a file uses `FileOpenPicker`. A signed document stays read-only. The PIN
dialog implements the UniFFI `PinProvider` callback: it shows `pin_prompt`
plus `describe_pin_status`, and an empty field or Cancel is
`FfiError.PinCancelled` (silent, never an empty PIN).

en/es strings go through `translate` (shared `po/`).

MSIX identity: `io.github.ej3mpl0.openpdf` (`Package.appxmanifest`). CI builds
unpackaged (`WindowsPackageType=None`) and publishes the measure console.

## Build

Windows only. From the repo root (PowerShell):

```powershell
$env:OPENPDF_PDFIUM = "$HOME\pdfium\bin\pdfium.dll"  # pdfium-win-x64, chromium/8035
.\apps\windows\build.ps1
```

`build.ps1` compiles `openpdf-ffi`, generates C# bindings, then
`dotnet build` of the WinUI app and `OpenPDF.Measure`.

Pack MSIX (optional, local):

```powershell
dotnet publish apps/windows/src/OpenPDF/OpenPDF.csproj -c Release -p:WindowsPackageType=MSIX
```

## Measure

Same `key=value` report as `openpdf-gtk --measure`:

```powershell
.\apps\windows\src\OpenPDF.Measure\bin\Release\net8.0\OpenPDF.Measure.exe --measure 32
```

Gates: startup ≤ 8000 ms, peak RSS ≤ 250 MiB, at least 1 visible tile and 3
thumbnail tiles. Exit 0 if `ok=true`.
