@echo off
rem Deploy the neural-matting runtime: ONNX Runtime (DirectML) DLLs next to the
rem executables, and the RMBG-2.0 model into PhotoCraftData\models\rmbg-2.0.
rem
rem ONNX Runtime ships as the NuGet package Microsoft.ML.OnnxRuntime.DirectML;
rem the loader (ort, load-dynamic) picks the DLLs up from the executable's
rem directory at run time — nothing is linked at build time. The model weights
rem are NOT distributed here: RMBG-2.0 is BRIA's CC-BY-NC-4.0 research model,
rem so download it yourself from https://huggingface.co/briaai/RMBG-2.0
rem (or the community mirror https://github.com/danielgatis/rembg/releases —
rem file bria-rmbg-2.0.onnx, rename to RMBG-2.0.onnx)Into PhotoCraftData\models\rmbg-2.0.
setlocal
cd /d "%~dp0.."

set "ORT_SRC=%~1"
if "%ORT_SRC%"=="" (
  echo Usage: deploy_matting.bat ^<path to unpacked Microsoft.ML.OnnxRuntime.DirectML package or its runtimes\win-x64\native folder^>
  echo.
  echo Unpack the .nupkg with:  tar -xf Microsoft.ML.OnnxRuntime.DirectML.x.y.z.nupkg -C onnxruntime-dml
  echo then pass: onnxruntime-dml\runtimes\win-x64\native
  exit /b 2
)

set "SRC=%ORT_SRC%"
if exist "%SRC%\runtimes\win-x64\native\onnxruntime.dll" set "SRC=%SRC%\runtimes\win-x64\native"

for %%f in (onnxruntime.dll DirectML.dll onnxruntime_providers_shared.dll) do (
  if not exist "%SRC%\%%f" (
    echo ERROR: %%f not found under "%SRC%"
    exit /b 1
  )
  copy /y "%SRC%\%%f" ".\%%f" >nul
  echo installed %%f
)

if not exist "PhotoCraftData\models\rmbg-2.0" mkdir "PhotoCraftData\models\rmbg-2.0"
if not exist "PhotoCraftData\models\rmbg-2.0\RMBG-2.0.onnx" (
  echo.
  echo NOTE: place RMBG-2.0.onnx into PhotoCraftData\models\rmbg-2.0  ^(download link above^).
) else (
  echo model present: PhotoCraftData\models\rmbg-2.0\RMBG-2.0.onnx
)
echo done.
endlocal
