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
        foreach (var arg in Environment.GetCommandLineArgs().Skip(1))
        {
            if (arg.EndsWith(".pdf", StringComparison.OrdinalIgnoreCase) && File.Exists(arg))
            {
                window.OpenFromCli(arg);
                break;
            }
        }
    }
}
