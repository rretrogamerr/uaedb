@echo off
setlocal

cd /d "%~dp0"

set "DATA=data.unity3d"
set "PATCH=data.xdelta"
set "OUT=data.unity3d.patched"
set "BACKUP=data.unity3d.bak"

if not exist "uaedb.exe" (
  echo uaedb.exe not found in %cd%
  exit /b 1
)

if not exist "%DATA%" (
  echo %DATA% not found in %cd%
  exit /b 1
)

if not exist "%PATCH%" (
  echo %PATCH% not found in %cd%
  exit /b 1
)

if exist "%BACKUP%" (
  echo Backup already exists: %BACKUP%
  echo Delete or rename it to proceed.
  exit /b 1
)

echo Backing up %DATA% to %BACKUP%...
copy /b "%DATA%" "%BACKUP%" >nul
if errorlevel 1 (
  echo Backup failed.
  exit /b 1
)

echo Patching...
uaedb.exe "%DATA%" "%PATCH%" "%OUT%"
if errorlevel 1 goto restore

echo Replacing original bundle...
move /y "%OUT%" "%DATA%" >nul
if errorlevel 1 goto restore

echo Patch completed.
echo Backup kept at %BACKUP%.
exit /b 0

:restore
echo Patch failed. Restoring backup...
if exist "%BACKUP%" (
  copy /b "%BACKUP%" "%DATA%" >nul
)
if exist "%OUT%" (
  del /f /q "%OUT%"
)
exit /b 1
