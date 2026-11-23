@echo off
setlocal
rem Runs the blueprint app (Vulkan). Missing models download on start, with progress in the settings panel.
cd /d "%~dp0..\..\app"
if not exist "node_modules\.bin\tauri.cmd" (
    call npm install
    if errorlevel 1 goto :failed
)
call npx tauri dev %*
if errorlevel 1 goto :failed
endlocal & exit /b 0
:failed
pause
endlocal & exit /b 1
