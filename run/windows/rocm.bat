@echo off
rem Builds and runs blueprint-core on AMD ROCm/HIP. Needs the HIP SDK and MSVC 14.44 (scripts\rocm-env.bat).
rem     rocm.bat [example] [example arguments]
call "%~dp0..\..\scripts\run-windows.bat" rocm %*
