@echo off
rem Builds a small, fast anvil.exe for sharing and puts it in a "dist" folder
rem next to the build output, plus a zip of it.
rem   - optimized, link-time optimization, no debug info, symbols stripped
rem   - no console window (set in main.rs)
setlocal
cd /d "%~dp0"

echo Building Anvil (this takes a few minutes the first time)...
cargo build --profile dist
if errorlevel 1 (
    echo.
    echo Build failed.
    exit /b 1
)

rem Where cargo puts things (see .cargo\config.toml)
for /f "usebackq delims=" %%i in (`powershell -NoProfile -Command "(cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory"`) do set "TARGET=%%i"
for /f "usebackq delims=" %%v in (`powershell -NoProfile -Command "((cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).packages | Where-Object name -eq 'anvil').version"`) do set "VERSION=%%v"
if "%TARGET%"=="" set "TARGET=%~dp0target"
for %%d in ("%TARGET%\..") do set "OUT=%%~fd\dist"

if exist "%OUT%\Anvil" rmdir /s /q "%OUT%\Anvil"
mkdir "%OUT%\Anvil\plugins"
copy /y "%TARGET%\dist\anvil.exe" "%OUT%\Anvil\" >nul
copy /y "plugins\*.rhai" "%OUT%\Anvil\plugins\" >nul
copy /y "README.md" "%OUT%\Anvil\" >nul

set "ZIP=%OUT%\Anvil-%VERSION%.zip"
if exist "%ZIP%" del /q "%ZIP%"
powershell -NoProfile -Command "Compress-Archive -Path '%OUT%\Anvil' -DestinationPath '%ZIP%'"
powershell -NoProfile -Command "(Get-FileHash '%ZIP%' -Algorithm SHA256).Hash" > "%ZIP%.sha256"

echo.
echo Done:
echo   %OUT%\Anvil\anvil.exe
echo   %ZIP%
endlocal
