; glyphcam Windows installer (per-user, no admin rights needed).
;
; makensis /DVERSION=0.1.0 /DBUILDTAG=0.1.0-abcdef12 /DSRCDIR=..\..\dist\stage /DOUTFILE=..\..\dist\glyphcam-0.1.0-windows-x64-setup.exe installer.nsi
;
; Silent update (used by the in-app build manager):  setup.exe /S /RELAUNCH

Unicode true
ManifestDPIAware true
SetCompressor /SOLID lzma

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef BUILDTAG
  !define BUILDTAG "${VERSION}"
!endif
!ifndef SRCDIR
  !define SRCDIR "..\..\dist\stage"
!endif
!ifndef OUTFILE
  !define OUTFILE "..\..\dist\glyphcam-setup.exe"
!endif

!define APP "glyphcam"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\glyphcam"

Name "glyphcam ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\glyphcam"
RequestExecutionLevel user
BrandingText "glyphcam ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "glyphcam"
VIAddVersionKey "FileDescription" "glyphcam installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "MIT License"

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!define MUI_ICON "glyphcam.ico"
!define MUI_UNICON "glyphcam.ico"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "glyphcam ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "glyphcam turns your webcam into live glyph art and adds a virtual camera called $\"glyphcam$\" that Discord, Zoom, OBS, Teams and browsers can use.$\r$\n$\r$\nNo administrator rights are needed; it installs just for you."
!define MUI_FINISHPAGE_RUN "$INSTDIR\glyphcam.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start glyphcam"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function StopApp
  ; Ask a running glyphcam to exit (it may be the one that launched us).
  nsExec::Exec 'taskkill /IM glyphcam.exe'
  Sleep 700
  nsExec::Exec 'taskkill /F /IM glyphcam.exe'
  Sleep 300
FunctionEnd

Function un.StopApp
  nsExec::Exec 'taskkill /F /IM glyphcam.exe'
  Sleep 300
FunctionEnd

Section "glyphcam" SecMain
  Call StopApp

  SetOutPath "$INSTDIR"
  SetOverwrite on
  ; The exe can't be locked now; retry briefly in case it is still exiting.
  StrCpy $0 0
  retry:
    ClearErrors
    File "${SRCDIR}\glyphcam.exe"
    ${If} ${Errors}
      IntOp $0 $0 + 1
      ${If} $0 < 20
        Sleep 250
        Goto retry
      ${EndIf}
      MessageBox MB_ICONSTOP "Could not replace glyphcam.exe (is it still running?)"
      Abort
    ${EndIf}
  File "${SRCDIR}\LICENSE.txt"
  File "${SRCDIR}\THIRD-PARTY.txt"

  ; Virtual camera DLLs live in a per-build folder: apps that currently have
  ; the camera open keep the old DLL loaded, so we never overwrite it.
  SetOutPath "$INSTDIR\vcam\${BUILDTAG}"
  SetOverwrite try
  File "${SRCDIR}\glyphcam-vcam-x64.dll"
  File "${SRCDIR}\glyphcam-vcam-x86.dll"
  File "${SRCDIR}\glyphcam-mfcam.dll"
  SetOverwrite on

  ; Remove older DLL folders that aren't in use (in-use ones fail silently).
  FindFirst $1 $2 "$INSTDIR\vcam\*"
  loop:
    StrCmp $2 "" done
    StrCmp $2 "." next
    StrCmp $2 ".." next
    StrCmp $2 "${BUILDTAG}" next
    RMDir /r "$INSTDIR\vcam\$2"
  next:
    FindNext $1 $2
    Goto loop
  done:
  FindClose $1

  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Register the "glyphcam" camera for this user.
  nsExec::ExecToLog '"$INSTDIR\glyphcam.exe" --register-vcam'

  CreateShortcut "$SMPROGRAMS\glyphcam.lnk" "$INSTDIR\glyphcam.exe" "" "$INSTDIR\glyphcam.exe" 0

  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "glyphcam"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "bentsignal"
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://github.com/bentsignal/glyphcam"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\glyphcam.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $3 $4 $5
  IntFmt $3 "0x%08X" $3
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" "$3"

  ; Silent updates from the in-app build manager relaunch the app.
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/RELAUNCH" $R1
  ${IfNot} ${Errors}
    Exec '"$INSTDIR\glyphcam.exe"'
  ${EndIf}
SectionEnd

Section "Uninstall"
  Call un.StopApp
  ; Windows 11 camera: removes the per-user camera; asks for admin to
  ; remove the machine-wide part if it was set up.
  nsExec::ExecToLog '"$INSTDIR\glyphcam.exe" --remove-system-camera'
  nsExec::ExecToLog '"$INSTDIR\glyphcam.exe" --unregister-vcam'
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "glyphcam"
  Delete "$SMPROGRAMS\glyphcam.lnk"
  Delete "$INSTDIR\glyphcam.exe"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\THIRD-PARTY.txt"
  RMDir /r /REBOOTOK "$INSTDIR\vcam"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${UNINST_KEY}"
SectionEnd
