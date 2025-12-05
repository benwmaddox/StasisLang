#!/usr/bin/env bash
# stasis run <file> [--with-tests] [--module name]
dotnet run -p Stasis.Cli -- "$@"
