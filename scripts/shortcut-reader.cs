using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Text;

// Read stored Unicode fields without resolving, modifying or launching the shortcut.
public static class ShortcutReader
{
    public sealed class ShortcutInfo
    {
        public string TargetPath { get; internal set; }
        public string IconLocation { get; internal set; }
        public int IconIndex { get; internal set; }
    }

    public static ShortcutInfo Read(string path)
    {
        const uint STGM_READ = 0;
        const uint SLGP_RAWPATH = 4;
        object instance = Activator.CreateInstance(Type.GetTypeFromCLSID(
            new Guid("00021401-0000-0000-C000-000000000046"), true));
        try
        {
            ((IPersistFile)instance).Load(Path.GetFullPath(path), (int)STGM_READ);
            var link = (IShellLinkW)instance;
            var target = new StringBuilder(32768);
            var icon = new StringBuilder(32768);
            link.GetPath(target, target.Capacity, IntPtr.Zero, SLGP_RAWPATH);
            int iconIndex;
            link.GetIconLocation(icon, icon.Capacity, out iconIndex);
            return new ShortcutInfo {
                TargetPath = target.ToString(), IconLocation = icon.ToString(), IconIndex = iconIndex
            };
        }
        finally
        {
            // Both interface casts share this RCW; release its COM references once.
            if (instance != null) Marshal.FinalReleaseComObject(instance);
        }
    }

    [ComImport, Guid("000214F9-0000-0000-C000-000000000046")]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IShellLinkW
    {
        void GetPath([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int count, IntPtr findData, uint flags);
        void GetIDList(out IntPtr idList);
        void SetIDList(IntPtr idList);
        void GetDescription([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder description, int count);
        void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string description);
        void GetWorkingDirectory([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder directory, int count);
        void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string directory);
        void GetArguments([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder arguments, int count);
        void SetArguments([MarshalAs(UnmanagedType.LPWStr)] string arguments);
        void GetHotkey(out short hotkey);
        void SetHotkey(short hotkey);
        void GetShowCmd(out int showCommand);
        void SetShowCmd(int showCommand);
        void GetIconLocation([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int count, out int index);
    }
}
