param([string]$FixtureDirectory = (Join-Path $PSScriptRoot '../.scratch'), [switch]$GenerationOnly)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetFullPath($FixtureDirectory)) ('modern-package-fixtures-' + [guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($root) | Out-Null
$version = '1.2.3'
$resources = [ordered]@{
    'WW-StudyApp.exe' = 'application'
    'Uninstall.exe' = 'uninstaller'
    'vcruntime140.dll' = 'runtime'
    'vcruntime140_1.dll' = 'runtime'
    'support/LICENSE' = 'notice'
    'support/THIRD-PARTY-NOTICES.txt' = 'notice'
}
$cases = @('valid','wrong_hash','wrong_size','wrong_version','wrong_schema','wrong_product','wrong_arch','wrong_role','duplicate_manifest_path','duplicate_entry','case_alias','parent_path','backslash','extra_source','missing_runtime','different_uninstaller','empty_notice')
if ($GenerationOnly) { $cases = @() }
$passed = 0
foreach ($case in $cases) {
    $contents = [ordered]@{}
    $files = @()
    foreach ($name in $resources.Keys) {
        $bytes = [Text.Encoding]::UTF8.GetBytes($(if ($name -like '*.exe') { 'same desktop binary' } else { 'synthetic runtime or license' }))
        if ($case -eq 'different_uninstaller' -and $name -eq 'Uninstall.exe') { $bytes = [Text.Encoding]::UTF8.GetBytes('unexpected uninstaller') }
        if ($case -eq 'empty_notice' -and $name -eq 'support/LICENSE') { $bytes = [byte[]]@() }
        $contents[$name] = $bytes
        $files += [ordered]@{ relative_path=$name; role=$resources[$name]; size=$bytes.Length; sha256=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant() }
    }
    $manifest = [ordered]@{schema=1;product='WW-StudyApp';version=$version;architecture='x86_64';files=$files}
    switch ($case) {
        'wrong_hash' { $files[0].sha256 = '0' * 64 }
        'wrong_size' { $files[0].size++ }
        'wrong_version' { $manifest.version = '1.2.4' }
        'wrong_schema' { $manifest.schema = 2 }
        'wrong_product' { $manifest.product = 'another app' }
        'wrong_arch' { $manifest.architecture = 'arm64' }
        'wrong_role' { $files[0].role = 'notice' }
        'duplicate_manifest_path' { $manifest.files += $files[0] }
        'parent_path' { $contents['../outside.exe'] = [byte[]]@(1) }
        'backslash' { $contents['support\bad.txt'] = [byte[]]@(1) }
        'extra_source' { $contents['source.rs'] = [byte[]]@(1) }
        'missing_runtime' { $contents.Remove('vcruntime140_1.dll') }
    }
    $contents['support/manifest.json'] = [Text.Encoding]::UTF8.GetBytes(($manifest | ConvertTo-Json -Depth 8 -Compress))
    $archivePath = Join-Path $root "$case.zip"
    $zip = [IO.Compression.ZipFile]::Open($archivePath,[IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($name in $contents.Keys) {
            $entry = $zip.CreateEntry("WW-StudyApp-$version-windows-x86_64-portable/$name")
            $stream = $entry.Open()
            try { $stream.Write($contents[$name]) } finally { $stream.Dispose() }
        }
        if ($case -in @('duplicate_entry','case_alias')) {
            $name = if ($case -eq 'duplicate_entry') { 'WW-StudyApp.exe' } else { 'ww-studyapp.EXE' }
            $stream = $zip.CreateEntry("WW-StudyApp-$version-windows-x86_64-portable/$name").Open()
            try { $stream.WriteByte(1) } finally { $stream.Dispose() }
        }
    } finally { $zip.Dispose() }
    $accepted = $true
    try { & (Join-Path $PSScriptRoot 'check-package.ps1') -ArchivePath $archivePath -Version $version -Kind Portable | Out-Null }
    catch { $accepted = $false }
    if ($accepted -ne ($case -eq 'valid')) { throw "Modern package policy case failed: $case (accepted=$accepted)" }
    $passed++
}
. (Join-Path $PSScriptRoot 'modern-package.ps1')
$legacy = Join-Path $root 'legacy'
[IO.Directory]::CreateDirectory($legacy) | Out-Null
foreach ($name in @('WW-StudyApp.exe','vcruntime140.dll','vcruntime140_1.dll','LICENSE','THIRD-PARTY-NOTICES.txt','README.md','RELEASE-NOTES.md')) {
    [IO.File]::WriteAllText((Join-Path $legacy $name),"source $name",[Text.UTF8Encoding]::new($false))
}
$before = Get-FileHash -LiteralPath (Join-Path $legacy 'WW-StudyApp.exe')
$generated = New-ModernPortablePackage -LegacyStaging $legacy -Version $version -OutputDirectory $root
Test-ModernPortablePackage -ArchivePath $generated.Archive -Version $version | Out-Null
$actual = (Get-FileHash -LiteralPath $generated.Archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ((Get-Content -LiteralPath $generated.Checksum -Raw).Trim() -cne "$actual  WW-StudyApp-$version-windows-x86_64-portable.zip") { throw 'Generated ZIP checksum mismatch' }
if ((Get-FileHash -LiteralPath (Join-Path $legacy 'WW-StudyApp.exe')).Hash -cne $before.Hash) { throw 'Generator modified source payload' }
$collisionRejected = $false
try { New-ModernPortablePackage -LegacyStaging $legacy -Version $version -OutputDirectory $root | Out-Null }
catch { $collisionRejected = $true }
if (-not $collisionRejected) { throw 'Generator overwrote existing output' }
$passed += 2
Write-Output "Modern package policy: $passed cases passed; fixtures retained at $root"
