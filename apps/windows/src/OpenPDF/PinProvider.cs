#if !OPENPDF_MEASURE
using System.Windows.Forms;

namespace OpenPdf;

/// <summary>
/// UniFFI <c>PinProvider</c>. Empty field or Cancel → PinCancelled (never an empty PIN).
/// Shows <c>pin_prompt</c> plus <c>describe_pin_status</c>.
/// </summary>
sealed class WinPinProvider : PinProvider
{
    public bool CountLow;
    public bool FinalTry;
    public bool Locked;

    public SecretPin RequestPin(string tokenLabel)
    {
        var prompt = OpenpdfMethods.PinPrompt(tokenLabel);
        var status = OpenpdfMethods.DescribePinStatus(CountLow, FinalTry, Locked);
        string? value = null;
        if (System.Windows.Forms.Application.MessageLoop
            || Thread.CurrentThread.GetApartmentState() == ApartmentState.STA)
        {
            value = Prompt(prompt, status);
        }
        else
        {
            string? captured = null;
            var t = new Thread(() => captured = Prompt(prompt, status));
            t.SetApartmentState(ApartmentState.STA);
            t.Start();
            t.Join();
            value = captured;
        }
        if (string.IsNullOrEmpty(value))
        {
            throw PinCancelledError();
        }
        return new SecretPin(value);
    }

    static Exception PinCancelledError()
    {
        var message = L10n.T("PIN entry was cancelled");
        var nested = typeof(FfiException).GetNestedType("PinCancelled");
        if (nested != null)
        {
            var ex = Activator.CreateInstance(nested, message) as Exception;
            if (ex != null)
            {
                return ex;
            }
        }
        return new OperationCanceledException(message);
    }

    static string? Prompt(string prompt, string? status)
    {
        using var form = new Form
        {
            Text = L10n.T("Enter PIN"),
            FormBorderStyle = FormBorderStyle.FixedDialog,
            StartPosition = FormStartPosition.CenterScreen,
            ClientSize = new System.Drawing.Size(380, 180),
            MinimizeBox = false,
            MaximizeBox = false,
        };
        var label = new Label
        {
            Text = string.IsNullOrEmpty(status) ? prompt : prompt + Environment.NewLine + Environment.NewLine + status,
            AutoSize = false,
            Width = 350,
            Height = 70,
            Left = 12,
            Top = 12,
        };
        var box = new TextBox
        {
            UseSystemPasswordChar = true,
            Left = 12,
            Top = 90,
            Width = 350,
        };
        var ok = new Button
        {
            Text = L10n.T("Unlock"),
            DialogResult = DialogResult.OK,
            Left = 196,
            Top = 130,
        };
        var cancel = new Button
        {
            Text = L10n.T("Cancel"),
            DialogResult = DialogResult.Cancel,
            Left = 286,
            Top = 130,
        };
        form.Controls.Add(label);
        form.Controls.Add(box);
        form.Controls.Add(ok);
        form.Controls.Add(cancel);
        form.AcceptButton = ok;
        form.CancelButton = cancel;
        form.ActiveControl = box;
        return form.ShowDialog() == DialogResult.OK ? box.Text : null;
    }
}
#endif
