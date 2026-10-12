@echo off
setlocal
title PhotoCraft
pushd "%~dp0"
if errorlevel 1 exit /b 1

rem Rust may have been installed since Explorer last refreshed its PATH.
where cargo.exe >nul 2>nul
if not errorlevel 1 goto run
if defined CARGO_HOME if exist "%CARGO_HOME%\bin\cargo.exe" set "PATH=%CARGO_HOME%\bin;%PATH%"
where cargo.exe >nul 2>nul
if not errorlevel 1 goto run
if exist "%USERPROFILE%\.cargo\bin\cargo.exe" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo.exe >nul 2>nul
if errorlevel 1 goto missing_rust

:run
echo Building and opening PhotoCraft. The first release build may take several minutes.
cargo.exe run --release -p photocraft -- %*
set "PHOTOCRAFT_EXIT=%ERRORLEVEL%"
popd
if "%PHOTOCRAFT_EXIT%"=="0" exit /b 0
echo.
echo PhotoCraft could not build or run. See the error above.
pause
exit /b %PHOTOCRAFT_EXIT%

:missing_rust
echo Rust was not found. Install Rust and Microsoft C++ Build Tools as described in docs\development.md.
popd
pause
exit /b 1
