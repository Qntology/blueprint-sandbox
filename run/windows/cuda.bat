@echo off
rem Builds and runs blueprint-core on NVIDIA CUDA. Needs the CUDA Toolkit and MSVC (scripts\cuda-env.bat).
rem     cuda.bat [example] [example arguments]
call "%~dp0..\..\scripts\run-windows.bat" cuda %*
