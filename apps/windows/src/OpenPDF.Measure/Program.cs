namespace OpenPdf;

static class MeasureProgram
{
    static int Main(string[] args)
    {
        var pages = MeasureRunner.DefaultPages;
        var idx = Array.IndexOf(args, "--measure");
        if (idx >= 0 && idx + 1 < args.Length && int.TryParse(args[idx + 1], out var parsed) && parsed > 0)
        {
            pages = parsed;
        }
        else if (idx < 0 && args.Length > 0 && int.TryParse(args[0], out parsed) && parsed > 0)
        {
            pages = parsed;
        }
        return MeasureRunner.Run(pages);
    }
}
