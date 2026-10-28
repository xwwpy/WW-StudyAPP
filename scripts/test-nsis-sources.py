"""Headless contracts for the current per-user NSIS installer."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class NsisSources(unittest.TestCase):
    def test_registered_install_cannot_be_relocated_before_any_writes(self):
        source = (ROOT / "installer/studyapp.nsi").read_text(encoding="utf-8")
        self.assertIn('ReadRegStr $PreviousInstall HKCU "${INSTALL_KEY}" "InstallLocation"', source)
        self.assertIn('!define MUI_PAGE_CUSTOMFUNCTION_LEAVE ValidateInstallLocation', source)
        validation = source.split('Function ValidateInstallLocation\n', 1)[1].split('FunctionEnd', 1)[0]
        self.assertIn('GetFullPathName $0 "$PreviousInstall\\."', validation)
        self.assertIn('GetFullPathName $1 "$INSTDIR\\."', validation)
        self.assertIn('StrCmp $0 $1 location_same', validation)
        self.assertIn('StrCpy $INSTDIR $0', validation)
        self.assertIn('先卸载旧版本', validation)
        self.assertIn('数据', validation)
        self.assertIn('SetErrorLevel 6', validation)
        self.assertIn('Quit', validation)
        self.assertIn('/SD IDOK', validation)
        section = source.split('Section "Install"', 1)[1].split('SectionEnd', 1)[0]
        self.assertLess(section.index('Call ValidateInstallLocation'), section.index('SetOutPath'))

    def test_native_lifecycle_covers_relocation_and_equivalent_paths(self):
        source = (ROOT / "scripts/test-nsis-install.ps1").read_text(encoding="utf-8")
        self.assertIn("Assert-RejectedRelocationPreservesInstall 'interactive rejection'", source)
        self.assertIn("Assert-RejectedRelocationPreservesInstall 'silent rejection'", source)
        self.assertIn("('/D=' + $relocated)", source)
        self.assertIn('$rejected.ExitCode -eq 6', source)
        self.assertIn('$install.ToUpperInvariant()', source)
        helper = (ROOT / "scripts/nsis-directory-native.cs").read_text(encoding="utf-8")
        self.assertIn('args[2] == "reject"', helper)
        self.assertIn('process.ExitCode == 6', helper)
        self.assertIn('先卸载旧版本', helper)

    def test_directory_is_offered_before_install_and_saved_for_updates(self):
        source = (ROOT / "installer/studyapp.nsi").read_text(encoding="utf-8")
        self.assertIn("!insertmacro MUI_PAGE_DIRECTORY", source)
        self.assertLess(source.index("MUI_PAGE_WELCOME"), source.index("MUI_PAGE_DIRECTORY"))
        self.assertLess(source.index("MUI_PAGE_DIRECTORY"), source.index("MUI_PAGE_INSTFILES"))
        self.assertIn('InstallDirRegKey HKCU "${INSTALL_KEY}" "InstallLocation"', source)
        self.assertIn('WriteRegStr HKCU "${INSTALL_KEY}" "InstallLocation" "$INSTDIR"', source)
        self.assertIn("RequestExecutionLevel user", source)

    def test_desktop_shortcut_uses_executable_icon_and_is_uninstalled(self):
        source = (ROOT / "installer/studyapp.nsi").read_text(encoding="utf-8")
        self.assertIn('CreateShortCut "$DESKTOP\\${PRODUCT}.lnk" "$INSTDIR\\WW-StudyApp.exe" "" "$INSTDIR\\WW-StudyApp.exe" 0', source)
        self.assertIn('Delete "$DESKTOP\\${PRODUCT}.lnk"', source)
        self.assertNotIn("RMDir /r", source)

    def test_finish_only_launches_when_user_requests_it(self):
        source = (ROOT / "installer/studyapp.nsi").read_text(encoding="utf-8")
        self.assertIn("!define MUI_FINISHPAGE_RUN_NOTCHECKED", source)

    def test_shortcut_assertions_use_unicode_read_only_shell_link(self):
        source = (ROOT / "scripts/test-nsis-install.ps1").read_text(encoding="utf-8")
        self.assertNotIn("WScript.Shell", source)
        self.assertIn("Add-Type -Path (Join-Path $PSScriptRoot 'shortcut-reader.cs')", source)
        self.assertIn("$link = [ShortcutReader]::Read($path)", source)
        self.assertIn("$uninstallLink = [ShortcutReader]::Read(", source)
        self.assertIn("$link.IconIndex -eq 0", source)
        helper = (ROOT / "scripts/shortcut-reader.cs").read_text(encoding="utf-8")
        self.assertIn('000214F9-0000-0000-C000-000000000046', helper)
        self.assertIn('((IPersistFile)instance).Load(', helper)
        self.assertIn('SLGP_RAWPATH = 4', helper)
        self.assertIn('FinalReleaseComObject(instance)', helper)
        for forbidden in ('Resolve(', '.Save(', 'Process.Start(', 'ShellExecute'):
            self.assertNotIn(forbidden, helper)

    def test_native_acceptance_is_runner_guarded_before_any_side_effects(self):
        source = (ROOT / "scripts/test-nsis-install.ps1").read_text(encoding="utf-8")
        guard = source.index("$env:GITHUB_ACTIONS -ne 'true'")
        for operation in ("Add-Type", "Resolve-Path", "CreateDirectory", "Start-Process"):
            self.assertLess(guard, source.index(operation))
        self.assertIn("Invoke-Installer @('/S')", source)
        self.assertNotIn('"/D=$install"', source)
        helper = (ROOT / "scripts/nsis-directory-native.cs").read_text(encoding="utf-8")
        guard = helper.index('Environment.GetEnvironmentVariable("GITHUB_ACTIONS")')
        self.assertLess(guard, helper.index("process = Process.Start("))
        for forbidden in ("SetForegroundWindow", "SendInput", "SetCursorPos", "Clipboard"):
            self.assertNotIn(forbidden, helper)


if __name__ == "__main__":
    unittest.main()
