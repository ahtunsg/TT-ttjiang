@echo off
chcp 65001 >nul
setlocal
cd /d "%~dp0"

rem 只用开发者本机 PATH 里的 dotnet；SDK 仅是编译工具，不会进入发布包
set "DOTNET="
where dotnet >nul 2>nul
if not errorlevel 1 set "DOTNET=dotnet"
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
echo 主程序约 2.8 MB（内置搜索引擎）。目标机 Win10 1903+ / Win11 64 位自带 .NET Framework 4.8，不需要装 .NET 8 或任何额外运行时/SDK。
echo.
pause
exit /b 0

:nodotnet
echo [错误] 没找到可用的 dotnet。
echo.
echo 仅开发机需要安装 .NET SDK（6.0 或更高），目标机不需要任何 SDK 或运行时。
echo 安装地址：https://dotnet.microsoft.com/download/dotnet
pause
exit /b 1

:failed
echo.
echo [失败] 编译没通过。把上面的报错内容贴出来即可。
echo.
pause
exit /b 1
