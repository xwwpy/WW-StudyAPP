#requires -Version 7.0
# Exact modern portable payload; user data and mutable receipts are never shipped.
function Get-ModernPackageResources {
    [ordered]@{
        'WW-StudyApp.exe' = 'application'
        'Uninstall.exe' = 'uninstaller'
        'vcruntime140.dll' = 'runtime'
        'vcruntime140_1.dll' = 'runtime'
        'support/LICENSE' = 'notice'
        'support/THIRD-PARTY-NOTICES.txt' = 'notice'
    }
}

function Test-ModernPortablePackage {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$ArchivePath, [Parameter(Mandatory)][string]$Version)
    $ErrorActionPreference = 'Stop'
    $resources = Get-ModernPackageResources
    $prefix = "WW-StudyApp-$Version-windows-x86_64-portable/"
    $manifestPath = 'support/manifest.json'
    $zip = [IO.Compression.ZipFile]::OpenRead([IO.Path]::GetFullPath($ArchivePath))
    try {
        if ($zip.Entries.Count -ne 7) { throw 'Expected exactly seven portable package entries' }
        $entries = [Collections.Generic.Dictionary[string,object]]::new([StringComparer]::Ordinal)
        foreach ($entry in $zip.Entries) {
            if (-not $entry.FullName.StartsWith($prefix,[StringComparison]::Ordinal)) { throw 'Unexpected portable package root' }
            $name = $entry.FullName.Substring($prefix.Length)
            if ($name -cnotin $resources.Keys -and $name -cne $manifestPath) { throw 'Unexpected portable package member' }
            if ($entry.Length -le 0 -or $entry.Length -gt 256MB -or $entries.ContainsKey($name)) { throw 'Duplicate, empty or excessive portable member' }
            $entries.Add($name,$entry)
        }
        if (-not $entries.ContainsKey($manifestPath) -or $entries[$manifestPath].Length -gt 1MB) { throw 'Missing or excessive portable manifest' }
        $reader = [IO.StreamReader]::new($entries[$manifestPath].Open(),[Text.UTF8Encoding]::new($false,$true))
        try { $manifest = $reader.ReadToEnd() | ConvertFrom-Json -AsHashtable } finally { $reader.Dispose() }
        if (($manifest.schema -isnot [long] -and $manifest.schema -isnot [int]) -or $manifest.schema -ne 1 -or
            $manifest.product -cne 'WW-StudyApp' -or $manifest.version -cne $Version -or $manifest.architecture -cne 'x86_64' -or
            $manifest.files.Count -ne $resources.Count) { throw 'Invalid portable manifest identity' }
        $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        $hashes = @{}
        foreach ($file in $manifest.files) {
            $name = $file.relative_path
            if ($name -isnot [string] -or $name -cnotin $resources.Keys -or -not $seen.Add($name) -or
                -not $entries.ContainsKey($name) -or $file.role -cne $resources[$name] -or
                ($file.size -isnot [long] -and $file.size -isnot [int]) -or $file.size -ne $entries[$name].Length -or
                $file.sha256 -isnot [string] -or $file.sha256 -cnotmatch '^[a-f0-9]{64}$') { throw 'Invalid portable manifest file' }
            $stream = $entries[$name].Open()
            $hasher = [Security.Cryptography.SHA256]::Create()
            try { $hash = [Convert]::ToHexString($hasher.ComputeHash($stream)).ToLowerInvariant() }
            finally { $hasher.Dispose(); $stream.Dispose() }
            if ($hash -cne $file.sha256) { throw "Portable member hash mismatch: $name" }
            $hashes[$name] = $hash
        }
        if ($hashes['WW-StudyApp.exe'] -cne $hashes['Uninstall.exe']) { throw 'Application and uninstaller must be byte-identical' }
        Write-Output 'Modern portable package verified: six owned resources and one bounded manifest.'
    } finally { $zip.Dispose() }
}

function New-ModernPortablePackage {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$LegacyStaging,
        [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
        [Parameter(Mandatory)][string]$OutputDirectory
    )
    $ErrorActionPreference = 'Stop'
    $output = [IO.Path]::GetFullPath($OutputDirectory)
    $source = (Resolve-Path -LiteralPath $LegacyStaging).Path
    $name = "WW-StudyApp-$Version-windows-x86_64-portable"
    $staging = Join-Path $output $name
    $archive = Join-Path $output "$name.zip"
    foreach ($path in @($staging,$archive,"$archive.sha256")) {
        if (Test-Path -LiteralPath $path) { throw 'Modern portable output already exists; select a fresh output directory' }
    }
    [IO.Directory]::CreateDirectory((Join-Path $staging 'support')) | Out-Null
    $files = @()
    $resources = Get-ModernPackageResources
    foreach ($relative in $resources.Keys) {
        $sourceName = if ($relative -ceq 'Uninstall.exe') { 'WW-StudyApp.exe' } else { $relative.Replace('support/','') }
        $origin = Get-Item -LiteralPath (Join-Path $source $sourceName)
        if ($origin.PSIsContainer -or ($origin.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Package resource must be a regular file' }
        $destination = Join-Path $staging $relative
        Copy-Item -LiteralPath $origin.FullName -Destination $destination
        $files += [ordered]@{
            relative_path = $relative
            role = $resources[$relative]
            size = (Get-Item -LiteralPath $destination).Length
            sha256 = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
        }
    }
    $manifest = [ordered]@{schema=1;product='WW-StudyApp';version=$Version;architecture='x86_64';files=$files}
    [IO.File]::WriteAllText((Join-Path $staging 'support/manifest.json'),($manifest | ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
    [IO.Compression.ZipFile]::CreateFromDirectory($staging,$archive,[IO.Compression.CompressionLevel]::Optimal,$true)
    Test-ModernPortablePackage -ArchivePath $archive -Version $Version | Out-Null
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$archive.sha256","$hash  $name.zip`n",[Text.UTF8Encoding]::new($false))
    [pscustomobject]@{Staging=$staging;Archive=$archive;Checksum="$archive.sha256"}
}
