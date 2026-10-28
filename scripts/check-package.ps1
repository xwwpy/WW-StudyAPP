# Validate the user-facing portable package without executing its binary.
param([Parameter(Mandatory)][string]$ArchivePath, [Parameter(Mandatory)][string]$Version,
      [ValidateSet('Legacy','Portable')][string]$Kind = 'Legacy')
$ErrorActionPreference = 'Stop'
if ($Kind -eq 'Portable') {
    . (Join-Path $PSScriptRoot 'modern-package.ps1')
    Test-ModernPortablePackage -ArchivePath $ArchivePath -Version $Version
    return
}
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$allowed = @('WW-StudyApp.exe', 'vcruntime140.dll', 'vcruntime140_1.dll', 'LICENSE', 'THIRD-PARTY-NOTICES.txt', 'README.md', 'RELEASE-NOTES.md')
$prefix = "WW-StudyApp-$Version-windows-x86_64/"
$zip = [IO.Compression.ZipFile]::OpenRead([IO.Path]::GetFullPath($ArchivePath))
try {
    $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $zip.Entries) {
        if (-not $entry.FullName.StartsWith($prefix, [StringComparison]::Ordinal)) { throw 'Unexpected package root' }
        $relative = $entry.FullName.Substring($prefix.Length)
        if ($relative -cnotin $allowed -or -not $names.Add($relative) -or $entry.Length -eq 0) { throw "Unexpected, duplicate or empty package member: $relative" }
    }
    foreach ($required in $allowed) {
        if (-not $names.Contains($required)) { throw "Missing runtime/package file: $required" }
    }
    Write-Output "Portable package verified: $($names.Count) runtime and user-facing files; no source or development material."
} finally { $zip.Dispose() }
