; SPDX-License-Identifier: AGPL-3.0-or-later

!include "LogicLib.nsh"
!include "WordFunc.nsh"
!include "x64.nsh"

!define ZHIYE_WEBVIEW2_CLIENT "Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
; The dynamic x64 loader shipped by the Windows 0.31.0–0.50.0 candidates.
!define ZHIYE_LEGACY_LOADER_BYTES 160880
!define ZHIYE_LEGACY_LOADER_SHA256 "86545b66cdb0603bc26b626fb9ad610cb6e71f28d468f5ea66df23b03dda96d5"
!define ZHIYE_FILE_RETRY_COUNT 8
!define ZHIYE_FILE_RETRY_DELAY_MS 125

!ifndef ZHIYE_OFFLINE_WEBVIEW2
  !if "$%ZHIYE_WEBVIEW2_INSTALLER_MODE%" == "offline"
    !define ZHIYE_OFFLINE_WEBVIEW2 "$%ZHIYE_OFFLINE_WEBVIEW2_PATH%"
  !endif
!endif

Var ZhiyeWebView2Version
Var ZhiyeWebView2Installer
Var ZhiyeWebView2Result
Var ZhiyeOverlayInstall

; A running editor can veto an ordinary close to retain an unsaved draft.
; Wait briefly for a naturally released file, then leave its owner running.
!ifmacrodef CheckIfAppIsRunning
!macroundef CheckIfAppIsRunning
!macro CheckIfAppIsRunning executablePath productName
  !insertmacro RestartManager_StartSession $R0
  ${If} $R0 != ""
    !insertmacro RestartManager_RegisterFile $R0 "${executablePath}"
    StrCpy $R4 -1
    ${If} $0 == 0
      StrCpy $R5 "${ZHIYE_FILE_RETRY_COUNT}"
      ${Do}
        System::Call 'RSTRTMGR::RmGetList(i R0, *i .r1, *i .r2, p 0, *i .r3) i.r0'
        StrCpy $R4 $0
        ${If} $R4 != 234 ; ERROR_MORE_DATA
        ${OrIf} $R5 == 0
          ${ExitDo}
        ${EndIf}
        Sleep ${ZHIYE_FILE_RETRY_DELAY_MS}
        IntOp $R5 $R5 - 1
      ${Loop}
    ${EndIf}
    !insertmacro RestartManager_EndSession $R0
    ${If} $R4 == 234
      nsis_tauri_utils::StrReplace "$(ZhiyeAppInUse)" "{{product_name}}" "${productName}"
      Pop $R1
      DetailPrint $R1
      SetErrorLevel 2
      ${IfNot} ${Silent}
      ${AndIf} $PassiveMode != 1
        MessageBox MB_ICONEXCLAMATION|MB_OK $R1
      ${EndIf}
      Quit
    ${EndIf}
  ${EndIf}
!macroend
!endif

Function ZhiyeValidateWebView2Version
  ${StrFilter} "$ZhiyeWebView2Version" "1" "." "" $ZhiyeWebView2Result
  ${If} $ZhiyeWebView2Result != $ZhiyeWebView2Version
    StrCpy $ZhiyeWebView2Version ""
    Return
  ${EndIf}
  ${WordFind} "$ZhiyeWebView2Version" "." "#" $ZhiyeWebView2Result
  ${If} $ZhiyeWebView2Result != 4
    StrCpy $ZhiyeWebView2Version ""
    Return
  ${EndIf}
  ${WordFind} "$ZhiyeWebView2Version" "." "*" $ZhiyeWebView2Result
  ${If} $ZhiyeWebView2Result != 3
    StrCpy $ZhiyeWebView2Version ""
    Return
  ${EndIf}
  ${VersionCompare} "$ZhiyeWebView2Version" "0.0.0.0" $ZhiyeWebView2Result
  ${If} $ZhiyeWebView2Result != 1
    StrCpy $ZhiyeWebView2Version ""
  ${EndIf}
FunctionEnd

Function ZhiyeReadWebView2Version
  ${If} ${RunningX64}
    ReadRegStr $ZhiyeWebView2Version HKLM "SOFTWARE\WOW6432Node\${ZHIYE_WEBVIEW2_CLIENT}" "pv"
  ${Else}
    ReadRegStr $ZhiyeWebView2Version HKLM "SOFTWARE\${ZHIYE_WEBVIEW2_CLIENT}" "pv"
  ${EndIf}
  Call ZhiyeValidateWebView2Version

  ${If} $ZhiyeWebView2Version == ""
    ReadRegStr $ZhiyeWebView2Version HKCU "SOFTWARE\${ZHIYE_WEBVIEW2_CLIENT}" "pv"
    Call ZhiyeValidateWebView2Version
  ${EndIf}
  ClearErrors
FunctionEnd

