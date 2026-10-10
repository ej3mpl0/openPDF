// SPDX-License-Identifier: MPL-2.0
#if !OPENPDF_MEASURE
using System.Runtime.InteropServices;
using Microsoft.UI.Xaml.Controls;
using WinRT;
using Vortice.Direct3D;
using Vortice.Direct3D11;
using Vortice.DXGI;
using Vortice.Mathematics;

namespace OpenPdf;

/// <summary>
/// Visible-tile compositor: core RGBA → D3D11 texture → SwapChainPanel.
/// LRU eviction is inside openpdf-core (<c>render_tile</c>).
/// </summary>
sealed class TileSwapChain : IDisposable
{
    readonly SwapChainPanel _panel;
    ID3D11Device? _device;
    ID3D11DeviceContext? _context;
    IDXGISwapChain1? _swapChain;
    ID3D11Texture2D? _canvas;
    int _width;
    int _height;
    bool _ready;

    public TileSwapChain(SwapChainPanel panel)
    {
        _panel = panel;
        _panel.SizeChanged += (_, _) => Resize();
        Resize();
    }

    public int VisibleReady { get; private set; }
    public int VisibleTotal { get; private set; }

    public void Resize()
    {
        var scale = _panel.XamlRoot?.RasterizationScale ?? 1.0;
        var w = Math.Max(1, (int)(_panel.ActualWidth * scale));
        var h = Math.Max(1, (int)(_panel.ActualHeight * scale));
        if (w == _width && h == _height && _swapChain != null)
        {
            return;
        }
        _width = w;
        _height = h;
        EnsureDevice();
        RecreateSwapChain();
    }

    public void DrawDocument(PdfDocument? doc, PageInfo[] pages, float zoom, uint scrollY)
    {
        if (doc == null || _context == null || _canvas == null || _swapChain == null)
        {
            VisibleReady = 0;
            VisibleTotal = 0;
            return;
        }
        Clear(0.12f, 0.12f, 0.12f);
        var tile = OpenpdfMethods.TileSize();
        var y = 12u;
        var ready = 0;
        var total = 0;
        var viewH = (uint)_height;
        var viewW = (uint)_width;
        foreach (var page in pages)
        {
            var pw = (uint)Math.Max(1, page.widthPt * zoom);
            var ph = (uint)Math.Max(1, page.heightPt * zoom);
            var pageTop = y;
            y += ph + 12;
            if (pageTop + ph < scrollY || pageTop > scrollY + viewH)
            {
                continue;
            }
            var maxTx = (pw + tile - 1) / tile;
            var maxTy = (ph + tile - 1) / tile;
            for (uint ty = 0; ty < maxTy; ty++)
            {
                for (uint tx = 0; tx < maxTx; tx++)
                {
                    var destX = 16 + tx * tile;
                    var destY = (int)pageTop + (int)(ty * tile) - (int)scrollY;
                    if (destX >= viewW || destY >= (int)viewH || destY + (int)tile < 0)
                    {
                        continue;
                    }
                    total++;
                    try
                    {
                        var raster = doc.RenderTile(page.index, zoom, tx, ty);
                        Blit(raster, (int)destX, destY);
                        ready++;
                    }
                    catch
                    {
                        // leave background
                    }
                }
            }
        }
        VisibleReady = ready;
        VisibleTotal = total;
        Present();
    }

    void EnsureDevice()
    {
        if (_device != null)
        {
            return;
        }
        D3D11.D3D11CreateDevice(
            null,
            DriverType.Hardware,
            DeviceCreationFlags.BgraSupport,
            new[] { FeatureLevel.Level_11_0, FeatureLevel.Level_10_0 },
            out _device,
            out _,
            out _context).CheckError();
        _ready = _device != null;
    }

