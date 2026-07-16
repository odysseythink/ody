@echo off
rem Clean only this workspace's own crates, keeping third-party dependency
rem artifacts (e.g. the compiled `v8` crate) intact in target\.
rem
rem Use this instead of `cargo clean` to avoid recompiling v8 after every clean.
setlocal

cd /d "%~dp0.."

powershell -NoProfile -ExecutionPolicy Bypass -Command "$pkgs = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages.name | Sort-Object -Unique; if (-not $pkgs) { Write-Error 'no workspace packages found (is cargo metadata working?)'; exit 1 }; Write-Host ('Cleaning ' + $pkgs.Count + ' workspace packages (dependencies like v8 are kept)...'); $cargoArgs = $pkgs | ForEach-Object { '-p'; $_ }; & cargo clean $cargoArgs; exit $LASTEXITCODE"

exit /b %ERRORLEVEL%
