@echo off
setlocal EnableExtensions
cd /d "%~dp0"
where adb >nul 2>&1
if errorlevel 1 (
  set "ADB=%LOCALAPPDATA%\Android\Sdk\platform-tools\adb.exe"
) else (
  set "ADB=adb"
)
if not exist "%LOCALAPPDATA%\Android\Sdk\platform-tools\adb.exe" (
  where adb >nul 2>&1 || (echo [ERRO] adb nao encontrado. Instale Android SDK Platform-Tools. & pause & exit /b 1)
)
set "LOGFILE=%~dp0photocraft_spen_log.txt"
echo Conecte o tablet via USB com depuracao USB ativada.
"%ADB%" devices
echo.
echo [INFO] Limpando logcat. Abra o PhotoCraft e toque nos botoes com a S Pen.
echo [INFO] Gravando log completo em "%LOGFILE%"
echo [INFO] Pressione Ctrl+C para parar. O arquivo permanece salvo.
"%ADB%" logcat -c
"%ADB%" logcat -v time > "%LOGFILE%"
pause
