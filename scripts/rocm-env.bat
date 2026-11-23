@echo off
rem ASCII-only on purpose: cmd.exe mis-parses non-ASCII bytes in .bat files on CP949 consoles.
rem
rem Sets up the AMD ROCm (HIP) build environment for blueprint-core. Use it before cargo:
rem     call scripts\rocm-env.bat
rem     cargo build -p blueprint-core --no-default-features --features rocm
rem
rem 1) MSVC toolset 14.44. The default 14.51 STL conflicts with the clang 21 HIP headers
rem    ("__device__ function 'isgreater' cannot overload ...") and the kernel build fails.
rem    14.44 is installed with the Visual Studio component "MSVC v143 ... (14.44)".
rem 2) HIP SDK: defaults to 7.2 (legacy track). Set HIP_PATH before the call to use another one.
rem 3) GPU target: defaults to gfx1032 (Radeon RX 6600). Set HIP_ARCH before the call to change it.

set "_USER_VCVARS_VER=1"
if not defined VCVARS_VER (
    set "_USER_VCVARS_VER="
    set "VCVARS_VER=14.44"
)
set "VCVARS="
for %%P in (
    "%ProgramFiles%\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
    "%ProgramFiles%\Microsoft Visual Studio\2022\Professional\VC\Auxiliary\Build\vcvars64.bat"
    "%ProgramFiles%\Microsoft Visual Studio\2022\Enterprise\VC\Auxiliary\Build\vcvars64.bat"
    "%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
    "%ProgramFiles%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
) do (
    if not defined VCVARS if exist %%~P set "VCVARS=%%~P"
)
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not defined VCVARS if exist "%VSWHERE%" for /f "usebackq delims=" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do (
    if not defined VCVARS if exist "%%i\VC\Auxiliary\Build\vcvars64.bat" set "VCVARS=%%i\VC\Auxiliary\Build\vcvars64.bat"
)
if not defined VCVARS goto :no_vcvars
if not exist "%VCVARS%" goto :no_vcvars

call "%VCVARS%" -vcvars_ver=%VCVARS_VER% > nul 2>&1
if errorlevel 1 (
    if defined _USER_VCVARS_VER goto :vcvars_failed
    echo [ROCm] MSVC toolset %VCVARS_VER% not found; retrying with default VS toolset...
    call "%VCVARS%" > nul 2>&1
    if errorlevel 1 goto :vcvars_failed
)
set "_USER_VCVARS_VER="
echo [ROCm] MSVC toolset %VCToolsVersion%

if not defined HIP_PATH if defined ROCM_PATH set "HIP_PATH=%ROCM_PATH%"
if not defined HIP_PATH if exist "%ProgramFiles%\AMD\ROCm\7.2" set "HIP_PATH=%ProgramFiles%\AMD\ROCm\7.2"
if not defined HIP_PATH if exist "%ProgramFiles%\AMD\ROCm" (
    for /f "delims=" %%d in ('dir /b /ad /o-n "%ProgramFiles%\AMD\ROCm" 2^>nul') do (
        if not defined HIP_PATH set "HIP_PATH=%ProgramFiles%\AMD\ROCm\%%d"
    )
)
if not defined HIP_PATH set "HIP_PATH=%ProgramFiles%\AMD\ROCm\7.2"
if "%HIP_PATH:~-1%"=="\" set "HIP_PATH=%HIP_PATH:~0,-1%"
if not exist "%HIP_PATH%\bin\hipcc.exe" if not exist "%HIP_PATH%\lib\llvm\bin\clang++.exe" if not exist "%HIP_PATH%\bin\clang++.exe" goto :no_hipcc

set "ROCM_PATH=%HIP_PATH%"
set "PATH=%HIP_PATH%\bin;%PATH%"
if exist "%HIP_PATH%\lib\llvm\bin" set "PATH=%PATH%;%HIP_PATH%\lib\llvm\bin"
if not defined CANDLE_ROCM_TRACK set "CANDLE_ROCM_TRACK=legacy"
if not defined HIP_ARCH set "HIP_ARCH=gfx1032"
echo [ROCm] HIP SDK %HIP_PATH% ^| track %CANDLE_ROCM_TRACK% ^| GPU target %HIP_ARCH%
exit /b 0

:no_vcvars
set "_USER_VCVARS_VER="
echo [ROCm] vcvars64.bat not found. Install Visual Studio C++ Build Tools with the 14.44 toolset.
exit /b 1

:vcvars_failed
set "_USER_VCVARS_VER="
echo [ROCm] vcvars64.bat failed to initialize MSVC environment (VCVARS_VER=%VCVARS_VER%).
exit /b 1

:no_hipcc
echo [ROCm] hipcc.exe not found under %HIP_PATH%. Install the AMD HIP SDK or set HIP_PATH.
exit /b 1
