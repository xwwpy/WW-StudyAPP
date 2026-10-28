# Use the redistributable CRT from the installed compiler toolchain, never System32.
function Get-WindowsRuntime {
    $locator = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $installation = & $locator -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $installation) { throw 'Visual C++ build tools are required for packaging' }
    $tools = Get-ChildItem -LiteralPath (Join-Path $installation 'VC/Tools/MSVC') -Directory |
        Where-Object { $_.Name -match '^\d+\.\d+\.\d+$' } | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
    $redist = Get-ChildItem -LiteralPath (Join-Path $installation 'VC/Redist/MSVC') -Directory |
        Where-Object { $_.Name -match '^\d+\.\d+\.\d+$' } | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
    if (-not $tools -or -not $redist) { throw 'Missing Visual C++ toolchain or redistributable' }
    $crt = @(Get-ChildItem -LiteralPath (Join-Path $redist.FullName 'x64') -Directory | Where-Object Name -Match '^Microsoft\.VC\d+\.CRT$')
    if ($crt.Count -ne 1) { throw 'Cannot uniquely locate the x64 release CRT' }
    [pscustomobject]@{
        Dumpbin = Join-Path $tools.FullName 'bin/Hostx64/x64/dumpbin.exe'
        Directory = $crt[0].FullName
        Files = @('vcruntime140.dll', 'vcruntime140_1.dll')
    }
}

function Assert-WindowsRuntimeImports([string]$Binary, $Runtime) {
    $output = & $Runtime.Dumpbin /dependents $Binary
    if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect Windows binary dependencies' }
    $imports = @($output | ForEach-Object { if ($_ -match '^\s+([\w.-]+\.dll)\s*$') { $Matches[1].ToLowerInvariant() } })
    if ($imports.Count -eq 0) { throw 'No DLL imports found in Windows binary' }
    foreach ($name in $imports) {
        if ($name -match '^(vcruntime|msvcp|concrt)' -and $name -notin $Runtime.Files) {
            throw "Unpackaged C++ runtime dependency: $name"
        }
    }
    return $imports
}