    void RecreateSwapChain()
    {
        _canvas?.Dispose();
        _canvas = null;
        _swapChain?.Dispose();
        _swapChain = null;
        if (_device == null)
        {
            return;
        }
        using var dxgi = _device.QueryInterface<IDXGIDevice>();
        using var adapter = dxgi.GetAdapter();
        using var factory = adapter.GetParent<IDXGIFactory2>();
        var desc = new SwapChainDescription1
        {
            Width = (uint)_width,
            Height = (uint)_height,
            Format = Format.B8G8R8A8_UNorm,
            Stereo = false,
            SampleDescription = new SampleDescription(1, 0),
            BufferUsage = Usage.RenderTargetOutput | Usage.BackBuffer,
            BufferCount = 2,
            Scaling = Scaling.Stretch,
            SwapEffect = SwapEffect.FlipSequential,
            AlphaMode = AlphaMode.Ignore,
            Flags = SwapChainFlags.None,
        };
        _swapChain = factory.CreateSwapChainForComposition(_device, desc);
        var native = WinRT.CastExtensions.As<ISwapChainPanelNative>(_panel);
        native.SetSwapChain(_swapChain.NativePointer);
        var canvasDesc = new Texture2DDescription
        {
            Width = (uint)_width,
            Height = (uint)_height,
            MipLevels = 1,
            ArraySize = 1,
            Format = Format.R8G8B8A8_UNorm,
            SampleDescription = new SampleDescription(1, 0),
            Usage = ResourceUsage.Default,
            BindFlags = BindFlags.ShaderResource | BindFlags.RenderTarget,
            CPUAccessFlags = CpuAccessFlags.None,
        };
        _canvas = _device.CreateTexture2D(canvasDesc);
    }

    void Clear(float r, float g, float b)
    {
        if (_context == null || _canvas == null)
        {
            return;
        }
        using var rtv = _device!.CreateRenderTargetView(_canvas);
        _context.ClearRenderTargetView(rtv, new Color4(r, g, b, 1));
    }

    void Blit(RasterTile tile, int destX, int destY)
    {
        if (_context == null || _canvas == null || tile.rgba.Length == 0)
        {
            return;
        }
        var w = (int)tile.width;
        var h = (int)tile.height;
        if (w <= 0 || h <= 0)
        {
            return;
        }
        var box = new Box(
            Math.Max(destX, 0),
            Math.Max(destY, 0),
            0,
            Math.Min(destX + w, _width),
            Math.Min(destY + h, _height),
            1);
        if (box.Right <= box.Left || box.Bottom <= box.Top)
        {
            return;
        }
        var desc = new Texture2DDescription
        {
            Width = (uint)w,
            Height = (uint)h,
            MipLevels = 1,
            ArraySize = 1,
            Format = Format.R8G8B8A8_UNorm,
            SampleDescription = new SampleDescription(1, 0),
            Usage = ResourceUsage.Default,
            BindFlags = BindFlags.None,
            CPUAccessFlags = CpuAccessFlags.None,
        };
        using var staging = CreateRgbaTexture(_device!, desc, tile.rgba, w);
        var src = new Box(
            (uint)Math.Max(0, -destX),
            (uint)Math.Max(0, -destY),
            0,
            (uint)(Math.Max(0, -destX) + (box.Right - box.Left)),
            (uint)(Math.Max(0, -destY) + (box.Bottom - box.Top)),
            1);
        _context.CopySubresourceRegion(_canvas, 0, (uint)box.Left, (uint)box.Top, 0, staging, 0, src);
    }

    static unsafe ID3D11Texture2D CreateRgbaTexture(
        ID3D11Device device,
        Texture2DDescription desc,
        byte[] rgba,
        int width)
    {
        fixed (byte* ptr = rgba)
        {
            var data = new SubresourceData((IntPtr)ptr, (uint)(width * 4));
            return device.CreateTexture2D(desc, new[] { data });
        }
    }

    void Present()
    {
        if (_context == null || _swapChain == null || _canvas == null)
        {
            return;
        }
        using var back = _swapChain.GetBuffer<ID3D11Texture2D>(0);
        _context.CopyResource(back, _canvas);
        _swapChain.Present(1, PresentFlags.None);
    }

    public void Dispose()
    {
        _canvas?.Dispose();
        _swapChain?.Dispose();
        _context?.Dispose();
        _device?.Dispose();
    }
}

[ComImport]
[InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
[Guid("F92F19D2-3ADE-45A6-A20C-986F715E8F08")]
interface ISwapChainPanelNative
{
    [PreserveSig]
    int SetSwapChain(IntPtr swapChain);
}
#endif
