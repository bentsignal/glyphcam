@echo off
rem Build realsense2.dll (Intel librealsense v2.48.0) into third_party\realsense\.
rem
rem 2.48 on purpose: the SR300/SR305 were dropped in 2.55, and 2.51-2.54
rem crash on newer Windows builds. Built with the static CRT, so the DLL
rem needs no Visual C++ redistributable. Skips the build when the DLL exists.
rem
rem Needs Visual Studio 2022 (or Build Tools) with the C++ workload.
setlocal
set VER=2.48.0
set ROOT=%~dp0..
set OUT=%ROOT%\third_party\realsense
if exist "%OUT%\realsense2.dll" (
  echo realsense2.dll already built: %OUT%\realsense2.dll
  exit /b 0
)
for /f "usebackq tokens=*" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set VS=%%i
if not defined VS (
  echo Visual Studio with C++ tools not found
  exit /b 1
)
call "%VS%\VC\Auxiliary\Build\vcvars64.bat" >nul || exit /b 1
set CMAKE=cmake
set NINJA=ninja
if exist "%VS%\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe" set CMAKE="%VS%\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe"
if exist "%VS%\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe" set NINJA="%VS%\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe"

set WORK=%TEMP%\glyphcam-librealsense-%VER%
if not exist "%WORK%" mkdir "%WORK%"
pushd "%WORK%"
if not exist "librealsense-%VER%" (
  curl -sSfL -o src.tar.gz https://github.com/IntelRealSense/librealsense/archive/refs/tags/v%VER%.tar.gz || exit /b 1
  tar xzf src.tar.gz || exit /b 1
  del src.tar.gz
)
rem The /FI force-includes cover headers the 2021-era sources forgot and that
rem current MSVC no longer pulls in implicitly.
%CMAKE% -S librealsense-%VER% -B build -G Ninja -DCMAKE_MAKE_PROGRAM=%NINJA% ^
  -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=ON -DBUILD_WITH_STATIC_CRT=ON ^
  -DBUILD_EXAMPLES=OFF -DBUILD_GRAPHICAL_EXAMPLES=OFF -DBUILD_TOOLS=OFF -DBUILD_UNIT_TESTS=OFF ^
  -DBUILD_WITH_TM2=OFF -DIMPORT_DEPTH_CAM_FW=OFF -DBUILD_EASYLOGGINGPP=OFF ^
  -DCMAKE_POLICY_VERSION_MINIMUM=3.5 ^
  "-DCMAKE_CXX_FLAGS=/DWIN32 /D_WINDOWS /EHsc /FIchrono /FIfunctional /FIalgorithm /FIstring /FIsstream /FIcstdint /FIlimits /FIarray" || exit /b 1
%CMAKE% --build build --target realsense2 || exit /b 1
if not exist "%OUT%" mkdir "%OUT%"
copy /y build\realsense2.dll "%OUT%\" >nul || exit /b 1
copy /y librealsense-%VER%\LICENSE "%OUT%\LICENSE-librealsense.txt" >nul
copy /y librealsense-%VER%\NOTICE "%OUT%\NOTICE-librealsense.txt" >nul
popd
rmdir /s /q "%WORK%"
echo built %OUT%\realsense2.dll
