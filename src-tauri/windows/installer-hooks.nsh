; Cadence installer hooks (see bundle.windows.nsis.installerHooks in tauri.conf.json).

!define /ifndef ERROR_MORE_DATA 234

; In-app updates: Cadence starts this installer and exits. Tauri's installer then checks once,
; without waiting, whether Cadence is still running, and if it's still on its way out the update
; aborts with "Failed to kill Cadence" (tauri-apps/tauri#16277). Wait for it to be gone first
; (up to 30 s); the template's check runs right after this.
!macro NSIS_HOOK_PREINSTALL
  ${If} $UpdateMode = 1
    Push $0
    Push $1
    Push $2
    Push $3
    Push $R4
    Push $R5
    StrCpy $R5 0
    ${Do}
      !insertmacro RestartManager_StartSession $R4
      ${If} $R4 == ""
        ${Break}
      ${EndIf}
      !insertmacro RestartManager_RegisterFile $R4 "$INSTDIR\${MAINBINARYNAME}.exe"
      ${If} $0 = 0
        ; ERROR_MORE_DATA: something still uses the exe.
        System::Call 'RSTRTMGR::RmGetList(i R4, *i .r1, *i .r2, p 0, *i .r3) i .r0'
      ${EndIf}
      !insertmacro RestartManager_EndSession $R4
      ${If} $0 <> ${ERROR_MORE_DATA}
      ${OrIf} $R5 >= 120
        ${Break}
      ${EndIf}
      ${If} $R5 = 0
        DetailPrint "Waiting for ${PRODUCTNAME} to close..."
      ${EndIf}
      IntOp $R5 $R5 + 1
      Sleep 250
    ${Loop}
    Pop $R5
    Pop $R4
    Pop $3
    Pop $2
    Pop $1
    Pop $0
  ${EndIf}
!macroend

; Tauri's installer adds the Start menu shortcut only on a fresh install and skips it when
; updating. Put it back whenever it's missing, so Cadence can always be found in Start search.
!macro NSIS_HOOK_POSTINSTALL
  ${If} $NoShortcutMode <> 1
    ${IfNot} ${FileExists} "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
      !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}
!macroend
