@echo off
chcp 65001 >nul
setlocal
cd /d "%~dp0"

rem 优先用工程旁边的便携 SDK，其次用系统 PATH 里的 dotnet
set "DOTNET="
if exist "%~dp0..\dotnet8sdk\dotnet.exe" set "DOTNET=%~dp0..\dotnet8sdk\dotnet.exe"
if not defined DOTNET (
  where dotnet >nul 2>nul
  if not errorlevel 1 set "DOTNET=dotnet"
)
if not defined DOTNET goto nodotnet

echo ============================================
echo    TT酱 (TTJiang) - net48 单文件发布
echo ============================================
echo 使用: %DOTNET%
echo.

"%DOTNET%" publish "%~dp0src\TTJiang.csproj" -c Release -p:DebugType=None -p:DebugSymbols=false -o "%~dp0dist"

if errorlevel 1 goto failed

echo.
echo [完成] 输出目录：%~dp0dist
echo 主程序约 2.7 MB（内置搜索引擎），Win10/Win11 自带 .NET Framework 4.8，目标机不需要装任何运行时。
echo.
pause
exit /b 0

:nodotnet
echo [错误] 没找到可用的 dotnet。
echo.
echo 两种解决方式（任选其一）：
echo   1. 安装 SDK：winget install Microsoft.DotNet.SDK.8
echo   2. 把便携 SDK 解压到 %~dp0..\dotnet8sdk
echo.
echo 安装地址：https://dotnet.microsoft.com/download/dotnet/8.0
pause
exit /b 1

:failed
echo.
echo [失败] 编译没通过。把上面的报错内容贴出来即可。
echo.
pause
exit /b 1
