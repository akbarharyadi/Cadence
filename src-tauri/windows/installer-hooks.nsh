; Cadence installer hooks (see bundle.windows.nsis.installerHooks in tauri.conf.json).

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
