// Runner-only client of the real Windows Installer external-UI API.
using System;
using System.IO;
using System.Runtime.InteropServices;

internal static class MsiControlNativeTests
{
    [UnmanagedFunctionPointer(CallingConvention.Winapi, CharSet = CharSet.Unicode)]
    private delegate int InstallUiHandler(IntPtr context, uint type, string message);
    [DllImport("msi.dll", CharSet = CharSet.Unicode)]
    private static extern uint MsiInstallProductW(string package, string properties);
    [DllImport("msi.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr MsiSetExternalUIW(InstallUiHandler handler, uint filter, IntPtr context);
    [DllImport("msi.dll")]
    private static extern uint MsiSetInternalUI(uint level, IntPtr window);
    [DllImport("msi.dll", CharSet = CharSet.Unicode)]
    private static extern uint MsiEnableLogW(uint mode, string path, uint attributes);
    private static bool requested;
    private static string lastAction = String.Empty;

    private static int CancelAtFiles(IntPtr context, uint type, string message)
    {
        if ((type & 0xFF000000) == 0x08000000)
        {
            lastAction = message ?? String.Empty;
            if (lastAction.IndexOf("InstallFiles", StringComparison.Ordinal) >= 0)
            {
                requested = true;
                return 2; // IDCANCEL: Windows Installer must execute its real rollback.
            }
        }
        return 0;
    }

    private static int Main(string[] args)
    {
        if (Environment.GetEnvironmentVariable("GITHUB_ACTIONS") != "true" || Environment.GetEnvironmentVariable("RUNNER_OS") != "Windows" || args.Length != 2) return 87;
        string log = Path.GetFullPath(args[1]);
        string runner = Path.GetFullPath(Environment.GetEnvironmentVariable("RUNNER_TEMP")).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        if (!log.StartsWith(runner, StringComparison.OrdinalIgnoreCase) || File.Exists(log) || File.Exists(log + ".json")) return 87;
        try
        {
            if (MsiEnableLogW(0x3FFF, log, 2) != 0) return 1;
            MsiSetInternalUI(2, IntPtr.Zero); // INSTALLUILEVEL_NONE
            InstallUiHandler callback = CancelAtFiles;
            MsiSetExternalUIW(callback, 0x100, IntPtr.Zero); // ACTIONSTART
            uint code;
            try { code = MsiInstallProductW(Path.GetFullPath(args[0]), "REBOOT=ReallySuppress MSIRESTARTMANAGERCONTROL=Disable"); }
            finally { MsiSetExternalUIW(null, 0, IntPtr.Zero); }
            GC.KeepAlive(callback);
            string action = lastAction.Replace("\\", "\\\\").Replace("\"", "\\\"").Replace("\r", "\\r").Replace("\n", "\\n");
            File.WriteAllText(log + ".json", "{\"msi_exit_code\":" + code + ",\"callback_requested\":" + (requested ? "true" : "false") + ",\"last_action\":\"" + action + "\"}");
            if (!requested || code != 1602) return 1;
            return (int)code;
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine(exception.ToString());
            return 1;
        }
    }
}
