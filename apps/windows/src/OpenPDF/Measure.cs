// SPDX-License-Identifier: MPL-2.0
using System.Runtime.InteropServices;
using System.Text;

namespace OpenPdf;

static class MeasureRunner
{
    public const int DefaultPages = 32;
    public const ulong MaxStartupMs = 8_000;
    public const ulong MaxRssBytes = 250UL * 1024 * 1024;
    public const int MinVisibleTiles = 1;
    public const int MinThumbTiles = 3;

    public sealed class Report
    {
        public int Pages;
        public ulong StartupMs;
        public ulong RssBytes;
        public int VisibleReady;
        public int VisibleTotal;
        public int ThumbReady;
        public int ThumbTotal;
        public bool Ok;
        public string? Error;

        public void Evaluate()
        {
            string? err = Error;
            if (VisibleReady < MinVisibleTiles)
            {
                err ??= $"visible tiles {VisibleReady}/{VisibleTotal} (need ≥ {MinVisibleTiles})";
            }
            if (ThumbReady < MinThumbTiles)
            {
                err ??= $"thumbnail tiles {ThumbReady}/{ThumbTotal} (need ≥ {MinThumbTiles})";
            }
            if (StartupMs > MaxStartupMs)
            {
                err ??= $"startup {StartupMs} ms exceeds {MaxStartupMs} ms";
            }
            if (RssBytes > MaxRssBytes)
            {
                err ??= $"peak RSS {RssBytes} exceeds {MaxRssBytes}";
            }
            Error = err;
            Ok = err == null;
        }

        public override string ToString()
        {
            var sb = new StringBuilder();
            sb.AppendLine("mode=ui");
            sb.AppendLine($"pages={Pages}");
            sb.AppendLine($"startup_ms={StartupMs}");
            sb.AppendLine($"peak_rss_bytes={RssBytes}");
            sb.AppendLine($"peak_rss_mb={RssBytes / (1024.0 * 1024.0):0.0}");
            sb.AppendLine($"visible_tiles={VisibleReady}/{VisibleTotal}");
            sb.AppendLine($"thumb_tiles={ThumbReady}/{ThumbTotal}");
            sb.AppendLine($"limit_startup_ms={MaxStartupMs}");
            sb.AppendLine($"limit_peak_rss_bytes={MaxRssBytes}");
            sb.AppendLine($"ok={Ok.ToString().ToLowerInvariant()}");
            if (Error != null)
            {
                sb.AppendLine($"error={Error}");
            }
            return sb.ToString();
        }
    }

    public static int Run(int pages)
    {
        var t0 = DateTime.UtcNow;
        var report = new Report { Pages = pages };
        string? pdf = null;
        try
        {
            pdf = WriteMeasurePdf(Math.Max(pages, 2));
            var lib = new OpenPdf();
            var doc = lib.Open(pdf);
            var pageInfos = doc.Pages();
            var tile = OpenpdfMethods.TileSize();
            var visKeys = 0;
            var visReady = 0;
            if (pageInfos.Length > 0)
            {
                var first = pageInfos[0];
                var zoom = 1f;
                var pw = first.widthPt * zoom;
                var ph = first.heightPt * zoom;
                var maxTx = Math.Max(0, (int)Math.Ceiling(pw / tile) - 1);
                var maxTy = Math.Max(0, (int)Math.Ceiling(Math.Min(ph, 600) / tile) - 1);
                for (var ty = 0; ty <= maxTy; ty++)
                {
                    for (var tx = 0; tx <= Math.Min(maxTx, (int)Math.Ceiling(800 / (double)tile) - 1); tx++)
                    {
                        visKeys++;
                        try
                        {
                            var raster = doc.RenderTile(first.index, zoom, (uint)tx, (uint)ty);
                            if (raster.rgba.Length > 0)
                            {
                                visReady++;
                            }
                        }
                        catch
                        {
                            // counted as missing
                        }
                    }
                }
            }
            if (visKeys == 0)
            {
                visKeys = 1;
            }
            report.VisibleReady = visReady;
            report.VisibleTotal = visKeys;

            var thumbN = Math.Min(4, pageInfos.Length);
            var thumbReady = 0;
            for (var i = 0; i < thumbN; i++)
            {
                try
                {
                    var raster = doc.RenderTile(pageInfos[i].index, 0.18f, 0, 0);
                    if (raster.rgba.Length > 0)
                    {
                        thumbReady++;
                    }
                }
                catch
                {
                    // missing thumb
                }
            }
            report.ThumbReady = thumbReady;
            report.ThumbTotal = thumbN;
            report.StartupMs = (ulong)Math.Max(0, (DateTime.UtcNow - t0).TotalMilliseconds);
            report.RssBytes = PeakRssBytes();
            report.Evaluate();
        }
        catch (Exception ex)
        {
            report.StartupMs = (ulong)Math.Max(0, (DateTime.UtcNow - t0).TotalMilliseconds);
            report.RssBytes = PeakRssBytes();
            report.Error = ex.Message;
            report.Evaluate();
        }
        finally
        {
            if (pdf != null)
            {
                try { File.Delete(pdf); } catch { /* temp */ }
            }
        }
        Console.Write(report.ToString());
        return report.Ok ? 0 : 1;
    }

