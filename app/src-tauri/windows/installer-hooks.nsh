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
