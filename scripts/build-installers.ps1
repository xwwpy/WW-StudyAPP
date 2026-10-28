#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
    [Parameter(Mandatory)][string]$PayloadDirectory,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [switch]$BuildLifecycleFixtures,
    [string]$UiTestBinaryPath
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows' -or -not $IsWindows) { throw 'Installer tool acquisition and compilation run only on the Windows CI runner' }
if($BuildLifecycleFixtures -and (-not $UiTestBinaryPath -or -not (Test-Path -LiteralPath $UiTestBinaryPath -PathType Leaf))) {throw 'Lifecycle fixtures require the current-run ui-test binary'}
if($BuildLifecycleFixtures) {
    $nativeInput=Get-Item -LiteralPath $UiTestBinaryPath
    if($nativeInput.PSIsContainer -or ($nativeInput.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $nativeInput.Length -le 0 -or $nativeInput.Length -gt 256MB) {throw 'Private native validation binary must be one bounded regular file'}
    $UiTestBinaryPath=$nativeInput.FullName
}
$numericVersion = [version]$Version
if ($numericVersion.Major -gt 255 -or $numericVersion.Minor -gt 255 -or $numericVersion.Build -gt 65535) { throw 'Version exceeds Windows Installer three-field limits' }
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$payload = (Resolve-Path -LiteralPath $PayloadDirectory).Path
$output = [IO.Path]::GetFullPath($OutputDirectory)
$name = "WW-StudyApp-$Version-windows-x86_64"
$msi = Join-Path $output "$name.msi"
$setup = Join-Path $output "$name-Setup.exe"
foreach ($file in @($msi,"$msi.sha256",$setup,"$setup.sha256")) {
    if (Test-Path -LiteralPath $file) { throw "Installer output exists: $file" }
}
. (Join-Path $PSScriptRoot 'installer-policy.ps1')
. (Join-Path $PSScriptRoot 'modern-package.ps1')
$resources = Get-ModernPackageResources
$manifest = Get-Content -LiteralPath (Join-Path $payload 'support/manifest.json') -Raw | ConvertFrom-Json
if ($manifest.schema -ne 1 -or $manifest.product -cne 'WW-StudyApp' -or $manifest.version -cne $Version -or $manifest.architecture -cne 'x86_64' -or $manifest.files.Count -ne 6) { throw 'Installer staging manifest identity mismatch' }
$seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
foreach ($entry in $manifest.files) {
    if ($entry.relative_path -cnotin $resources.Keys -or -not $seen.Add($entry.relative_path) -or $entry.role -cne $resources[$entry.relative_path]) { throw 'Unexpected installer staging member' }
    $file = Get-Item -LiteralPath (Join-Path $payload $entry.relative_path)
    if ($file.PSIsContainer -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $file.Length -ne $entry.size -or
        (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant() -cne $entry.sha256) { throw 'Installer staging resource hash mismatch' }
}
if ((Get-FileHash -LiteralPath (Join-Path $payload 'WW-StudyApp.exe')).Hash -cne (Get-FileHash -LiteralPath (Join-Path $payload 'Uninstall.exe')).Hash) { throw 'Installer executable copies differ' }

$work = Join-Path ([IO.Path]::GetFullPath($env:RUNNER_TEMP)) ('studyapp-installer-' + [guid]::NewGuid().ToString('N'))
$feed = Join-Path $work 'feed'
$tools = Join-Path $work 'tools'
[IO.Directory]::CreateDirectory($feed) | Out-Null
if ($env:GITHUB_OUTPUT) { "installer-evidence=$work" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8 }
[IO.Directory]::CreateDirectory($output) | Out-Null
$wixVersion = '5.0.2'
$wixSize = 4587200
$wixSha512 = 'cfe7eed5d79371a57e4d10a8e3ee87ac9e3c49d9e3fb27908e3312b53237196ecdc4510f6a49d809dc6b7f4a84be0757ae361d8cb9e464a0ac352a934cea3638'
$wixPackageUri = 'https://api.nuget.org/v3-flatcontainer/wix/5.0.2/wix.5.0.2.nupkg'
$package = Join-Path $feed "wix.$wixVersion.nupkg"
Invoke-WebRequest -Uri $wixPackageUri -OutFile $package
if ((Get-Item -LiteralPath $package).Length -ne $wixSize -or (Get-FileHash -LiteralPath $package -Algorithm SHA512).Hash.ToLowerInvariant() -cne $wixSha512) { throw 'Pinned WiX compiler package failed size/SHA512 verification' }
$config = Join-Path $work 'NuGet.Config'
$escapedFeed = [Security.SecurityElement]::Escape($feed)
[IO.File]::WriteAllText($config,"<configuration><packageSources><clear/><add key=`"verified-local`" value=`"$escapedFeed`"/></packageSources><disabledPackageSources><clear/></disabledPackageSources></configuration>")
$oldPackages = [Environment]::GetEnvironmentVariable('NUGET_PACKAGES','Process')
$oldHttpCache = [Environment]::GetEnvironmentVariable('NUGET_HTTP_CACHE_PATH','Process')
$oldCliHome = [Environment]::GetEnvironmentVariable('DOTNET_CLI_HOME','Process')
Push-Location -LiteralPath $work
try {
    $env:NUGET_PACKAGES = Join-Path $work 'packages'
    $env:NUGET_HTTP_CACHE_PATH = Join-Path $work 'http-cache'
    $env:DOTNET_CLI_HOME = Join-Path $work 'dotnet-home'
    $installedSdks = @(& dotnet --list-sdks)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect runner .NET SDKs' }
    $sdk = @($installedSdks | ForEach-Object { if ($_ -match '^(10\.\d+\.\d+) \[') { $Matches[1] } } | Sort-Object { [version]$_ } -Descending | Select-Object -First 1)
    if ($sdk.Count -ne 1) { throw 'The Windows runner needs an already installed supported .NET 10 SDK' }
    $sdk = $sdk[0]
    @{sdk=@{version=$sdk;rollForward='disable';allowPrerelease=$false}} | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $work 'global.json') -Encoding utf8
    $selectedSdk = (& dotnet --version).Trim()
    if ($LASTEXITCODE -ne 0 -or $selectedSdk -cne $sdk) { throw 'Runner .NET SDK selection differs from the exact installed version' }
    & dotnet nuget verify $package --all *> (Join-Path $work 'nuget-signature.log')
    if ($LASTEXITCODE -ne 0) { throw 'WiX NuGet signature verification failed; inspect private log' }
    & dotnet tool install wix --version $wixVersion --tool-path $tools --configfile $config --no-cache --allow-roll-forward *> (Join-Path $work 'tool-install.log')
    if ($LASTEXITCODE -ne 0) { throw 'Isolated pinned WiX installation failed; inspect private log' }
    $wix = Join-Path $tools 'wix.exe'
    $actualVersion = (& $wix --version).Trim()
    if ($LASTEXITCODE -ne 0 -or $actualVersion -notmatch '^5\.0\.2(?:\+|$)') { throw 'WiX tool version differs from pin' }
    & $wix build (Join-Path $repo 'installer/Product.wxs') -arch x64 -d "Version=$Version" -d "PayloadDir=$payload" -intermediateFolder (Join-Path $work 'obj') -pdbtype none -out $msi *> (Join-Path $work 'wix-build.log')
    if ($LASTEXITCODE -ne 0) { throw "WiX compile/validation failed; inspect $work" }
    # wix build runs MSI validation by default; never suppress ICEs.
    $tables = Get-InstallerTables -MsiPath $msi
    Assert-InstallerTables -Tables $tables -Version $Version
    $tables | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $work 'msi-tables.json') -Encoding utf8
    $msiDigest = (Get-FileHash -LiteralPath $msi -Algorithm SHA256).Hash.ToLowerInvariant()
    $digestResource = Join-Path $work 'embedded.sha256'
    [IO.File]::WriteAllText($digestResource,$msiDigest,[Text.Encoding]::ASCII)
    $compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
    if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) { $compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework/v4.0.30319/csc.exe' }
    if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) { throw 'The runner has no existing .NET Framework C# compiler' }
    & $compiler /nologo /target:winexe /platform:x64 /optimize+ /warnaserror+ "/out:$setup" "/win32icon:$(Join-Path $repo 'assets/icon.ico')" "/win32manifest:$(Join-Path $repo 'installer/setup.manifest')" "/resource:$msi,StudyApp.Payload.msi" "/resource:$digestResource,StudyApp.Payload.sha256" (Join-Path $repo 'installer/Setup.cs') *> (Join-Path $work 'csc.log')
    if ($LASTEXITCODE -ne 0) { throw 'Setup wrapper compilation failed; inspect private csc.log' }
    # Inspect resources without running Main, MSI, or a window.
    $assembly = [Reflection.Assembly]::Load([IO.File]::ReadAllBytes($setup))
    $stream = $assembly.GetManifestResourceStream('StudyApp.Payload.msi')
    $hasher = [Security.Cryptography.SHA256]::Create()
    try { $embedded = [Convert]::ToHexString($hasher.ComputeHash($stream)).ToLowerInvariant() }
    finally { $hasher.Dispose(); $stream.Dispose() }
    $reader = [IO.StreamReader]::new($assembly.GetManifestResourceStream('StudyApp.Payload.sha256'))
    try { $embeddedChecksum = $reader.ReadToEnd() } finally { $reader.Dispose() }
    if ($embedded -cne $msiDigest -or $embeddedChecksum -cne $msiDigest) { throw 'Setup does not embed the exact standalone MSI/checksum' }
    # Exercise the production helper source without running Main: real Win32
    # sharing, real MSI database opening, and identity-bound cleanup under a swap.
    $nativeProbePath = Join-Path $work 'SetupNativeTests.exe'
    & $compiler /nologo /target:exe /platform:x64 /optimize+ /warnaserror+ /main:SetupNativeTests "/out:$nativeProbePath" (Join-Path $repo 'installer/Setup.cs') (Join-Path $repo 'scripts/setup-native-tests.cs') *> (Join-Path $work 'csc-native-probe.log')
    if ($LASTEXITCODE -ne 0) { throw 'Setup native sharing/cleanup probe failed compilation' }
    $nativeProbe = Start-Process -FilePath $nativeProbePath -ArgumentList @(('"'+$msi+'"'),('"'+$work+'"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $work 'setup-native-probe.stdout.log') -RedirectStandardError (Join-Path $work 'setup-native-probe.stderr.log')
    if (-not $nativeProbe.WaitForExit(60000)) { $nativeProbe.Kill(); throw 'Setup native sharing/cleanup probe timed out' }
    if ($nativeProbe.ExitCode -ne 0) { throw 'Setup native sharing/cleanup probe failed; inspect private evidence' }
    if ($BuildLifecycleFixtures) {
        # Private synthetic pre-release baseline. It uses the same desktop build;
        # it tests MSI ownership/version transactions, not historical app behavior.
        $fixtureVersion = '0.0.1'
        if ([version]$fixtureVersion -ge $numericVersion) { throw 'Current version must exceed the lifecycle fixture version' }
        $fixtures = Join-Path $work 'lifecycle'
        $fixturePayload = Join-Path $fixtures 'payload'
        [IO.Directory]::CreateDirectory((Join-Path $fixturePayload 'support')) | Out-Null
        if ($env:GITHUB_OUTPUT) { "installer-fixtures=$fixtures" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8 }
        foreach ($relative in @($resources.Keys) + @('support/manifest.json')) { Copy-Item -LiteralPath (Join-Path $payload $relative) -Destination (Join-Path $fixturePayload $relative) }
        $fixtureManifest = Get-Content -LiteralPath (Join-Path $fixturePayload 'support/manifest.json') -Raw | ConvertFrom-Json
        $fixtureManifest.version = $fixtureVersion
        $fixtureManifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $fixturePayload 'support/manifest.json') -Encoding utf8
        [IO.File]::WriteAllText((Join-Path $fixturePayload 'retired-runtime.txt'),'Synthetic old MSI-owned resource')
        $source = [IO.File]::ReadAllText((Join-Path $repo 'installer/Product.wxs'))
        $obsolete = @'
    <DirectoryRef Id="INSTALLFOLDER">
      <Component Id="RetiredFixture" Guid="{8484DD54-E42A-4D81-A12D-2268DE3583E6}">
        <File Id="RetiredFixture" Source="$(var.PayloadDir)\retired-runtime.txt" />
        <RegistryValue Id="RetiredFixture" Root="HKCU" Key="Software\WW-StudyApp\Installer" Name="RetiredFixture" Value="1" Type="integer" KeyPath="yes" />
      </Component>
    </DirectoryRef>
'@
        $oldSource = $source.Replace('    <Feature Id=', $obsolete + "`n    <Feature Id=").Replace('<ComponentRef Id="Shortcuts" />','<ComponentRef Id="Shortcuts" /><ComponentRef Id="RetiredFixture" />')
        $oldWxs = Join-Path $fixtures 'Previous.wxs'
        [IO.File]::WriteAllText($oldWxs,$oldSource)
        $oldName = "WW-StudyApp-$fixtureVersion-windows-x86_64"
        $oldMsi = Join-Path $fixtures "$oldName.msi"
        $oldSetup = Join-Path $fixtures "$oldName-Setup.exe"
        & $wix build $oldWxs -arch x64 -d "Version=$fixtureVersion" -d "PayloadDir=$fixturePayload" -intermediateFolder (Join-Path $fixtures 'old-obj') -pdbtype none -out $oldMsi *> (Join-Path $work 'wix-fixture.log')
        if ($LASTEXITCODE -ne 0) { throw 'Synthetic previous MSI failed compile/validation' }
        $oldDigest = (Get-FileHash -LiteralPath $oldMsi).Hash.ToLowerInvariant()
        $oldDigestResource = Join-Path $fixtures 'embedded.sha256'
        [IO.File]::WriteAllText($oldDigestResource,$oldDigest,[Text.Encoding]::ASCII)
        & $compiler /nologo /target:winexe /platform:x64 /optimize+ /warnaserror+ "/out:$oldSetup" "/win32manifest:$(Join-Path $repo 'installer/setup.manifest')" "/resource:$oldMsi,StudyApp.Payload.msi" "/resource:$oldDigestResource,StudyApp.Payload.sha256" (Join-Path $repo 'installer/Setup.cs') *> (Join-Path $work 'csc-fixture.log')
        if ($LASTEXITCODE -ne 0) { throw 'Synthetic previous Setup failed compilation' }
        $oldAssembly = [Reflection.Assembly]::Load([IO.File]::ReadAllBytes($oldSetup))
        $oldStream = $oldAssembly.GetManifestResourceStream('StudyApp.Payload.msi')
        $oldHasher = [Security.Cryptography.SHA256]::Create()
        try { $oldEmbedded = [Convert]::ToHexString($oldHasher.ComputeHash($oldStream)).ToLowerInvariant() }
        finally { $oldHasher.Dispose(); $oldStream.Dispose() }
        if ($oldEmbedded -cne $oldDigest) { throw 'Synthetic previous Setup payload differs' }
        # A separate private MSI must own the actual ui-test image so its
        # authenticated update handoff passes the unchanged production caller
        # path/hash/parent validation. Never substitute it into public payloads.
        $nativePayload=Join-Path $fixtures 'native-validation-payload'
        [IO.Directory]::CreateDirectory((Join-Path $nativePayload 'support')) | Out-Null
        $nativeResources=@()
        foreach($relative in $resources.Keys) {
            $origin=if($relative -in @('WW-StudyApp.exe','Uninstall.exe')) {$UiTestBinaryPath} else {Join-Path $payload $relative}
            $target=Join-Path $nativePayload $relative
            Copy-Item -LiteralPath $origin -Destination $target
            $nativeResources+= [ordered]@{relative_path=$relative;role=$resources[$relative];size=(Get-Item -LiteralPath $target).Length;sha256=(Get-FileHash -LiteralPath $target).Hash.ToLowerInvariant()}
        }
        $nativeManifest=[ordered]@{schema=1;product='WW-StudyApp';version=$fixtureVersion;architecture='x86_64';files=$nativeResources}
        [IO.File]::WriteAllText((Join-Path $nativePayload 'support/manifest.json'),($nativeManifest|ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
        $nativeMsi=Join-Path $fixtures 'NativeValidation-0.0.1.msi'
        & $wix build (Join-Path $repo 'installer/Product.wxs') -arch x64 -d "Version=$fixtureVersion" -d "PayloadDir=$nativePayload" -intermediateFolder (Join-Path $fixtures 'native-obj') -pdbtype none -out $nativeMsi *> (Join-Path $work 'wix-native-fixture.log')
        if($LASTEXITCODE -ne 0) {throw 'Private ui-test MSI failed compile/validation'}
        $nativeTables=Get-InstallerTables -MsiPath $nativeMsi
        Assert-InstallerTables -Tables $nativeTables -Version $fixtureVersion
        $nativeTables|ConvertTo-Json -Depth 12|Set-Content -LiteralPath (Join-Path $fixtures 'native-msi-tables.json') -Encoding utf8
        [ordered]@{private=$true;msi_version=$fixtureVersion;desktop_build_version=$Version;ui_test_binary_sha256=(Get-FileHash -LiteralPath $UiTestBinaryPath).Hash.ToLowerInvariant();msi_sha256=(Get-FileHash -LiteralPath $nativeMsi).Hash.ToLowerInvariant()}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $fixtures 'native-fixture-evidence.json') -Encoding utf8
        $failure = @'
    <CustomAction Id="FixtureFail" Error="Synthetic upgrade rollback regression." />
    <InstallExecuteSequence><Custom Action="FixtureFail" After="InstallFiles" Condition="NOT Installed" /></InstallExecuteSequence>
'@
        $failedWxs = Join-Path $fixtures 'FailedUpgrade.wxs'
        [IO.File]::WriteAllText($failedWxs,$source.Replace('    <Feature Id=',$failure + "`n    <Feature Id="))
        & $wix build $failedWxs -arch x64 -d "Version=$Version" -d "PayloadDir=$payload" -intermediateFolder (Join-Path $fixtures 'failed-obj') -pdbtype none -out (Join-Path $fixtures 'FailedUpgrade.msi') *> (Join-Path $work 'wix-failed-fixture.log')
        if ($LASTEXITCODE -ne 0) { throw 'Controlled failed-upgrade fixture did not compile/validate' }
        $rebootWxs=Join-Path $fixtures 'RebootFixture.wxs'
        $rebootSequence='<InstallExecuteSequence><ScheduleReboot Before="InstallFinalize" Condition="NOT Installed" /></InstallExecuteSequence>'
        [IO.File]::WriteAllText($rebootWxs,$source.Replace('    <Feature Id=',$rebootSequence+"`n    <Feature Id="))
        $rebootMsi=Join-Path $fixtures 'RebootFixture.msi'
        & $wix build $rebootWxs -arch x64 -d "Version=$Version" -d "PayloadDir=$payload" -intermediateFolder (Join-Path $fixtures 'reboot-obj') -pdbtype none -out $rebootMsi *> (Join-Path $work 'wix-reboot-fixture.log')
        if($LASTEXITCODE -ne 0) { throw 'Real ScheduleReboot fixture failed compile/validation' }
        $rebootDigest=Join-Path $fixtures 'reboot.sha256'
        [IO.File]::WriteAllText($rebootDigest,(Get-FileHash -LiteralPath $rebootMsi).Hash.ToLowerInvariant(),[Text.Encoding]::ASCII)
        & $compiler /nologo /target:winexe /platform:x64 /optimize+ /warnaserror+ "/out:$(Join-Path $fixtures 'RebootSetup.exe')" "/win32manifest:$(Join-Path $repo 'installer/setup.manifest')" "/resource:$rebootMsi,StudyApp.Payload.msi" "/resource:$rebootDigest,StudyApp.Payload.sha256" (Join-Path $repo 'installer/Setup.cs') *> (Join-Path $work 'csc-reboot-fixture.log')
        if($LASTEXITCODE -ne 0) { throw 'Real ScheduleReboot Setup fixture failed compilation' }
        & $compiler /nologo /target:exe /platform:x64 /optimize+ /warnaserror+ "/out:$(Join-Path $fixtures 'MsiControlNativeTests.exe')" (Join-Path $repo 'scripts/msi-control-native-tests.cs') *> (Join-Path $work 'csc-msi-control.log')
        if($LASTEXITCODE -ne 0) { throw 'Real MSI cancellation client failed compilation' }
        $oldTables = Get-InstallerTables -MsiPath $oldMsi
        $oldTables | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $fixtures 'old-msi-tables.json') -Encoding utf8
        $oldProduct = @($oldTables.Property | Where-Object { $_.Property -ceq 'ProductCode' })[0].Value
        $newProduct = @($tables.Property | Where-Object { $_.Property -ceq 'ProductCode' })[0].Value
        if ($oldProduct -ieq $newProduct) { throw 'MSI versions reused the product identity' }
        [ordered]@{synthetic=$true;desktop_build_version=$Version;msi_version=$fixtureVersion;retired_resource='retired-runtime.txt';msi_sha256=$oldDigest;embedded_msi_sha256=$oldEmbedded} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $fixtures 'fixture-evidence.json') -Encoding utf8
    }
    foreach ($file in @($msi,$setup)) {
        $info = Get-Item -LiteralPath $file
        if ($info.Length -le 0 -or $info.Length -gt 256MB) { throw 'Installer exceeds the updater package size bound' }
        $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
        [IO.File]::WriteAllText("$file.sha256","$hash  $($info.Name)`n",[Text.UTF8Encoding]::new($false))
    }
    [ordered]@{wix_version=$actualVersion;wix_package_sha512=$wixSha512;dotnet_sdk=$sdk;msi_sha256=$msiDigest;embedded_msi_sha256=$embedded;setup_sha256=(Get-FileHash -LiteralPath $setup).Hash.ToLowerInvariant()} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $work 'build-evidence.json') -Encoding utf8
    [pscustomobject]@{Msi=$msi;Setup=$setup;Evidence=$work}
} finally {
    [Environment]::SetEnvironmentVariable('NUGET_PACKAGES',$oldPackages,'Process')
    [Environment]::SetEnvironmentVariable('NUGET_HTTP_CACHE_PATH',$oldHttpCache,'Process')
    [Environment]::SetEnvironmentVariable('DOTNET_CLI_HOME',$oldCliHome,'Process')
    Pop-Location
}
