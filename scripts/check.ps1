# Copyright (C) 2026 whispr.dev
# SPDX-License-Identifier: GPL-3.0-only
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
function Invoke-Cargo {
    param([Parameter(Mandatory)][string[]]$CargoArguments)
    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($CargoArguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}
Push-Location (Join-Path $PSScriptRoot '..')
try {
    Invoke-Cargo -CargoArguments @('fmt', '--all', '--check')
    Invoke-Cargo -CargoArguments @('test', '--locked', '--all-targets')
    Invoke-Cargo -CargoArguments @('test', '--locked', '--doc')
    Invoke-Cargo -CargoArguments @('clippy', '--locked', '--all-targets', '--', '-D', 'warnings')
    Invoke-Cargo -CargoArguments @('build', '--locked', '--release')
    Invoke-Cargo -CargoArguments @('run', '--locked', '--release', '--example', 'standalone_demo', '--', './output/demo')
    Write-Host 'SkeleTrace validation passed.'
}
finally {
    Pop-Location
}
