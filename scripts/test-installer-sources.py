"""Silent source gates; actual compilation/installation is a Windows CI task."""
from pathlib import Path
import json
import re
import os
import shutil
import subprocess
import unittest
import uuid
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
NS = {"w": "http://wixtoolset.org/schemas/v4/wxs"}


class InstallerSources(unittest.TestCase):
    def check_package_dependency_cache(self, offline_exit, online_exit, expected_calls, error):
        # Load only the real helper; never execute packaging or a real Cargo process.
        command = r'''
$ErrorActionPreference = 'Stop'
$tokens = $null
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($env:PACKAGE_SCRIPT, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw "Package script has parse errors: $parseErrors" }
$helper = $ast.Find({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Initialize-PackageDependencyCache' }, $true)
if (-not $helper) { throw 'Missing Initialize-PackageDependencyCache helper' }
. ([scriptblock]::Create($helper.Extent.Text))
$script:CargoCalls = [Collections.Generic.List[string]]::new()
function cargo {
    $script:CargoCalls.Add(($args -join ' '))
    if ('--offline' -in $args) {
        Write-Error 'offline cache probe diagnostic' -ErrorAction Continue
        $global:LASTEXITCODE = [int]$env:CACHE_OFFLINE_EXIT
    } else {
        $global:LASTEXITCODE = [int]$env:CACHE_ONLINE_EXIT
    }
}
$failure = $null
$output = @()
try { $output = @(Initialize-PackageDependencyCache 2>&1) } catch { $failure = $_.Exception.Message }
[pscustomobject]@{ Calls = @($script:CargoCalls); Output = @($output | ForEach-Object { "$_" }); Failure = $failure } | ConvertTo-Json -Compress
'''
        env = dict(os.environ, PACKAGE_SCRIPT=str(ROOT / "scripts/package.ps1"),
                   CACHE_OFFLINE_EXIT=str(offline_exit), CACHE_ONLINE_EXIT=str(online_exit))
        result = subprocess.run([shutil.which("pwsh"), "-NoProfile", "-Command", command],
                                env=env, capture_output=True, text=True, encoding="utf-8", timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        observed = json.loads(result.stdout)
        self.assertEqual(observed["Calls"], expected_calls)
        self.assertEqual(observed["Output"], [], "Expected offline probe diagnostics must remain quiet")
        if error:
            self.assertIn("Locked Cargo dependency fetch failed", observed["Failure"])
        else:
            self.assertIsNone(observed["Failure"])

    def test_package_dependency_cache_hit_stays_offline(self):
        self.check_package_dependency_cache(0, 99, [
            "fetch --locked --offline --target x86_64-pc-windows-msvc"], False)

    def test_package_dependency_cache_miss_fetches_locked_windows_dependencies(self):
        self.check_package_dependency_cache(101, 0, [
            "fetch --locked --offline --target x86_64-pc-windows-msvc",
            "fetch --locked --target x86_64-pc-windows-msvc"], False)

    def test_package_dependency_cache_online_failure_stops_packaging(self):
        self.check_package_dependency_cache(101, 101, [
            "fetch --locked --offline --target x86_64-pc-windows-msvc",
            "fetch --locked --target x86_64-pc-windows-msvc"], True)

    def test_package_dependency_cache_precedes_metadata_and_staging(self):
        code = (ROOT / "scripts/package.ps1").read_text(encoding="utf-8")
        start = code.index("Push-Location -LiteralPath $repoRoot")
        self.assertIn("    Initialize-PackageDependencyCache", code[start:])
        prepare = code.index("    Initialize-PackageDependencyCache", start)
        metadata = code.index("cargo metadata --locked --offline --all-features --filter-platform x86_64-pc-windows-msvc", start)
        stage = code.index("[IO.Directory]::CreateDirectory($staging)", start)
        self.assertLess(prepare, metadata)
        self.assertLess(metadata, stage)

    def test_release_wrapper_preserves_existing_output_and_temporary_files(self):
        code = (ROOT / "scripts/package-release.ps1").read_text(encoding="utf-8")
        self.assertNotRegex(code, r"Get-ChildItem[^\n]+\|\s*Remove-Item")
        self.assertNotRegex(code, r"(?i)\brd\s+/s|\brmdir\s+/s|Remove-Item[^\n]+-Recurse")
        self.assertIn("Output directory must be empty", code)

    def test_release_packaging_rejects_nonempty_output_without_removing_files(self):
        root = ROOT / ".scratch" / "installer-wrapper-tests" / uuid.uuid4().hex
        payload = root / "payload"
        output = root / "existing-output"
        payload.mkdir(parents=True)
        output.mkdir()
        setup = root / "WW-StudyApp-1.2.3-Setup.exe"
        setup.write_bytes(b"package fixture; never executed")
        (payload / "WW-StudyApp.exe").write_bytes(b"payload fixture; never executed")
        marker = output / "keep.txt"
        marker.write_bytes(b"existing output must survive")
        command = [shutil.which("pwsh"), "-NoProfile", "-File", str(ROOT / "scripts/package-release.ps1"),
                   "-Version", "1.2.3", "-SetupPath", str(setup), "-PayloadDirectory", str(payload),
                   "-OutputDirectory", str(output)]
        result = subprocess.run(command, capture_output=True, timeout=30)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"Output directory must be empty", result.stderr)
        self.assertEqual(marker.read_bytes(), b"existing output must survive")
        self.assertEqual([p.name for p in output.iterdir()], ["keep.txt"])

    def test_private_native_msi_uses_test_binary_without_changing_public_payload(self):
        build=(ROOT / "scripts/build-installers.ps1").read_text(encoding="utf-8")
        self.assertIn('NativeValidation-0.0.1.msi',build)
        self.assertIn('$UiTestBinaryPath',build)
        self.assertIn('native-validation-payload',build)
        package=(ROOT / "scripts/package.ps1").read_text(encoding="utf-8")
        self.assertIn('-UiTestBinaryPath $UiTestBinaryPath',package)
        source=(ROOT / "crates/studyapp-desktop/src/maintenance_acceptance.rs").read_text(encoding="utf-8")
        self.assertIn('begin_update_handoff_for_acceptance(&prepared)',source)
        self.assertIn('prepared.with_restart_data_directory(Some(&library))',source)
        self.assertNotIn('stage.join("package.msi")',source)

    def test_msi_callback_uses_native_context_type_message_order(self):
        source = (ROOT / "scripts/msi-control-native-tests.cs").read_text(encoding="utf-8")
        self.assertIn("InstallUiHandler(IntPtr context, uint type, string message)", source)
        self.assertIn("CancelAtFiles(IntPtr context, uint type, string message)", source)

    def test_controlled_cancel_and_reboot_use_real_windows_installer(self):
        source = (ROOT / "scripts/msi-control-native-tests.cs").read_text(encoding="utf-8")
        for api in ("MsiInstallProductW", "MsiSetExternalUIW", "MsiSetInternalUI", "InstallFiles", "1602"):
            self.assertIn(api, source)
        build = (ROOT / "scripts/build-installers.ps1").read_text(encoding="utf-8")
        self.assertIn('<ScheduleReboot Before="InstallFinalize" Condition="NOT Installed"', build)
        self.assertIn("RebootSetup.exe", build)
        self.assertIn("msi-control-native-tests.cs", build)

    def test_runner_evidence_paths_are_emitted_before_failing_work(self):
        code = (ROOT / "scripts/build-installers.ps1").read_text(encoding="utf-8")
        self.assertLess(code.index('"installer-evidence=$work"'), code.index("Invoke-WebRequest"))
        self.assertLess(code.index('"installer-fixtures=$fixtures"'), code.index("& $wix build $oldWxs"))

    def test_runner_runs_real_native_sharing_and_identity_cleanup_probe(self):
        probe = (ROOT / "scripts/setup-native-tests.cs").read_text(encoding="utf-8")
        for required in ("MsiOpenDatabaseW", '"OpenVerifiedPackage"', '"DeleteOwnedPackage"', '"GetNativeIdentity"', "FileMode.CreateNew", "FileAccess.Read, FileShare.Read", "File.Move(package, renamed)"):
            self.assertIn(required, probe)
        build = (ROOT / "scripts/build-installers.ps1").read_text(encoding="utf-8")
        self.assertIn("/main:SetupNativeTests", build)
        self.assertIn("setup-native-tests.cs", build)
        self.assertIn("$nativeProbe.ExitCode -ne 0", build)

    def test_wrapper_cleanup_deletes_only_the_captured_native_file_handle(self):
        code = (ROOT / "installer/Setup.cs").read_text(encoding="utf-8")
        self.assertNotIn("File.Delete(package)", code)
        cleanup = code[code.index("private static void DeleteOwnedPackage"):code.index("[STAThread]")]
        self.assertIn("0x00010080, 0", cleanup)  # DELETE | READ_ATTRIBUTES, exclusive
        self.assertIn("0x00200000", cleanup)
        self.assertLess(cleanup.index("SameIdentity("), cleanup.index("SetFileInformationByHandle("))
        self.assertIn("GetNativeIdentity(handle)", cleanup)
        writer = code[code.index("using (var staged = new FileStream("):]
        self.assertLess(writer.index("packageIdentity = GetNativeIdentity(staged.SafeFileHandle)"), writer.index("resource.CopyTo(staged)"))
        final = code[code.rindex("finally"):]
        self.assertLess(final.index("DeleteOwnedPackage("), final.index("guards[index].Dispose()"))
        self.assertIn("Directory.Delete(directory, false)", final)

    def test_wrapper_closes_writer_then_leases_same_native_file_readonly(self):
        code = (ROOT / "installer/Setup.cs").read_text(encoding="utf-8")
        marker = "using (var staged = new FileStream("
        start = code.index(marker)
        begin = code.index("{", start)
        depth = 1
        end = begin + 1
        while depth:
            depth += (code[end] == "{") - (code[end] == "}")
            end += 1
        writer = code[begin:end]
        self.assertNotIn("Process.Start", writer)
        self.assertIn("GetNativeIdentity(staged.SafeFileHandle)", writer)
        lease = code.index("using (FileStream verified = OpenVerifiedPackage(", end)
        lease_begin = code.index("{", lease)
        depth = 1
        lease_end = lease_begin + 1
        while depth:
            depth += (code[lease_end] == "{") - (code[lease_end] == "}")
            lease_end += 1
        self.assertIn("Process.Start", code[lease_begin:lease_end])
        self.assertIn("process.WaitForExit()", code[lease_begin:lease_end])
        reader = code[code.index("private static FileStream OpenVerifiedPackage"):code.index("[STAThread]")]
        self.assertIn("0x80000000, 1", reader)  # GENERIC_READ, FILE_SHARE_READ
        self.assertIn("0x00200000", reader)  # OPEN_REPARSE_POINT
        self.assertIn("FileAccess.Read", reader)
        self.assertNotIn("FileAccess.ReadWrite", reader)
        self.assertIn("GetNativeIdentity(handle)", reader)
        self.assertLess(reader.index("SameIdentity("), reader.index("VerifyDigest("))
        self.assertIn("GetFileInformationByHandleEx", code)
        for native_field in ("ulong VolumeSerial", "ulong FileIdLow", "ulong FileIdHigh"):
            self.assertIn(native_field, code)
        self.assertIn("FileAttributes.ReparsePoint | FileAttributes.Directory", code)

    def test_user_profile_directory_cleanup_is_explicit_and_empty_only(self):
        package = ET.parse(ROOT / "installer/Product.wxs").find("w:Package", NS)
        removals = {}
        for component in package.findall(".//w:Component", NS):
            for row in component.findall("w:RemoveFolder", NS):
                removals[row.attrib["Id"]] = (component.attrib["Id"], row.attrib.get("Directory"), row.attrib["On"])
        self.assertEqual(removals, {
            "RemoveInstall": ("Application", None, "uninstall"),
            "RemovePrograms": ("Application", "PROGRAMS", "uninstall"),
            "RemoveSupport": ("License", None, "uninstall"),
            "RemoveMenu": ("Shortcuts", None, "uninstall"),
        })

    def test_local_runner_guards_have_no_side_effects(self):
        shell = shutil.which("pwsh")
        self.assertIsNotNone(shell, "PowerShell 7 is required for this silent gate")
        env = dict(os.environ)
        env.pop("GITHUB_ACTIONS", None)
        env.pop("RUNNER_OS", None)
        for script, arguments in (
            ("build-installers.ps1", ["-Version", "1.2.3", "-PayloadDirectory", "missing", "-OutputDirectory"]),
            ("smoke-maintenance.ps1", ["-OutputDirectory"]),
            ("package.ps1", ["-Version", "1.2.3", "-IncludeInstallers", "-OutputDirectory"]),
        ):
            path = ROOT / ".scratch" / "installation-20261026" / ("guard-" + uuid.uuid4().hex)
            result = subprocess.run([shell, "-NoProfile", "-File", str(ROOT / "scripts" / script), *arguments, str(path)],
                                    env=env, capture_output=True, text=True, encoding="utf-8", timeout=30)
            self.assertNotEqual(result.returncode, 0, script)
            self.assertIn("Windows CI runner", result.stderr, script)
            self.assertFalse(path.exists(), script)

    def test_explicit_payload_and_stable_distinct_components(self):
        tree = ET.parse(ROOT / "installer/Product.wxs")
        package = tree.find("w:Package", NS)
        self.assertEqual(package.attrib["Scope"], "perUser")
        self.assertEqual(package.attrib["UpgradeCode"], "{4CF5A7C2-094D-4D97-AB25-24C6C2BAA4CD}")
        self.assertEqual(package.attrib["ProductCode"], "*")
        upgrade = package.find("w:MajorUpgrade", NS)
        self.assertEqual(upgrade.attrib["Schedule"], "afterInstallInitialize")
        self.assertEqual(upgrade.attrib["AllowSameVersionUpgrades"], "yes")
        components = package.findall(".//w:Component", NS)
        self.assertEqual(len(components), 8)
        self.assertEqual(len({c.attrib["Guid"] for c in components}), 8)
        files = package.findall(".//w:File", NS)
        self.assertEqual({f.attrib["Source"].split("\\")[-1] for f in files}, {
            "WW-StudyApp.exe", "Uninstall.exe", "vcruntime140.dll", "vcruntime140_1.dll", "LICENSE", "THIRD-PARTY-NOTICES.txt", "manifest.json"})
        self.assertEqual(len(files), 7)
        for component in components:
            registry = component.find("w:RegistryValue", NS)
            self.assertEqual(registry.attrib["Root"], "HKCU")
            self.assertEqual(registry.attrib["KeyPath"], "yes")
        for forbidden in ("CustomAction", "RemoveFile", "RemoveFolderEx", "Binary"):
            self.assertFalse(package.findall(f".//w:{forbidden}", NS))

    def test_wrapper_has_only_as_invoker_privilege(self):
        tree = ET.parse(ROOT / "installer/setup.manifest")
        privileges = list(tree.iter("{urn:schemas-microsoft-com:asm.v3}requestedExecutionLevel"))
        self.assertEqual([p.attrib for p in privileges], [{"level": "asInvoker", "uiAccess": "false"}])
        code = (ROOT / "installer/Setup.cs").read_text(encoding="utf-8")
        for required in ("StudyApp.Payload.msi", "StudyApp.Payload.sha256", "FileMode.CreateNew", "FileShare.Read", "UseShellExecute = false", "process.WaitForExit()", "return process.ExitCode", "Directory.Delete(directory, false)"):
            self.assertIn(required, code)
        self.assertNotRegex(code, r"(?:Verb\s*=\s*\"runas|Directory\.Delete\([^\n]+true|Process\.Start\(\s*\"(?:cmd|powershell))")

    def test_ci_only_entrypoints_guard_before_mutation(self):
        for name in ("build-installers.ps1", "smoke-maintenance.ps1"):
            code = (ROOT / "scripts" / name).read_text(encoding="utf-8-sig")
            guard = code.index("$env:GITHUB_ACTIONS -ne 'true'")
            for operation in ("Invoke-WebRequest", "Start-Process", "[IO.Directory]::CreateDirectory"):
                position = code.find(operation)
                if position >= 0:
                    self.assertLess(guard, position)
            self.assertNotRegex(code, r"(?i)Remove-Item[^\n]+-Recurse|rm -rf|rmdir /s|rd /s|del /s")


if __name__ == "__main__":
    unittest.main()
