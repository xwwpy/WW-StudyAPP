// Compiled with the production Setup.cs source on the isolated Windows runner.
// No installer is run; the real MSI database is opened under the actual lease.
using System;
using System.Collections.Generic;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using Microsoft.Win32.SafeHandles;

internal static class SetupNativeTests
{
    [DllImport("msi.dll", CharSet = CharSet.Unicode)]
    private static extern uint MsiOpenDatabaseW(string path, IntPtr persistence, out uint database);
    [DllImport("msi.dll")]
    private static extern uint MsiCloseHandle(uint handle);

    private static object Call(string name, params object[] arguments)
    {
        MethodInfo method = typeof(Setup).GetMethod(name, BindingFlags.Static | BindingFlags.NonPublic);
        if (method == null) throw new InvalidOperationException("Production method missing: " + name);
        return method.Invoke(null, arguments);
    }
    private static void Require(bool value, string message)
    {
        if (!value) throw new InvalidOperationException(message);
    }
    private static string Digest(Stream stream)
    {
        stream.Position = 0;
        using (var sha = SHA256.Create()) return BitConverter.ToString(sha.ComputeHash(stream)).Replace("-", "").ToLowerInvariant();
    }
    private static void ExpectIoFailure(Action operation, string message)
    {
        try { operation(); }
        catch (IOException) { return; }
        catch (TargetInvocationException exception)
        {
            if (exception.InnerException is IOException) return;
            throw;
        }
        throw new InvalidOperationException(message);
    }

    private static int Main(string[] args)
    {
        if (Environment.GetEnvironmentVariable("GITHUB_ACTIONS") != "true" || Environment.GetEnvironmentVariable("RUNNER_OS") != "Windows") return 87;
        if (args.Length != 2) return 87;
        string parent = Path.GetFullPath(args[1]);
        string runner = Path.GetFullPath(Environment.GetEnvironmentVariable("RUNNER_TEMP")).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        if (!parent.StartsWith(runner, StringComparison.OrdinalIgnoreCase)) return 87;
        var guards = new List<SafeFileHandle>();
        try
        {
            Call("GuardAncestors", parent, guards);
            string root = (string)Call("CreatePrivateDirectory", parent);
            Call("GuardAncestors", root, guards);
            string package = Path.Combine(root, "StudyApp.msi");
            string renamed = Path.Combine(root, "owned-original.msi");
            object identity;
            string digest;
            using (var writer = new FileStream(package, FileMode.CreateNew, FileAccess.ReadWrite, FileShare.Read))
            {
                identity = Call("GetNativeIdentity", writer.SafeFileHandle);
                using (var source = File.OpenRead(args[0])) source.CopyTo(writer);
                writer.Flush(true);
                digest = Digest(writer);
            }
            using (var guard = (FileStream)Call("OpenVerifiedPackage", package, identity, digest))
            {
                using (var reader = new FileStream(package, FileMode.Open, FileAccess.Read, FileShare.Read))
                    Require(Digest(reader) == digest, "Ordinary read-only sharing did not consume the exact MSI.");
                uint database;
                uint opened = MsiOpenDatabaseW(package, IntPtr.Zero, out database);
                Require(opened == 0, "Windows Installer could not open the MSI database while the guard was held: " + opened);
                Require(MsiCloseHandle(database) == 0, "Windows Installer database handle did not close.");
                ExpectIoFailure(delegate { using (var writer = File.Open(package, FileMode.Open, FileAccess.Write, FileShare.ReadWrite)) {} }, "Read-only guard allowed a writer.");
                ExpectIoFailure(delegate { File.Move(package, renamed); }, "Read-only guard allowed path replacement.");
            }
            // After the read lease closes, deliberately substitute the leaf. The
            // production cleanup must reject it by ID, preserving unknown bytes.
            File.Move(package, renamed);
            const string unknown = "Unknown replacement must remain";
            using (var writer = new StreamWriter(new FileStream(package, FileMode.CreateNew, FileAccess.Write, FileShare.None))) writer.Write(unknown);
            ExpectIoFailure(delegate { Call("DeleteOwnedPackage", package, identity); }, "Cleanup accepted an unknown replacement.");
            Require(File.ReadAllText(package) == unknown, "Cleanup modified the substituted file.");
            Call("DeleteOwnedPackage", renamed, identity);
            Require(!File.Exists(renamed), "Exact native-handle cleanup retained its own original.");
            string partial = Path.Combine(root, "partial-extraction.msi");
            object partialIdentity;
            using (var writer = new FileStream(partial, FileMode.CreateNew, FileAccess.ReadWrite, FileShare.None)) partialIdentity = Call("GetNativeIdentity", writer.SafeFileHandle);
            Call("DeleteOwnedPackage", partial, partialIdentity);
            Require(!File.Exists(partial), "Exact cleanup did not support an interrupted empty extraction.");
            File.WriteAllText(Path.Combine(parent, "setup-native-sharing.txt"), "PASS: ordinary read-only and Windows Installer database opens; writes/renames denied; substituted leaf preserved; exact original and partial extraction cleaned.\r\nRetained unknown fixture: " + package);
            return 0;
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine(exception.ToString());
            return 1;
        }
        finally
        {
            for (int index = guards.Count - 1; index >= 0; index--) guards[index].Dispose();
        }
    }
}
