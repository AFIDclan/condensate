; Installer hooks: put the bundled `condensate` CLI on the user's PATH, and take it off again
; on uninstall. The CLI does the registry work itself (`condensate setup ...`).

!macro NSIS_HOOK_POSTINSTALL
  nsExec::Exec '"$INSTDIR\condensate.exe" setup add-to-path'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec '"$INSTDIR\condensate.exe" setup remove-from-path'
!macroend