!macro ZhiyeAbortWebView2 detail
  DetailPrint "${detail}"
  SetErrorLevel 2
  Abort "$(webview2AbortError)"
!macroend

Function ZhiyeRemoveLegacyWebView2Loader
  System::Store "SF"
  StrCpy $9 "${ZHIYE_FILE_RETRY_COUNT}"

  legacy_open:
  ; Skip an installation path that is itself a reparse point.
  System::Call 'kernel32::GetFileAttributesW(w "$INSTDIR") i.r0'
  IntOp $0 $0 & 0x400
  ${If} $0 != 0
    Goto legacy_done
  ${EndIf}

  ; GENERIC_READ | DELETE, FILE_SHARE_READ, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT.
  ; Keep the same handle from hashing through deletion, with writes and renames excluded.
  System::Call 'kernel32::CreateFileW(w "$INSTDIR\WebView2Loader.dll", i 0x80010000, i 1, p 0, i 3, i 0x00200000, p 0) p.r0 ?e'
  Pop $1
  ${If} $0 == -1
    ; ERROR_SHARING_VIOLATION and ERROR_LOCK_VIOLATION can clear shortly after an update.
    ${If} $1 == 32
    ${OrIf} $1 == 33
      ${If} $9 > 0
        IntOp $9 $9 - 1
        Sleep ${ZHIYE_FILE_RETRY_DELAY_MS}
        Goto legacy_open
      ${EndIf}
      DetailPrint "The legacy WebView2 loader is still in use."
    ${EndIf}
    Goto legacy_done
  ${EndIf}
  System::Call 'kernel32::GetFileSizeEx(p r0, *l .r1) i.r2'
  ${If} $2 == 0
  ${OrIf} $1 != "${ZHIYE_LEGACY_LOADER_BYTES}"
    Goto legacy_close
  ${EndIf}

  System::Alloc ${ZHIYE_LEGACY_LOADER_BYTES}
  Pop $3
  ${If} $3 == 0
    Goto legacy_close
  ${EndIf}
  System::Call 'kernel32::ReadFile(p r0, p r3, i ${ZHIYE_LEGACY_LOADER_BYTES}, *i .r4, p 0) i.r5'
  ${If} $5 != 0
  ${AndIf} $4 == ${ZHIYE_LEGACY_LOADER_BYTES}
    System::Alloc 32
    Pop $6
    ${If} $6 != 0
      System::Call 'crypt32::CryptHashCertificate2(w "SHA256", i 0, p 0, p r3, i r4, p r6, *i 32) i.r7'
      ${If} $7 != 0
        ; CRYPT_STRING_HEXRAW | CRYPT_STRING_NOCRLF.
        System::Call 'crypt32::CryptBinaryToStringW(p r6, i 32, i 0x4000000c, w .r8, *i 65) i.r7'
        ${If} $7 != 0
        ${AndIf} $8 == "${ZHIYE_LEGACY_LOADER_SHA256}"
          ; FileDispositionInfo takes a one-byte BOOLEAN.
          System::Call 'kernel32::SetFileInformationByHandle(p r0, i 4, *i 1, i 1) i.r7'
          ${If} $7 != 0
            DetailPrint "Removed the legacy WebView2 loader."
          ${EndIf}
        ${EndIf}
      ${EndIf}
      System::Free $6
    ${EndIf}
  ${EndIf}
  System::Free $3

  legacy_close:
  System::Call 'kernel32::CloseHandle(p r0)'
  legacy_done:
  System::Store "fL"
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
  ; Tauri renders its skip setting as an empty installer mode.
  !if "${INSTALLWEBVIEW2MODE}" != ""
    !error "The Zhiye WebView2 hook requires the Tauri WebView2 section to skip installation."
  !endif

  ; Read the old registration before Tauri writes the new one.
  StrCpy $ZhiyeOverlayInstall 0
  Push $0
  Push $1
  ReadRegStr $0 SHCTX "${UNINSTKEY}" "InstallLocation"
  ReadRegStr $1 SHCTX "${UNINSTKEY}" "MainBinaryName"
  ${If} $0 == '$\"$INSTDIR$\"'
  ${AndIf} $1 == "${MAINBINARYNAME}.exe"
  ${AndIf} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $ZhiyeOverlayInstall 1
  ${EndIf}
  Pop $1
  Pop $0

  Call ZhiyeReadWebView2Version
  ${If} $ZhiyeWebView2Version == ""
    InitPluginsDir
    !ifdef ZHIYE_OFFLINE_WEBVIEW2
      StrCpy $ZhiyeWebView2Installer "$PLUGINSDIR\MicrosoftEdgeWebView2RuntimeInstaller.exe"
      ClearErrors
      File "/oname=$PLUGINSDIR\MicrosoftEdgeWebView2RuntimeInstaller.exe" "${ZHIYE_OFFLINE_WEBVIEW2}"
      ${If} ${Errors}
        !insertmacro ZhiyeAbortWebView2 "Could not extract the bundled WebView2 Runtime installer."
      ${EndIf}
    !else
      StrCpy $ZhiyeWebView2Installer "$PLUGINSDIR\MicrosoftEdgeWebView2Setup.exe"
      DetailPrint "$(webview2Downloading)"
      ; NSISdl uses plain HTTP even when the URL has an HTTPS scheme.
      System::Call '$SYSDIR\urlmon.dll::URLDownloadToFileW(p 0, w "https://go.microsoft.com/fwlink/p/?LinkId=2124703", w "$ZhiyeWebView2Installer", i 0, p 0) i.s'
      Pop $ZhiyeWebView2Result
      ${If} $ZhiyeWebView2Result != "0"
        !insertmacro ZhiyeAbortWebView2 "WebView2 bootstrapper download failed ($ZhiyeWebView2Result)."
      ${EndIf}
      ${IfNot} ${FileExists} "$ZhiyeWebView2Installer"
        !insertmacro ZhiyeAbortWebView2 "The WebView2 download did not create the bootstrapper file."
      ${EndIf}
    !endif

    DetailPrint "$(installingWebView2)"
    ClearErrors
    ExecWait '"$ZhiyeWebView2Installer" /silent /install' $ZhiyeWebView2Result
    ${If} ${Errors}
      !insertmacro ZhiyeAbortWebView2 "Could not start the WebView2 Runtime installer."
    ${EndIf}
    ${If} $ZhiyeWebView2Result <> 0
      !insertmacro ZhiyeAbortWebView2 "The WebView2 Runtime installer exited with code $ZhiyeWebView2Result."
    ${EndIf}

    Call ZhiyeReadWebView2Version
    ${If} $ZhiyeWebView2Version == ""
      !insertmacro ZhiyeAbortWebView2 "The WebView2 Runtime version could not be confirmed after installation."
    ${EndIf}
    DetailPrint "$(webview2InstallSuccess)"
  ${EndIf}

  ; The application and its resources must be written before Tauri updates the
  ; installation registration. NSIS otherwise skips an unwritable File in silent mode.
  AllowSkipFiles off
