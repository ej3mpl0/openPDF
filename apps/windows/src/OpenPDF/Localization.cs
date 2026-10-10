// SPDX-License-Identifier: MPL-2.0
namespace OpenPdf;

static class L10n
{
    public static string T(string msgid)
    {
        try
        {
            return OpenpdfMethods.Translate(msgid);
        }
        catch
        {
            return msgid;
        }
    }

    public static string T(string msgid, string key, string value) =>
        T(msgid).Replace("{" + key + "}", value);

    public static bool IsSilentPinCancel(Exception ex)
    {
        for (var e = ex; e != null; e = e.InnerException)
        {
            var name = e.GetType().FullName ?? e.GetType().Name;
            if (name.Contains("PinCancelled", StringComparison.Ordinal))
            {
                return true;
            }
        }
        return false;
    }
}
