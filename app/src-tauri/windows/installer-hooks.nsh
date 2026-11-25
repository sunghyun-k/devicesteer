; DeviceSteer's additions to Tauri's NSIS installer and uninstaller (bundle.windows.nsis.installerHooks
; in tauri.conf.json). Tauri inserts these macros at the start of its Install and Uninstall sections.

; devicesteer.exe, the CLI, also runs the per-device daemons, which outlive the app. Tauri closes
; only the app's own exe before it copies or deletes files, and Windows can't overwrite or delete an
; exe that is running: an update (which overwrites the files in place) would stop at the CLI with
; "Error opening file for writing" while a daemon runs, and an uninstall would leave it behind.
; So stop whatever runs this file first. Restart Manager finds processes by the file, not by its
; path, which should also catch a daemon started through the hard link Settings › Agents makes in
; ~\.local\bin. No prompt, unlike the app: the next CLI command starts its daemon again.
!macro DEVICESTEER_STOP_CLI
  !insertmacro RestartManager_ShutdownFile "$INSTDIR\devicesteer.exe" $0
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro DEVICESTEER_STOP_CLI
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro DEVICESTEER_STOP_CLI
!macroend

; Builds up to 1.0.0-beta.2 kept their state in %LOCALAPPDATA%\devicesteer, which is also the
; default install folder; later ones keep it in %LOCALAPPDATA%\<identifier> and move it there
; (crates/core/src/paths.rs), and Tauri's "Delete the application data" box deletes that folder.
; Honour the box for state a later build hasn't moved yet (it never ran), then remove the old
; folder if that leaves it empty. Not on an update, which runs this uninstaller too.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RMDir /r "$LOCALAPPDATA\devicesteer\remote-pairing"
    RMDir /r "$LOCALAPPDATA\devicesteer\ddi"
    Delete "$LOCALAPPDATA\devicesteer\preferences.json"
    Delete "$LOCALAPPDATA\devicesteer\app_path"
    Delete "$LOCALAPPDATA\devicesteer\*.media"
    Delete "$LOCALAPPDATA\devicesteer\*.refs.json"
    Delete "$LOCALAPPDATA\devicesteer\*.record.*"
    Delete "$LOCALAPPDATA\devicesteer\*.log"
    RMDir "$LOCALAPPDATA\devicesteer"
  ${EndIf}
!macroend
