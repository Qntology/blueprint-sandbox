@echo off
setlocal
rem Runs the blueprint app on NVIDIA CUDA. Needs the CUDA Toolkit and MSVC (scripts\cuda-env.bat).
rem Set BLUEPRINT_ENGINE=rules to run without models. Scanner images are read from
rem BLUEPRINT_SCANNER_DIR (default %%LOCALAPPDATA%%\blueprint\scanner).
cd /d "%~dp0..\.."
call scripts\cuda-env.bat
if errorlevel 1 goto :failed
set "BLUEPRINT_DEVICE=cuda"
set "CARGO_TARGET_DIR=%CD%\target-cuda"
cd /d app
if not exist "node_modules\.bin\tauri.cmd" (
    call npm install
    if errorlevel 1 goto :failed
)
call npx tauri dev --release --no-watch --features cuda %* -- --no-default-features
if errorlevel 1 goto :failed
endlocal & exit /b 0
:failed
pause
endlocal & exit /b 1
