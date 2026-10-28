// CI-only: drive NSIS controls by Win32 messages, without mouse/keyboard/clipboard.
using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

internal static class NsisDirectoryNative
{
    private delegate bool EnumProc(IntPtr window, IntPtr parameter);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumProc callback, IntPtr parameter);
    [DllImport("user32.dll")] private static extern bool EnumChildWindows(IntPtr parent, EnumProc callback, IntPtr parameter);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll")] private static extern int GetDlgCtrlID(IntPtr window);
    [DllImport("user32.dll")] private static extern IntPtr GetParent(IntPtr window);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] private static extern bool IsWindowEnabled(IntPtr window);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongW")] private static extern int GetWindowLong(IntPtr window, int index);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetClassName(IntPtr window, StringBuilder name, int length);
    [DllImport("user32.dll")] private static extern bool PostMessage(IntPtr window, uint message, IntPtr wparam, IntPtr lparam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern IntPtr SendMessageTimeout(IntPtr window, uint message, IntPtr wparam, string text, uint flags, uint timeout, out UIntPtr result);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern IntPtr SendMessageTimeout(IntPtr window, uint message, IntPtr wparam, StringBuilder text, uint flags, uint timeout, out UIntPtr result);
    [DllImport("user32.dll")] private static extern IntPtr SendMessageTimeout(IntPtr window, uint message, IntPtr wparam, IntPtr lparam, uint flags, uint timeout, out UIntPtr result);

    private static void Require(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
    }
    private static IntPtr WaitFor(Func<IntPtr> probe, string stage)
    {
        var timer = Stopwatch.StartNew();
        while (timer.Elapsed.TotalSeconds < 90)
        {
            IntPtr result = probe();
            if (result != IntPtr.Zero) return result;
            Thread.Sleep(100);
        }
        throw new TimeoutException("NSIS stage timed out: " + stage);
    }
    private static IntPtr FindControl(IntPtr parent, int id, string className)
    {
        IntPtr found = IntPtr.Zero;
        EnumChildWindows(parent, delegate(IntPtr window, IntPtr unused) {
            var name = new StringBuilder(64);
            GetClassName(window, name, name.Capacity);
            if (GetDlgCtrlID(window) == id && name.ToString() == className && IsWindowVisible(window)) found = window;
            return true;
        }, IntPtr.Zero);
        return found;
    }
    private static void Click(IntPtr button)
    {
        Require(button != IntPtr.Zero && IsWindowEnabled(button), "Expected enabled NSIS button");
        // BN_CLICKED directly avoids BM_CLICK's active-dialog requirement.
        Require(PostMessage(GetParent(button), 0x0111, new IntPtr(GetDlgCtrlID(button)), button), "WM_COMMAND / BN_CLICKED failed");
    }
    private static IntPtr FindRunCheckbox(IntPtr parent)
    {
        // MUI2 uses nsDialogs, whose generated control IDs are not a stable API.
        IntPtr found = IntPtr.Zero;
        EnumChildWindows(parent, delegate(IntPtr window, IntPtr unused) {
            var name = new StringBuilder(64);
            GetClassName(window, name, name.Capacity);
            if (name.ToString() == "Button" && IsWindowVisible(window) && (GetWindowLong(window, -16) & 15) == 3)
                found = window; // BS_AUTOCHECKBOX; this installer offers only Run.
            return true;
        }, IntPtr.Zero);
        return found;
    }

    private static IntPtr FindDialog(int processId, IntPtr except)
    {
        IntPtr found = IntPtr.Zero;
        EnumWindows(delegate(IntPtr window, IntPtr unused) {
            uint pid;
            GetWindowThreadProcessId(window, out pid);
            if (pid == processId && window != except && IsWindowVisible(window))
            {
                var name = new StringBuilder(64);
                GetClassName(window, name, name.Capacity);
                if (name.ToString() == "#32770") found = window;
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    private static string DialogText(IntPtr dialog)
    {
        var text = new StringBuilder();
        EnumChildWindows(dialog, delegate(IntPtr window, IntPtr unused) {
            var value = new StringBuilder(2048);
            UIntPtr result;
            Require(SendMessageTimeout(window, 0x000D, new IntPtr(value.Capacity), value, 2, 5000, out result) != IntPtr.Zero, "Dialog WM_GETTEXT failed");
            var className = new StringBuilder(64);
            GetClassName(window, className, className.Capacity);
            Console.WriteLine("Dialog control: id=" + GetDlgCtrlID(window) + " class=" + className + " visible=" + IsWindowVisible(window) + " enabled=" + IsWindowEnabled(window) + " text=" + value);
            text.Append(value);
            return true;
        }, IntPtr.Zero);
        return text.ToString();
    }

    private static IntPtr FindOnlyAcknowledgementButton(IntPtr dialog)
    {
        IntPtr found = IntPtr.Zero;
        int count = 0;
        EnumChildWindows(dialog, delegate(IntPtr window, IntPtr unused) {
            var name = new StringBuilder(64);
            GetClassName(window, name, name.Capacity);
            int style = GetWindowLong(window, -16) & 15;
            if (name.ToString().Equals("Button", StringComparison.OrdinalIgnoreCase) &&
                (style == 0 || style == 1) && IsWindowVisible(window) && IsWindowEnabled(window))
            {
                found = window;
                count++;
            }
            return true;
        }, IntPtr.Zero);
        Require(count <= 1, "Expected a single acknowledgement button on relocation warning");
        return found;
    }

    private static int Main(string[] args)
    {
        // Must precede process creation or any user32 call. Never run on a user's PC.
        if (Environment.GetEnvironmentVariable("GITHUB_ACTIONS") != "true" || Environment.GetEnvironmentVariable("RUNNER_OS") != "Windows") return 87;
        if (args.Length != 2 && args.Length != 3) return 87;
        bool expectRejection = args.Length == 3 && args[2] == "reject";
        if (args.Length == 3 && !expectRejection) return 87;
        string runner = Path.GetFullPath(Environment.GetEnvironmentVariable("RUNNER_TEMP")).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        string destination = Path.GetFullPath(args[1]);
        if (!destination.StartsWith(runner, StringComparison.OrdinalIgnoreCase)) return 87;
        Process process = null;
        try
        {
            Require(Process.GetProcessesByName("WW-StudyApp").Length == 0, "Application already running before install");
            // This visible fixture exists only on the disposable GitHub runner.
            process = Process.Start(new ProcessStartInfo(Path.GetFullPath(args[0])) { UseShellExecute = false, WindowStyle = ProcessWindowStyle.Normal });
            IntPtr dialog = WaitFor(delegate { return FindDialog(process.Id, IntPtr.Zero); }, "welcome dialog");
            Click(WaitFor(delegate { return FindControl(dialog, 1, "Button"); }, "welcome next"));
            IntPtr edit = WaitFor(delegate { return FindControl(dialog, 1019, "Edit"); }, "directory edit");
            UIntPtr result;
            Require(SendMessageTimeout(edit, 0x000C, IntPtr.Zero, destination, 2, 5000, out result) != IntPtr.Zero, "WM_SETTEXT failed");
            var selected = new StringBuilder(1024);
            Require(SendMessageTimeout(edit, 0x000D, new IntPtr(selected.Capacity), selected, 2, 5000, out result) != IntPtr.Zero, "WM_GETTEXT failed");
            Require(selected.ToString() == destination, "Directory page did not retain chosen Unicode path");
            Click(WaitFor(delegate {
                IntPtr next = FindControl(dialog, 1, "Button");
                return next != IntPtr.Zero && IsWindowEnabled(next) ? next : IntPtr.Zero;
            }, "install button"));
            if (expectRejection)
            {
                IntPtr warning = WaitFor(delegate { return FindDialog(process.Id, dialog); }, "relocation rejection");
                string warningText = DialogText(warning);
                Require(warningText.Contains("先卸载旧版本") && warningText.Contains("保留") && warningText.Contains("数据"), "Rejection must explain uninstall first and data retention");
                Console.WriteLine("Relocation warning: " + warningText);
                Click(WaitFor(delegate {
                    return FindOnlyAcknowledgementButton(warning);
                }, "relocation warning enabled OK button"));
                Require(process.WaitForExit(30000) && process.ExitCode == 6, "Relocation must exit with code 6");
                Require(!Directory.Exists(destination), "Rejected relocation created destination");
                Require(Process.GetProcessesByName("WW-StudyApp").Length == 0, "Rejected relocation launched application");
                Console.WriteLine("PASS: interactive relocation rejected before installation");
                return 0;
            }
            IntPtr run = WaitFor(delegate { return FindRunCheckbox(dialog); }, "finish run checkbox");
            Require(SendMessageTimeout(run, 0x00F0, IntPtr.Zero, IntPtr.Zero, 2, 5000, out result) != IntPtr.Zero, "BM_GETCHECK failed");
            Require(result.ToUInt64() == 0, "Finish page must leave application launch unchecked by default");
            Click(FindControl(dialog, 1, "Button"));
            Require(process.WaitForExit(30000) && process.ExitCode == 0, "Interactive installer did not finish successfully");
            Require(Process.GetProcessesByName("WW-StudyApp").Length == 0, "Finish unexpectedly launched the application");
            Require(File.Exists(Path.Combine(destination, "WW-StudyApp.exe")), "Directory selection did not control installed location");
            Console.WriteLine("PASS: interactive directory choice (spaces + Unicode), finish without application launch");
            return 0;
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine(exception);
            return 1;
        }
        finally
        {
            // Only the exact process launched above; no cleanup of user files.
            if (process != null)
            {
                if (!process.HasExited) process.Kill();
                process.Dispose();
            }
        }
    }
}
