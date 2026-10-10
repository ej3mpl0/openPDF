// SPDX-License-Identifier: MPL-2.0
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Windows.Storage.Pickers;
using WinRT.Interop;

namespace OpenPdf;

public sealed partial class MainWindow : Window
{
    readonly OpenPdf _lib = new();
    readonly WinPinProvider _pin = new();
    readonly TileSwapChain _tiles;
    PdfDocument? _doc;
    PageInfo[] _pages = [];
    float _zoom = 1f;
    int _page;
    bool _signed;

    public MainWindow()
    {
        InitializeComponent();
        Title = L10n.T("Open PDF");
        OpenButton.Content = L10n.T("Open");
        PrevButton.Content = L10n.T("Previous page");
        NextButton.Content = L10n.T("Next page");
        ZoomOutButton.Content = L10n.T("Zoom out");
        ZoomInButton.Content = L10n.T("Zoom in");
        SignButton.Content = L10n.T("Sign");
        ThumbsHeader.Text = L10n.T("Thumbnails");
        SignedText.Text = L10n.T("Signed document — read-only");
        _tiles = new TileSwapChain(TilePanel);
        RefreshChrome();
    }

    async void OpenClicked(object sender, RoutedEventArgs e)
    {
        var picker = new FileOpenPicker();
        picker.FileTypeFilter.Add(".pdf");
        var hwnd = WindowNative.GetWindowHandle(this);
        InitializeWithWindow.Initialize(picker, hwnd);
        var file = await picker.PickSingleFileAsync();
        if (file == null)
        {
            return;
        }
        OpenPath(file.Path);
    }

    internal void OpenFromCli(string path) => OpenPath(path);

    void OpenPath(string path)
    {
        try
        {
            _doc = _lib.Open(path);
            _pages = _doc.Pages();
            _signed = _doc.IsSigned();
            _page = 0;
            _zoom = 1f;
            Title = Path.GetFileName(path);
            ErrorBanner.Visibility = Visibility.Collapsed;
            RebuildThumbs();
            Redraw();
            RefreshChrome();
        }
        catch (Exception ex)
        {
            if (L10n.IsSilentPinCancel(ex))
            {
                return;
            }
            ShowError(ex.Message);
        }
    }

    void RebuildThumbs()
    {
        ThumbList.Items.Clear();
        foreach (var page in _pages)
        {
            ThumbList.Items.Add($"{page.index + 1}");
        }
    }

    void ThumbSelected(object sender, SelectionChangedEventArgs e)
    {
        if (ThumbList.SelectedIndex >= 0)
        {
            _page = ThumbList.SelectedIndex;
            Redraw();
            RefreshChrome();
        }
    }

    void PrevClicked(object sender, RoutedEventArgs e)
    {
        if (_page > 0)
        {
            _page--;
            Redraw();
            RefreshChrome();
        }
    }

    void NextClicked(object sender, RoutedEventArgs e)
    {
        if (_page + 1 < _pages.Length)
        {
            _page++;
            Redraw();
            RefreshChrome();
        }
    }

    void ZoomInClicked(object sender, RoutedEventArgs e)
    {
        _zoom = Math.Min(4f, _zoom * 1.25f);
        Redraw();
        RefreshChrome();
    }

    void ZoomOutClicked(object sender, RoutedEventArgs e)
    {
        _zoom = Math.Max(0.25f, _zoom / 1.25f);
        Redraw();
        RefreshChrome();
    }

    void PageScrolled(object sender, ScrollViewerViewChangedEventArgs e) => Redraw();

    void SignClicked(object sender, RoutedEventArgs e)
    {
        if (_doc == null)
        {
            return;
        }
        if (_signed)
        {
            ShowError(L10n.T(
                "document is signed; {op} would invalidate the signature (incremental save only)",
                "op",
                L10n.T("edit")));
            return;
        }
        try
        {
            var certs = _lib.ListCertificates(_pin);
            var cert = certs.FirstOrDefault(c => c.usages.Contains("signing")) ?? certs.FirstOrDefault();
            if (cert == null)
            {
                ShowError(L10n.T("no signing certificates found"));
                return;
            }
            _doc.Sign(_pin, cert.id, null, null);
            _signed = _doc.IsSigned();
            ErrorBanner.Visibility = Visibility.Collapsed;
            RefreshChrome();
        }
        catch (Exception ex)
        {
            if (L10n.IsSilentPinCancel(ex))
            {
                return;
            }
            ShowError(ex.Message);
        }
    }

    void Redraw()
    {
        var scrollY = (uint)Math.Max(0, PageScroll.VerticalOffset);
        _tiles.DrawDocument(_doc, _pages, _zoom, scrollY);
        SignedBanner.Visibility = _signed ? Visibility.Visible : Visibility.Collapsed;
        SignButton.IsEnabled = _doc != null && !_signed;
    }

    void RefreshChrome()
    {
        PageLabel.Text = L10n.T("Page {page} of {total}")
            .Replace("{page}", (_page + 1).ToString())
            .Replace("{total}", _pages.Length.ToString());
        ZoomLabel.Text = $"{Math.Round(_zoom * 100)}%";
        PrevButton.IsEnabled = _page > 0;
        NextButton.IsEnabled = _page + 1 < _pages.Length;
        SignButton.IsEnabled = _doc != null && !_signed;
        SignedBanner.Visibility = _signed ? Visibility.Visible : Visibility.Collapsed;
    }

    void ShowError(string message)
    {
        ErrorText.Text = message;
        ErrorBanner.Visibility = Visibility.Visible;
    }
}
