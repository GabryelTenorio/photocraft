param([string]$OutputDirectory, [switch]$Unsigned)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repo 'target/android-dist' }
foreach ($name in @('ANDROID_HOME', 'ANDROID_NDK_HOME', 'JAVA_HOME')) {
    if (-not [Environment]::GetEnvironmentVariable($name)) { throw "Set $name before building." }
}
$signing = @('PHOTOCRAFT_KEYSTORE', 'PHOTOCRAFT_STORE_PASSWORD', 'PHOTOCRAFT_KEY_ALIAS', 'PHOTOCRAFT_KEY_PASSWORD')
if (-not $Unsigned) {
    foreach ($name in $signing) {
        if (-not [Environment]::GetEnvironmentVariable($name)) { throw "Set $name to reuse your signing identity, or explicitly pass -Unsigned." }
    }
} else {
    foreach ($name in $signing) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
}
Push-Location $repo
try {
    & cargo ndk -t arm64-v8a -o apps/photocraft-android/android/app/src/main/jniLibs build -p photocraft-android --release
    if ($LASTEXITCODE -ne 0) { throw 'Android native build failed.' }
    Push-Location apps/photocraft-android/android
    try {
        & ./gradlew.bat --no-daemon assembleRelease
        if ($LASTEXITCODE -ne 0) { throw 'Android packaging failed.' }
    } finally { Pop-Location }
    $gradle = Get-Content apps/photocraft-android/android/app/build.gradle -Raw
    $version = [regex]::Match($gradle, 'versionName "([A-Za-z0-9.-]+)"').Groups[1].Value
    if (-not $version) { throw 'Missing Android versionName.' }
    $apkName = if ($Unsigned) { 'app-release-unsigned.apk' } else { 'app-release.apk' }
    $source = Join-Path $repo "apps/photocraft-android/android/app/build/outputs/apk/release/$apkName"
    New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
    $suffix = if ($Unsigned) { '-unsigned' } else { '' }
    $destination = Join-Path $OutputDirectory "photocraft-$version$suffix.apk"
    Copy-Item -LiteralPath $source -Destination $destination
    $hash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($destination))" | Set-Content (Join-Path $OutputDirectory 'SHA256SUMS.txt') -Encoding utf8
    Write-Output "APK: $destination"
    Write-Output "SHA256: $hash"
} finally { Pop-Location }