    public static string WriteMeasurePdf(int pages)
    {
        var hello = CorpusPdf("hello.pdf");
        var lib = new OpenPdf();
        var src = lib.Open(hello);
        var doc = src;
        var range = new[] { new PageRange(0, 0) };
        while (doc.PageCount() < (uint)pages)
        {
            doc = doc.InsertPages(doc.PageCount(), src, range);
        }
        var dest = Path.Combine(Path.GetTempPath(), $"openpdf-measure-{Environment.ProcessId}-{pages}.pdf");
        doc.Save(dest);
        return dest;
    }

    public static string CorpusPdf(string name)
    {
        var candidates = new List<string>();
        var env = Environment.GetEnvironmentVariable("OPENPDF_CORPUS");
        if (!string.IsNullOrEmpty(env))
        {
            candidates.Add(Path.Combine(env, name));
        }
        candidates.Add(Path.Combine(Environment.CurrentDirectory, "corpus", name));
        candidates.Add(Path.Combine(Environment.CurrentDirectory, "..", "..", "corpus", name));
        var exe = AppContext.BaseDirectory;
        candidates.Add(Path.Combine(exe, "corpus", name));
        candidates.Add(Path.GetFullPath(Path.Combine(exe, "..", "..", "..", "..", "..", "corpus", name)));
        foreach (var c in candidates)
        {
            if (File.Exists(c))
            {
                return c;
            }
        }
        throw new FileNotFoundException($"corpus/{name} is missing; cannot build the measure document");
    }

    public static ulong PeakRssBytes()
    {
        var counters = new PROCESS_MEMORY_COUNTERS { cb = (uint)Marshal.SizeOf<PROCESS_MEMORY_COUNTERS>() };
        GetProcessMemoryInfo(GetCurrentProcess(), out counters, counters.cb);
        return counters.PeakWorkingSetSize;
    }

    [DllImport("kernel32.dll")]
    static extern IntPtr GetCurrentProcess();

    [DllImport("psapi.dll", SetLastError = true)]
    static extern bool GetProcessMemoryInfo(IntPtr hProcess, out PROCESS_MEMORY_COUNTERS counters, uint size);

    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_MEMORY_COUNTERS
    {
        public uint cb;
        public uint PageFaultCount;
        public ulong PeakWorkingSetSize;
        public ulong WorkingSetSize;
        public ulong QuotaPeakPagedPoolUsage;
        public ulong QuotaPagedPoolUsage;
        public ulong QuotaPeakNonPagedPoolUsage;
        public ulong QuotaNonPagedPoolUsage;
        public ulong PagefileUsage;
        public ulong PeakPagefileUsage;
    }
}
