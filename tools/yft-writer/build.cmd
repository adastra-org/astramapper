@echo off
rem Builds YftWriter.exe with the C# compiler that ships with .NET Framework.
rem CodeWalker.Core must be built first (CodeWalkerAstra, Release, net48).
setlocal
set CW=%CODEWALKER_DIR%
if "%CW%"=="" set CW=E:\Proyectos\Astra\CodeWalkerAstra\CodeWalker\bin\Release\net48
set FW=%WINDIR%\Microsoft.NET\Framework64\v4.0.30319
if not exist bin mkdir bin
"%FW%\csc.exe" /nologo /target:exe /platform:x64 /out:bin\YftWriter.exe ^
  /reference:"%CW%\CodeWalker.Core.dll" /reference:"%CW%\SharpDX.dll" /reference:"%CW%\SharpDX.Mathematics.dll" ^
  /reference:"%FW%\netstandard.dll" YftWriter.cs
