@echo off
REM stasis run <file> [--with-tests] [--module name]
dotnet run -p Stasis.Cli -- %*
