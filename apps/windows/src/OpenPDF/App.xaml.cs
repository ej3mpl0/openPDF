// SPDX-License-Identifier: MPL-2.0
using Microsoft.UI.Xaml;

namespace OpenPdf;

public partial class App : Application
{
    public App()
    {
        InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        var window = new MainWindow();
        window.Activate();
    }
}
