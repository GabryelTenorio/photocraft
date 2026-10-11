@echo off
setlocal EnableExtensions
cd /d "%~dp0"
if not exist "Cargo.toml" (
  echo [ERRO] Coloque este BAT na raiz do repositorio PhotoCraft, junto ao Cargo.toml.
  pause
  exit /b 1
)
set "ANDROID_HOME=%LOCALAPPDATA%\Android\Sdk"
set "ANDROID_SDK_ROOT=%ANDROID_HOME%"
set "ANDROID_NDK_HOME=%ANDROID_HOME%\ndk\27.0.12077973"
set "NDK_BIN=%ANDROID_NDK_HOME%\toolchains\llvm\prebuilt\windows-x86_64\bin"
set "CC_aarch64_linux_android=%NDK_BIN%\aarch64-linux-android26-clang.cmd"
set "CXX_aarch64_linux_android=%NDK_BIN%\aarch64-linux-android26-clang++.cmd"
set "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=%CC_aarch64_linux_android%"
set "AR_aarch64_linux_android=%NDK_BIN%\llvm-ar.exe"
if not exist "%CC_aarch64_linux_android%" goto :ndk_error
if not exist "%CXX_aarch64_linux_android%" goto :ndk_error
if not exist "%AR_aarch64_linux_android%" goto :ndk_error
where cargo >nul 2>&1 || (echo [ERRO] Cargo nao encontrado no PATH. & goto :fail)
where rustup >nul 2>&1 || (echo [ERRO] Rustup nao encontrado no PATH. & goto :fail)
echo [1/4] Instalando target ARM64 Android...
rustup target add aarch64-linux-android || goto :fail
echo [2/4] Compilando biblioteca Rust ARM64...
cargo build --release --target aarch64-linux-android -p photocraft-android -j 4 || goto :fail
set "SRC=target\aarch64-linux-android\release\libphotocraft_android.so"
set "DST=apps\photocraft-android\android\src\main\jniLibs\arm64-v8a"
if not exist "%SRC%" (echo [ERRO] Biblioteca nao encontrada: %SRC% & goto :fail)
echo [3/4] Copiando biblioteca para projeto Android...
if not exist "%DST%" mkdir "%DST%" || goto :fail
copy /Y "%SRC%" "%DST%\libphotocraft_android.so" >nul || goto :fail
echo [4/4] Compilando APK Debug...
pushd "apps\photocraft-android\android" || goto :fail
if exist "gradlew.bat" (
  call gradlew.bat :app:assembleDebug
) else (
  where gradle >nul 2>&1
  if errorlevel 1 (
    echo [ERRO] gradlew.bat e Gradle nao encontrados.
    popd
    goto :fail
  )
  call gradle --no-daemon :app:assembleDebug
)
set "GRADLE_EXIT=%ERRORLEVEL%"
popd
if not "%GRADLE_EXIT%"=="0" goto :fail
echo.
echo [OK] APK criado em:
echo %CD%\apps\photocraft-android\android\app\build\outputs\apk\debug\app-debug.apk
pause
exit /b 0
:ndk_error
echo [ERRO] NDK 27.0.12077973 incompleto ou compiladores ausentes:
echo %NDK_BIN%
:fail
echo.
echo [FALHA] Build interrompido. Veja o primeiro erro acima.
pause
exit /b 1
