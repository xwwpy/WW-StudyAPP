using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Cryptography;
using System.Security.Principal;
using Microsoft.Win32.SafeHandles;

internal static class Setup
{
    [StructLayout(LayoutKind.Sequential)]
    private struct SecurityAttributes { public int Length; public IntPtr Descriptor; public int Inherit; }
    [StructLayout(LayoutKind.Sequential)]
    private struct NativeFileIdentity { public ulong VolumeSerial; public ulong FileIdLow; public ulong FileIdHigh; }
    [StructLayout(LayoutKind.Sequential)]
    private struct NativeFileAttributes { public uint Attributes; public uint ReparseTag; }
    [StructLayout(LayoutKind.Sequential)]
    private struct NativeFileDisposition { [MarshalAs(UnmanagedType.U1)] public bool DeleteFile; }
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CreateDirectory(string path, ref SecurityAttributes attributes);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFile(string path, uint access, uint share, IntPtr security, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandleEx(SafeFileHandle handle, int informationClass, out NativeFileIdentity information, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandleEx(SafeFileHandle handle, int informationClass, out NativeFileAttributes information, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetFileInformationByHandle(SafeFileHandle handle, int informationClass, ref NativeFileDisposition information, uint size);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int MessageBox(IntPtr window, string text, string caption, uint type);

    // Keep opened ancestors alive until msiexec exits. Reparse points are rejected
    // and FILE_SHARE_DELETE is absent, so an ancestor cannot be renamed underneath us.
    private static void GuardAncestors(string path, List<SafeFileHandle> guards)
    {
        string full = Path.GetFullPath(path);
        if (full.StartsWith(@"\\", StringComparison.Ordinal) || full.IndexOf(':', 2) >= 0)
            throw new IOException("A local temporary directory is required.");
        var pending = new Stack<string>();
        for (var directory = new DirectoryInfo(full); directory != null; directory = directory.Parent) pending.Push(directory.FullName);
        while (pending.Count != 0)
        {
            string directory = pending.Pop();
            SafeFileHandle handle = CreateFile(directory, 0x80, 3, IntPtr.Zero, 3, 0x02200000, IntPtr.Zero);
            if (handle.IsInvalid) { handle.Dispose(); throw new IOException("Cannot protect the temporary directory."); }
            guards.Add(handle);
            if ((File.GetAttributes(directory) & FileAttributes.ReparsePoint) != 0) throw new IOException("Linked temporary directories are unsupported.");
        }
    }

    private static string CreatePrivateDirectory(string parent)
    {
        var security = new DirectorySecurity();
        security.SetAccessRuleProtection(true, false);
        var current = WindowsIdentity.GetCurrent().User;
        security.SetOwner(current);
        security.AddAccessRule(new FileSystemAccessRule(current, FileSystemRights.FullControl,
            InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit, PropagationFlags.None, AccessControlType.Allow));
        security.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(WellKnownSidType.LocalSystemSid, null), FileSystemRights.FullControl,
            InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit, PropagationFlags.None, AccessControlType.Allow));
        byte[] descriptor = security.GetSecurityDescriptorBinaryForm();
        IntPtr memory = Marshal.AllocHGlobal(descriptor.Length);
        try
        {
            Marshal.Copy(descriptor, 0, memory, descriptor.Length);
            var attributes = new SecurityAttributes { Length = Marshal.SizeOf(typeof(SecurityAttributes)), Descriptor = memory, Inherit = 0 };
            string path = Path.Combine(parent, "WW-StudyApp-Setup-" + Guid.NewGuid().ToString("N"));
            if (!CreateDirectory(path, ref attributes)) throw new IOException("Cannot create a private installation directory.");
            return path;
        }
        finally { Marshal.FreeHGlobal(memory); }
    }

    private static string EmbeddedDigest()
    {
        using (Stream resource = Assembly.GetExecutingAssembly().GetManifestResourceStream("StudyApp.Payload.sha256"))
        {
            if (resource == null || resource.Length > 128) throw new IOException("The installer checksum is missing.");
            using (var reader = new StreamReader(resource, System.Text.Encoding.ASCII))
            {
                string digest = reader.ReadToEnd().Trim();
                if (digest.Length != 64) throw new IOException("The installer checksum is invalid.");
                foreach (char character in digest) if (!((character >= '0' && character <= '9') || (character >= 'a' && character <= 'f'))) throw new IOException("The installer checksum is invalid.");
                return digest;
            }
        }
    }

    private static NativeFileIdentity GetNativeIdentity(SafeFileHandle handle)
    {
        NativeFileAttributes attributes;
        NativeFileIdentity identity;
        if (!GetFileInformationByHandleEx(handle, 9, out attributes, (uint)Marshal.SizeOf(typeof(NativeFileAttributes))) ||
            (attributes.Attributes & (uint)(FileAttributes.ReparsePoint | FileAttributes.Directory)) != 0 ||
            !GetFileInformationByHandleEx(handle, 18, out identity, (uint)Marshal.SizeOf(typeof(NativeFileIdentity))))
            throw new IOException("The staged installer is not a supported regular file.");
        return identity;
    }

    private static bool SameIdentity(NativeFileIdentity left, NativeFileIdentity right)
    {
        return left.VolumeSerial == right.VolumeSerial && left.FileIdLow == right.FileIdLow && left.FileIdHigh == right.FileIdHigh;
    }

    private static void VerifyDigest(Stream stream, string expected)
    {
        stream.Position = 0;
        using (var sha = SHA256.Create())
        {
            string actual = BitConverter.ToString(sha.ComputeHash(stream)).Replace("-", "").ToLowerInvariant();
            if (!String.Equals(actual, expected, StringComparison.Ordinal)) throw new IOException("The embedded installer checksum does not match.");
        }
        stream.Position = 0;
    }

    private static FileStream OpenVerifiedPackage(string path, NativeFileIdentity expectedIdentity, string expectedDigest)
    {
        // Open the file itself without following a leaf reparse point. The writer
        // is already closed; read-only sharing now permits MSI's read-only opens.
        SafeFileHandle handle = CreateFile(path, 0x80000000, 1, IntPtr.Zero, 3, 0x00200000, IntPtr.Zero);
        if (handle.IsInvalid) { handle.Dispose(); throw new IOException("Cannot protect the staged installer."); }
        FileStream stream = null;
        try
        {
            if (!SameIdentity(expectedIdentity, GetNativeIdentity(handle))) throw new IOException("The staged installer identity changed.");
            stream = new FileStream(handle, FileAccess.Read);
            VerifyDigest(stream, expectedDigest);
            return stream;
        }
        catch
        {
            if (stream != null) stream.Dispose();
            else handle.Dispose();
            throw;
        }
    }

    private static string StagedPackageName()
    {
        // The release asset is WW-StudyApp-<version>-windows-x86_64-Setup.exe next
        // to the equally named MSI; a renamed wrapper keeps a neutral fallback.
        string name = Path.GetFileNameWithoutExtension(Environment.GetCommandLineArgs()[0]);
        if (name.StartsWith("WW-StudyApp-", StringComparison.OrdinalIgnoreCase) &&
            name.EndsWith("-Setup", StringComparison.OrdinalIgnoreCase))
        {
            return name.Substring(0, name.Length - "-Setup".Length) + ".msi";
        }
        return "StudyApp.msi";
    }

    private static void Trace(string message)
    {        // Diagnostics only: normal users never set WW_STUDYAPP_SETUP_LOG.
        string path = Environment.GetEnvironmentVariable("WW_STUDYAPP_SETUP_LOG");
        if (String.IsNullOrEmpty(path) || path.IndexOf('"') >= 0) return;
        try { File.AppendAllText(path + ".trace.txt", DateTime.UtcNow.ToString("O") + " " + message + Environment.NewLine); } catch { }
    }

    private static void DeleteOwnedPackage(string path, NativeFileIdentity expectedIdentity)
    {
        // The MSI reader is closed. Bind cleanup to one exclusive DELETE-capable
        // no-follow handle; a name substitution cannot authorize another file.
        using (SafeFileHandle handle = CreateFile(path, 0x00010080, 0, IntPtr.Zero, 3, 0x00200000, IntPtr.Zero))
        {
            if (handle.IsInvalid) throw new IOException("The staged installer could not be reopened for cleanup.");
            if (!SameIdentity(expectedIdentity, GetNativeIdentity(handle))) throw new IOException("The cleanup candidate identity changed.");
            var disposition = new NativeFileDisposition { DeleteFile = true };
            if (!SetFileInformationByHandle(handle, 4, ref disposition, (uint)Marshal.SizeOf(typeof(NativeFileDisposition))))
                throw new IOException("The owned installer could not be removed.");
        }
    }

    [STAThread]
    private static int Main(string[] args)
    {
        bool quiet = false;
        Trace("start " + String.Join(" ", args));
        foreach (string argument in args)
        {
            if (String.Equals(argument, "/quiet", StringComparison.OrdinalIgnoreCase)) quiet = true;
            else return 87;
        }
        string directory = null;
        string package = null;
        NativeFileIdentity? packageIdentity = null;
        var guards = new List<SafeFileHandle>();
        try
        {
            string parent = Path.GetFullPath(Path.GetTempPath());
            GuardAncestors(parent, guards);
            directory = CreatePrivateDirectory(parent);
            GuardAncestors(directory, guards);
            // Windows Installer resolves the original package by name when a
            // same-version Setup runs as a reconfiguration of the installed
            // product. Stage the embedded MSI under that same release name so
            // the new SOURCEDIR can satisfy the lookup.
            string candidate = Path.Combine(directory, StagedPackageName());
            string expectedDigest = EmbeddedDigest();
            // Finish extraction and record its native identity before closing the
            // writer. Ancestor leases remain held across the reader transition.
            using (var staged = new FileStream(candidate, FileMode.CreateNew, FileAccess.ReadWrite, FileShare.Read))
            {
                package = candidate;
                packageIdentity = GetNativeIdentity(staged.SafeFileHandle);
                using (Stream resource = Assembly.GetExecutingAssembly().GetManifestResourceStream("StudyApp.Payload.msi"))
                {
                    if (resource == null || resource.Length <= 0 || resource.Length > 268435456) throw new IOException("The embedded installer is missing or too large.");
                    resource.CopyTo(staged);
                }
                staged.Flush(true);
                VerifyDigest(staged, expectedDigest);
            }
            Trace("extracted");
            using (FileStream verified = OpenVerifiedPackage(candidate, packageIdentity.Value, expectedDigest))
            {
                package = candidate;
                string executable = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "msiexec.exe");
                // The acceptance harness opts into a verbose MSI log; normal users
                // never set this variable and keep the quiet invocation.
                string verboseLog = Environment.GetEnvironmentVariable("WW_STUDYAPP_SETUP_LOG");
                string logging = String.IsNullOrEmpty(verboseLog) || verboseLog.IndexOf('"') >= 0
                    ? "" : " /l*v \"" + verboseLog + "\"";
                var start = new ProcessStartInfo(executable, "/i \"" + package + "\" /norestart MSIRESTARTMANAGERCONTROL=Disable" + (quiet ? " /qn" : "") + logging);
                start.UseShellExecute = false;
                start.CreateNoWindow = true;
                Trace("msiexec starting");
                using (Process process = Process.Start(start))
                {
                    if (process == null) throw new IOException("Windows Installer did not start.");
                    process.WaitForExit();
                    Trace("msiexec exit " + process.ExitCode);
                    return process.ExitCode;
                }
            }
        }
        catch (Exception error)
        {
            Trace("error " + error);
            if (!quiet) MessageBox(IntPtr.Zero, "Installation could not start. Download the installer again, or use the standalone MSI package.", "WW-StudyApp Setup", 0x10);
            return 1603;
        }
        finally
        {
            // The only cleanup candidates are our exact file and our empty directory.
            // Leave failures for diagnosis; never enumerate/delete adjacent content.
            if (package != null && packageIdentity.HasValue) { try { DeleteOwnedPackage(package, packageIdentity.Value); } catch (IOException) {} catch (UnauthorizedAccessException) {} }
            for (int index = guards.Count - 1; index >= 0; index--) guards[index].Dispose();
            if (directory != null) { try { Directory.Delete(directory, false); } catch (IOException) {} catch (UnauthorizedAccessException) {} }
        }
    }
}