!macroend

!macro NSIS_HOOK_POSTINSTALL
  AllowSkipFiles on
  ${If} $ZhiyeOverlayInstall == 1
    ; Keep the old loader if the new executable could not be written.
    System::Store "SF"
    GetDLLVersionLocal "${MAINBINARYSRCPATH}" $0 $1
    StrCpy $4 "${ZHIYE_FILE_RETRY_COUNT}"

    zhiye_read_installed_version:
    ClearErrors
    GetDLLVersion "$INSTDIR\${MAINBINARYNAME}.exe" $2 $3
    ${If} ${Errors}
      ; A newly copied executable can be briefly unavailable for version reads.
      ${If} $4 > 0
      ${AndIf} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
        IntOp $4 $4 - 1
        Sleep ${ZHIYE_FILE_RETRY_DELAY_MS}
        Goto zhiye_read_installed_version
      ${EndIf}
      DetailPrint "Could not confirm the installed application version."
    ${ElseIf} $0 == $2
    ${AndIf} $1 == $3
      Call ZhiyeRemoveLegacyWebView2Loader
    ${EndIf}
    System::Store "fL"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Apply Tauri's running-app check before removing the executable. Abort here on
  ; a deletion failure while the uninstaller and its registration are still usable.
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  System::Store "SF"
  StrCpy $2 "${ZHIYE_FILE_RETRY_COUNT}"

  zhiye_uninstall_application:
  ; Retain the normal NSIS Delete behavior for read-only files.
  ClearErrors
  Delete "$INSTDIR\${MAINBINARYNAME}.exe"
  ${IfNot} ${Errors}
    Goto zhiye_uninstall_application_done
  ${EndIf}
  ; Ask Windows for the error code so only a lock conflict is retried.
  System::Call 'kernel32::DeleteFileW(w "$INSTDIR\${MAINBINARYNAME}.exe") i.r0 ?e'
  Pop $1
  ${If} $0 == 0
  ${AndIf} $1 != 2 ; ERROR_FILE_NOT_FOUND
  ${AndIf} $1 != 3 ; ERROR_PATH_NOT_FOUND
    ${If} $1 == 32
    ${OrIf} $1 == 33
      ${If} $2 > 0
        IntOp $2 $2 - 1
        Sleep ${ZHIYE_FILE_RETRY_DELAY_MS}
        Goto zhiye_uninstall_application
      ${EndIf}
    ${EndIf}
    DetailPrint "Could not delete ${MAINBINARYNAME}.exe (Windows error $1)."
    System::Store "fL"
    SetErrorLevel 2
    Abort "$(ZhiyeProgramDeleteDenied)"
  ${EndIf}

  zhiye_uninstall_application_done:
  System::Store "fL"
!macroend
