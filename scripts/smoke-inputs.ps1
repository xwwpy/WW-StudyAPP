#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$DataDirectory,
    [string]$BinaryPath = (Join-Path $PSScriptRoot '../target/debug/studyapp.exe')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# Native character/focus tests may operate only on the isolated runner desktop.
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'Native input diagnostics run only on the isolated GitHub CI desktop' }
if (-not $env:RUNNER_TEMP) { throw 'Native input diagnostics require RUNNER_TEMP' }
$inputsBinary = (Resolve-Path -LiteralPath $BinaryPath).Path
$inputsData = [IO.Path]::GetFullPath($DataDirectory)
$inputsRunner = [IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if (-not $inputsData.StartsWith($inputsRunner, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Input diagnostics data must be under the isolated runner temporary directory'
}
$inputsStdout = "$inputsData.stdout.log"
$inputsStderr = "$inputsData.stderr.log"
$inputsInventoryPath = "$inputsData.ime-inventory.json"
foreach ($inputsPath in @($inputsData, $inputsStdout, $inputsStderr, $inputsInventoryPath)) {
    if (Test-Path -LiteralPath $inputsPath) { throw 'Input diagnostics require fresh data and log paths' }
}
# Read-only inventory. A language profile or IMM context is evidence of
# availability, not proof that composition/candidate confirmation occurred.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class InputImeInventory {
    [DllImport("user32.dll")] public static extern int GetKeyboardLayoutList(int count, [Out] IntPtr[] layouts);
    [DllImport("user32.dll")] public static extern IntPtr GetKeyboardLayout(uint thread);
    [DllImport("imm32.dll")] [return: MarshalAs(UnmanagedType.Bool)] public static extern bool ImmIsIME(IntPtr layout);
}
'@
Add-Type -TypeDefinition @'
// Read-only TSF inventory snippet for the existing CI-only Add-Type block.
// No language registration, activation, input injection, or capability changes.
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public static class InputTsfInventory
{
    [StructLayout(LayoutKind.Sequential)]
    private struct Profile
    {
        public uint Type;
        public ushort Language;
        public Guid Clsid;
        public Guid ProfileId;
        public Guid Category;
        public IntPtr SubstituteHkl;
        public uint Capabilities;
        public IntPtr Hkl;
        public uint Flags;
    }

    [ComImport, Guid("71C6E74C-0F28-11D8-A82A-00065B84435C"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface ProfileManager
    {
        // The first three methods preserve the native vtable order; never called.
        [PreserveSig] int ActivateProfile(uint type, ushort language, ref Guid clsid, ref Guid profile, IntPtr hkl, uint flags);
        [PreserveSig] int DeactivateProfile(uint type, ushort language, ref Guid clsid, ref Guid profile, IntPtr hkl, uint flags);
        [PreserveSig] int GetProfile(uint type, ushort language, ref Guid clsid, ref Guid profile, IntPtr hkl, out Profile result);
        [PreserveSig] int EnumProfiles(ushort language, out ProfileEnumerator result);
    }

    [ComImport, Guid("71C6E74D-0F28-11D8-A82A-00065B84435C"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface ProfileEnumerator
    {
        [PreserveSig] int Clone(out ProfileEnumerator result);
        [PreserveSig] int Next(uint count, out Profile result, out uint fetched);
        [PreserveSig] int Reset();
        [PreserveSig] int Skip(uint count);
    }

    public sealed class ProfileInfo
    {
        public uint Type { get; set; }
        public string Language { get; set; }
        public string Clsid { get; set; }
        public string ProfileId { get; set; }
        public string Category { get; set; }
        public string Hkl { get; set; }
        public uint Flags { get; set; }
        public bool Enabled { get; set; }
        public bool ActiveInInventoryProcess { get; set; }
        public bool IsMicrosoftPinyin { get; set; }
    }

    public sealed class Result
    {
        public string Error { get; set; }
        public int EnumHresult { get; set; }
        public int LastNextHresult { get; set; }
        public bool Complete { get; set; }
        public bool PinyinEnumerated { get; set; }
        public bool PinyinEnabled { get; set; }
        public List<ProfileInfo> Profiles { get; set; } = new List<ProfileInfo>();
    }

    public static Result ReadChineseProfiles()
    {
        var result = new Result();
        object instance = null;
        ProfileEnumerator entries = null;
        try
        {
            var classId = new Guid("33C53A50-F456-4884-B049-85FD643ECFED");
            var pinyinClass = new Guid("81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E");
            var pinyinProfile = new Guid("FA550B04-5AD7-411F-A5AC-CA038EC515D7");
            instance = Activator.CreateInstance(Type.GetTypeFromCLSID(classId, true));
            var manager = (ProfileManager)instance;
            result.EnumHresult = manager.EnumProfiles(0x0804, out entries);
            if (result.EnumHresult < 0) Marshal.ThrowExceptionForHR(result.EnumHresult);
            if (entries == null) throw new InvalidOperationException("EnumProfiles returned no enumerator");
            for (int index = 0; index < 512; index++)
            {
                Profile native;
                uint fetched;
                result.LastNextHresult = entries.Next(1, out native, out fetched);
                if (result.LastNextHresult < 0) Marshal.ThrowExceptionForHR(result.LastNextHresult);
                if (fetched == 0)
                {
                    result.Complete = true;
                    return result;
                }
                if (fetched != 1) throw new InvalidOperationException("Unexpected TSF profile count");
                bool pinyin = native.Type == 1 && native.Language == 0x0804 &&
                    native.Clsid == pinyinClass && native.ProfileId == pinyinProfile;
                bool enabled = (native.Flags & 2) != 0;
                result.PinyinEnumerated |= pinyin;
                result.PinyinEnabled |= pinyin && enabled;
                result.Profiles.Add(new ProfileInfo {
                    Type = native.Type,
                    Language = native.Language.ToString("X4"),
                    Clsid = native.Clsid.ToString("B"),
                    ProfileId = native.ProfileId.ToString("B"),
                    Category = native.Category.ToString("B"),
                    Hkl = native.Hkl.ToString("X"),
                    Flags = native.Flags,
                    Enabled = enabled,
                    ActiveInInventoryProcess = (native.Flags & 1) != 0,
                    IsMicrosoftPinyin = pinyin
                });
            }
            result.Error = "TSF profile inventory exceeded the 512 item bound";
        }
        catch (Exception error)
        {
            result.Error = error.GetType().Name + ": " + error.Message;
        }
        finally
        {
            if (entries != null && Marshal.IsComObject(entries)) Marshal.ReleaseComObject(entries);
            if (instance != null && Marshal.IsComObject(instance)) Marshal.ReleaseComObject(instance);
        }
        return result;
    }
}
'@
$inputsLayouts = New-Object IntPtr[] ([InputImeInventory]::GetKeyboardLayoutList(0, $null))
[void][InputImeInventory]::GetKeyboardLayoutList($inputsLayouts.Length, $inputsLayouts)
$inputsLanguages = try { @(Get-WinUserLanguageList | ForEach-Object {
    @{ LanguageTag = $_.LanguageTag; InputMethodTips = @($_.InputMethodTips) }
}) } catch { @(@{ inventoryError = $_.Exception.Message }) }
$inputsOperatingSystem = try {
    Get-CimInstance Win32_OperatingSystem -ErrorAction Stop |
        Select-Object Caption, Version, BuildNumber, OSArchitecture, ProductType
} catch { @{ inventoryError = $_.Exception.Message } }
$inputsChineseTyping = try { @(Get-WindowsCapability -Online -Name 'Language.Basic~~~zh-CN~0.0.1.0' -LimitAccess -ErrorAction Stop |
    ForEach-Object { @{ name = $_.Name; state = $_.State.ToString() } })
} catch { @(@{ inventoryError = $_.Exception.Message }) }
$inputsImeInventory = @{
    operatingSystem = $inputsOperatingSystem
    runnerImage = @{ os = $env:ImageOS; version = $env:ImageVersion }
    chineseBasicTyping = $inputsChineseTyping
    tsfChinese = [InputTsfInventory]::ReadChineseProfiles()
    activeKeyboardLayout = [InputImeInventory]::GetKeyboardLayout(0).ToString('X')
    layouts = @($inputsLayouts | ForEach-Object {
        @{ hkl = $_.ToString('X'); immIsIme = [InputImeInventory]::ImmIsIME($_) }
    })
    userLanguages = $inputsLanguages
    note = 'Read-only runner inventory; native-inputs-evidence.json records app HWND IMM context. Real IME composition is not asserted by this scenario.'
} | ConvertTo-Json -Depth 8
# Keep environment evidence even if startup fails before creating the library.
Set-Content -LiteralPath $inputsInventoryPath -Value $inputsImeInventory -Encoding utf8
$inputsOldScenario = [Environment]::GetEnvironmentVariable('WW_STUDYAPP_SMOKE_SCENARIO', 'Process')
try {
    $env:WW_STUDYAPP_SMOKE_SCENARIO = 'inputs'
    $inputsProcess = Start-Process -FilePath $inputsBinary -ArgumentList @('--smoke-test', '--data-dir', ('"' + $inputsData + '"')) -PassThru -WindowStyle Hidden -RedirectStandardOutput $inputsStdout -RedirectStandardError $inputsStderr
    if (-not $inputsProcess.WaitForExit(120000)) {
        $inputsProcess.Kill()
        throw 'Isolated input diagnostics exceeded 120 seconds'
    }
    if ($inputsProcess.ExitCode -ne 0) {
        Get-Content -LiteralPath $inputsStderr -Tail 80
        throw "Input diagnostics exit code: $($inputsProcess.ExitCode)"
    }
    $inputsEvidence = Get-Content -LiteralPath (Join-Path $inputsData 'native-inputs-evidence.json') -Raw | ConvertFrom-Json
    foreach ($inputsRequired in @('same-page-search:after-type', 'settings-number:after-type', 'learning-tab:after-navigation', 'appearance-virtual-scroll:unmounted', 'appearance-virtual-scroll:remounted', 'appearance-virtual-scroll:after-type', 'folder-create-modal:after-type', 'record-date-filter:after-type')) {
        if (-not @($inputsEvidence | Where-Object { $_.stage -eq $inputsRequired }).Count) {
            throw "Input diagnostics missed required stage: $inputsRequired"
        }
    }
    if ((Get-Content -LiteralPath (Join-Path $inputsData 'native-inputs-result.txt') -Raw) -notmatch '^PASS: strict Win32 SendInput') {
        throw 'Native GPUI character delivery was not verified'
    }
    Write-Output "Native input diagnostics passed; focus evidence retained at $inputsData"
}
finally {
    [Environment]::SetEnvironmentVariable('WW_STUDYAPP_SMOKE_SCENARIO', $inputsOldScenario, 'Process')
    if (Test-Path -LiteralPath $inputsData -PathType Container) {
        Set-Content -LiteralPath (Join-Path $inputsData 'ime-inventory.json') -Value $inputsImeInventory -Encoding utf8
        if (Test-Path -LiteralPath $inputsStdout) { Copy-Item -LiteralPath $inputsStdout -Destination (Join-Path $inputsData 'smoke.stdout.log') }
        if (Test-Path -LiteralPath $inputsStderr) { Copy-Item -LiteralPath $inputsStderr -Destination (Join-Path $inputsData 'smoke.stderr.log') }
    }
}
