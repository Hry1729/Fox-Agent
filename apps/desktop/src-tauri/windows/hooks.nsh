!define FOX_SANDBOX_FILES "${__FILEDIR__}\..\resources\sandbox-installer"
!include "${FOX_SANDBOX_FILES}\package.nsh"

!macro NSIS_HOOK_PREINSTALL
  DetailPrint "Installing Fox sandbox runtime ${FOX_SANDBOX_VERSION}..."
  InitPluginsDir
  SetOutPath "$PLUGINSDIR\fox-sandbox"
  File /oname=runtime.zip "${FOX_SANDBOX_FILES}\runtime.zip"
  File /oname=enable-sandbox.ps1 "${FOX_SANDBOX_FILES}\enable-sandbox.ps1"
  File /oname=install-sandbox.ps1 "${FOX_SANDBOX_FILES}\install-sandbox.ps1"
  ClearErrors
  ExecWait '"$WINDIR\SysNative\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File "$PLUGINSDIR\fox-sandbox\install-sandbox.ps1" -PackagePath "$PLUGINSDIR\fox-sandbox\runtime.zip" -ExpectedSha256 "${FOX_SANDBOX_SHA256}" -Version "${FOX_SANDBOX_VERSION}"' $0
  ${If} ${Errors}
    StrCpy $0 1
  ${EndIf}
  ${If} $0 != 0
    SetErrorLevel 1
    MessageBox MB_OK|MB_ICONSTOP "Fox 沙箱安装失败，安装已停止。$\r$\n请查看 C:\ProgramData\Fox\logs 中的 sandbox-installer 日志，然后重新运行安装包。$\r$\n$\r$\nFox sandbox setup failed. See %ProgramData%\Fox\logs." /SD IDOK
    Abort
  ${EndIf}
  SetOutPath "$INSTDIR"
  DetailPrint "Fox sandbox runtime installed."
!macroend
