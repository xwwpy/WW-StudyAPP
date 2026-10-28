; WW-StudyApp per-user NSIS installer.
;
; Mirrors the packaging model of xwwpy/cargo-sweep: per-user install under
; %LOCALAPPDATA%\Programs, Start Menu shortcuts, an uninstaller registered in
; the current-user uninstall hive, silent (/S) support for the in-app updater,
; and no access to the user's library data.
;
; Defines: VERSION, PAYLOAD, ICON, OUTFILE (all supplied by the build script).

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !error "VERSION define is required"
!endif
!ifndef PAYLOAD
  !error "PAYLOAD define is required"
!endif
!ifndef ICON
  !error "ICON define is required"
!endif
!ifndef OUTFILE
  !error "OUTFILE define is required"
!endif

!define PRODUCT "WW-StudyApp"
!define PUBLISHER "WW-StudyApp"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}"
!define INSTALL_KEY "Software\WW-StudyApp\Installer"

Name "${PRODUCT}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\WW-StudyApp"
InstallDirRegKey HKCU "${INSTALL_KEY}" "InstallLocation"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ShowInstDetails nevershow
ShowUninstDetails nevershow
BrandingText "${PRODUCT} ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${PRODUCT}"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "FileDescription" "${PRODUCT} Setup"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "${PUBLISHER}"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\WW-StudyApp.exe"
!define MUI_FINISHPAGE_RUN_TEXT "运行 ${PRODUCT}"
!define MUI_FINISHPAGE_RUN_NOTCHECKED
!insertmacro MUI_PAGE_WELCOME
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE ValidateInstallLocation
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

Var Updated
Var PreviousInstall

; A second directory would leave an old uninstaller able to remove the shared
; shortcuts and registration. Require uninstall first; uninstall retains data.
; Appending a dot normalizes trailing separators as well as relative segments.
Function ValidateInstallLocation
  StrCmp $PreviousInstall "" location_valid
  GetFullPathName $0 "$PreviousInstall\."
  GetFullPathName $1 "$INSTDIR\."
  ; StrCmp is case-insensitive, matching Windows directory name semantics.
  StrCmp $0 $1 location_same
  MessageBox MB_OK|MB_ICONEXCLAMATION "已安装 WW-StudyApp，无法直接更换安装目录。请先卸载旧版本，再选择新目录安装。卸载会保留学习资料和个人数据。$\r$\n原安装目录：$PreviousInstall" /SD IDOK
  SetErrorLevel 6
  Quit
location_same:
  ; Keep registration and shortcut paths stable for equivalent /D spellings.
  StrCpy $INSTDIR $0
location_valid:
FunctionEnd

; The updater starts the installer and then exits. Silent installs therefore wait
; briefly for WW-StudyApp.exe to leave before touching its files, and restart it
; afterwards when the /UPDATED switch was supplied. Interactive installs still
; ask the user to close the running application.
Function .onInit
  ReadRegStr $PreviousInstall HKCU "${INSTALL_KEY}" "InstallLocation"
  StrCpy $Updated 0
  ${GetParameters} $R0
  ${GetOptions} $R0 "/UPDATED" $R1
  ${IfNot} ${Errors}
    StrCpy $Updated 1
  ${EndIf}

  ${If} ${Silent}
    StrCpy $0 0
    ${Do}
      nsExec::ExecToStack 'cmd /c tasklist /FI "IMAGENAME eq WW-StudyApp.exe" /NH | find /i "WW-StudyApp.exe"'
      Pop $1
      Pop $2
      ${If} $2 == ""
        ${ExitDo}
      ${EndIf}
      Sleep 500
      IntOp $0 $0 + 1
      ${If} $0 >= 120
        SetErrorLevel 5
        Abort
      ${EndIf}
    ${Loop}
  ${Else}
    nsExec::ExecToStack 'cmd /c tasklist /FI "IMAGENAME eq WW-StudyApp.exe" /NH | find /i "WW-StudyApp.exe"'
    Pop $0
    Pop $1
    ${If} $1 != ""
      MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION "WW-StudyApp 正在运行。请先退出应用，再继续安装。" IDOK 0
      Abort
    ${EndIf}
  ${EndIf}
FunctionEnd

Function un.onInit
  SetShellVarContext current
FunctionEnd

Section "Install"
  ; Also enforce for /S /D=... because silent installs skip directory pages.
  Call ValidateInstallLocation
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  CreateDirectory "$INSTDIR"
  File "${PAYLOAD}\WW-StudyApp.exe"
  File "${PAYLOAD}\vcruntime140.dll"
  File "${PAYLOAD}\vcruntime140_1.dll"
  File "${PAYLOAD}\README.md"
  File "${PAYLOAD}\LICENSE"
  File "${PAYLOAD}\RELEASE-NOTES.md"
  File "${PAYLOAD}\THIRD-PARTY-NOTICES.txt"

  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateDirectory "$SMPROGRAMS\${PRODUCT}"
  CreateShortCut "$DESKTOP\${PRODUCT}.lnk" "$INSTDIR\WW-StudyApp.exe" "" "$INSTDIR\WW-StudyApp.exe" 0
  CreateShortCut "$SMPROGRAMS\${PRODUCT}\${PRODUCT}.lnk" "$INSTDIR\WW-StudyApp.exe" "" "$INSTDIR\WW-StudyApp.exe" 0
  CreateShortCut "$SMPROGRAMS\${PRODUCT}\卸载 ${PRODUCT}.lnk" "$INSTDIR\Uninstall.exe"

  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\WW-StudyApp.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1

  WriteRegStr HKCU "${INSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${INSTALL_KEY}" "Version" "${VERSION}"

  ; Only the updater asks for an automatic restart; a plain /S install stays
  ; closed so unattended fixtures can inspect the files.
  ${If} $Updated == 1
    ExecShell "" "$INSTDIR\WW-StudyApp.exe"
  ${EndIf}
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  ; Never touch the user's library (for example %APPDATA%\StudyApp).
  Delete "$INSTDIR\WW-StudyApp.exe"
  Delete "$INSTDIR\vcruntime140.dll"
  Delete "$INSTDIR\vcruntime140_1.dll"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\RELEASE-NOTES.md"
  Delete "$INSTDIR\THIRD-PARTY-NOTICES.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${PRODUCT}\${PRODUCT}.lnk"
  Delete "$DESKTOP\${PRODUCT}.lnk"
  Delete "$SMPROGRAMS\${PRODUCT}\卸载 ${PRODUCT}.lnk"
  RMDir "$SMPROGRAMS\${PRODUCT}"

  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey HKCU "${INSTALL_KEY}"
SectionEnd
