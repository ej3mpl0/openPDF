using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace OpenPdf;

public static class Program
{
    [STAThread]
    static void Main(string[] args)
    {
        if (args.Contains("--measure"))
        {
            var pages = MeasureRunner.DefaultPages;
            var idx = Array.IndexOf(args, "--measure");
            if (idx + 1 < args.Length && int.TryParse(args[idx + 1], out var parsed) && parsed > 0)
            {
                pages = parsed;
            }
            Environment.Exit(MeasureRunner.Run(pages));
            return;
        }

        WinRT.ComWrappersSupport.InitializeComWrappers();
        Application.Start(p =>
        {
            var context = new DispatcherQueueSynchronizationContext(
                DispatcherQueue.GetForCurrentThread());
            SynchronizationContext.SetSynchronizationContext(context);
            new App();
        });
    }
}
