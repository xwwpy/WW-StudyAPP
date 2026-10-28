param([string]$FixtureDirectory = (Join-Path $PSScriptRoot '../.scratch'))
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$root = Join-Path ([IO.Path]::GetFullPath($FixtureDirectory)) ('package-fixtures-' + [guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($root) | Out-Null
$version = '1.2.3'
$required = @('WW-StudyApp.exe', 'vcruntime140.dll', 'vcruntime140_1.dll', 'LICENSE', 'THIRD-PARTY-NOTICES.txt', 'README.md', 'RELEASE-NOTES.md')
$cases = @(
    @{ Names = $required; Valid = $true },
    @{ Names = ($required + 'Cargo.lock'); Valid = $false },
    @{ Names = ($required + 'editor-package-lock.json'); Valid = $false },
    @{ Names = ($required + 'third-party/library/source.rs'); Valid = $false },
    @{ Names = ($required + 'docs/ui-optimization-prompts.md'); Valid = $false },
    @{ Names = ($required + 'studyapp.pdb'); Valid = $false },
    @{ Names = ($required + 'LICENSE'); Valid = $false },
    @{ Names = @($required | Where-Object { $_ -ne 'vcruntime140_1.dll' }); Valid = $false }
)
$passed = 0
foreach ($case in $cases) {
    $path = Join-Path $root ($passed.ToString() + '.zip')
    $archive = [IO.Compression.ZipFile]::Open($path, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($name in $case.Names) {
            $entry = $archive.CreateEntry("WW-StudyApp-$version-windows-x86_64/$name")
            $stream = $entry.Open()
            try { $stream.WriteByte(42) } finally { $stream.Dispose() }
        }
    } finally { $archive.Dispose() }
    $accepted = $true
    try { & (Join-Path $PSScriptRoot 'check-package.ps1') -ArchivePath $path -Version $version | Out-Null }
    catch { $accepted = $false }
    if ($accepted -ne $case.Valid) { throw "Package policy case $passed failed" }
    $passed++
}
Write-Output "Package policy: $passed cases passed."
