"""Silent source/entry guards; native executor acceptance runs only in Windows CI."""
from pathlib import Path
import os
import shutil
import subprocess
import unittest
import uuid

ROOT = Path(__file__).resolve().parents[1]


class MaintenanceAcceptanceSources(unittest.TestCase):
    def test_authenticated_result_polling_retries_empty_partial_and_null_json(self):
        command=r'''
        Set-StrictMode -Version Latest
        $ErrorActionPreference='Stop'
        $tokens=$null;$errors=$null
        $ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PWD 'scripts/test-maintenance.ps1'),[ref]$tokens,[ref]$errors)
        if($errors.Count){throw 'Invalid source'}
        $definition=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq 'Wait-AuthenticatedResult'},$true)
        . ([scriptblock]::Create($definition.Extent.Text))
        $base=Join-Path $PWD ('.scratch/installation-20261026/result-polling-'+[guid]::NewGuid().ToString('N'))
        $runner=[IO.Path]::GetFullPath($base)+[IO.Path]::DirectorySeparatorChar
        $execution=Join-Path $base 'evidence'
        [void][IO.Directory]::CreateDirectory($execution)
        # Advance the real result file at the polling boundary, without timing,
        # native processes, or copying the acceptance loop into this regression.
        function Start-Sleep {
            param($Milliseconds)
            $script:polls++
            if($script:polls -gt 1){throw 'Polling failed to consume the complete result'}
            [IO.File]::WriteAllText($script:resultFile,$script:completeJson)
        }
        foreach($case in @('empty','partial','null','wrong-transaction','failed-state')) {
            $stage=Join-Path $base $case
            [void][IO.Directory]::CreateDirectory($stage)
            $script:resultFile=Join-Path $stage 'result.json'
            $script:completeJson='{"transaction_id":"exact-transaction","state":"succeeded","version":"0.11.0","code":"complete"}'
            $initial=switch($case) {
                'empty' {''}
                'partial' {'{"transaction_id":'}
                'null' {'null'}
                'wrong-transaction' {$script:completeJson.Replace('exact-transaction','other-transaction')}
                'failed-state' {$script:completeJson.Replace('succeeded','failed')}
            }
            [IO.File]::WriteAllText($script:resultFile,$initial)
            $process=[pscustomobject]@{Id=42;ExitCode=0}
            $process | Add-Member ScriptMethod WaitForExit {param($Milliseconds) $script:waited++;return $true}
            $process | Add-Member ScriptMethod Dispose {$script:disposed++}
            $handoff=[pscustomobject]@{stage=$stage;helper_pid=42;helper_created=123;transaction_id='exact-transaction'}
            $identity=[pscustomobject]@{Process=$process;Id=42;Created=123;Image=(Join-Path $stage 'WW-StudyApp.exe');Transaction='exact-transaction'}
            $script:polls=0;$script:waited=0;$script:disposed=0
            $rejected=$false
            try {$result=Wait-AuthenticatedResult $handoff $case -ResultMode 'silent-update' -AuthenticatedHelper $identity}
            catch {
                if($_.Exception.Message -cne 'Authenticated child did not finish the exact transaction'){throw}
                $rejected=$true
            }
            if($case -in @('wrong-transaction','failed-state')) {
                if(-not $rejected -or $script:polls -ne 0 -or $script:waited -ne 0){throw "Invalid completed result was accepted or retried: $case"}
            } else {
                if($rejected){throw "Incomplete result was rejected before the completed write: $case"}
                if($result.transaction_id -cne $handoff.transaction_id -or $result.state -cne 'succeeded' -or $script:polls -ne 1 -or $script:waited -ne 1){throw "Completed result or helper exit was not verified: $case"}
                if(-not (Test-Path -LiteralPath (Join-Path $execution "$case-result.json"))){throw 'Completed result evidence was not copied'}
            }
            if($script:disposed -ne 1){throw 'Bound helper handle was not disposed'}
        }
        '5 authenticated result polling cases passed without native calls'
        '''
        result=subprocess.run([shutil.which("pwsh"),"-NoProfile","-Command",command],cwd=ROOT,capture_output=True,text=True,encoding="utf-8",timeout=30)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("5 authenticated result polling cases passed without native calls",result.stdout)

    def test_msi_inventory_accepts_api_owner_without_arp_and_records_diagnostics(self):
        command=r'''
        Set-StrictMode -Version Latest
        $ErrorActionPreference='Stop'
        foreach($source in @('scripts/installer-policy.ps1','scripts/test-maintenance.ps1')) {
            $tokens=$null;$errors=$null
            $ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PWD $source),[ref]$tokens,[ref]$errors)
            if($errors.Count){throw 'Invalid source'}
            foreach($definition in $ast.FindAll({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst]},$false)) {
                . ([scriptblock]::Create($definition.Extent.Text))
            }
        }
        function Test-Path {param($LiteralPath) return $true}
        function Get-ChildItem {param($LiteralPath) return}
        function Get-InstallerApiInventory {
            $script:apiCalls++
            [pscustomobject]@{
                CurrentSid='S-1-5-21-1';Family='{4CF5A7C2-094D-4D97-AB25-24C6C2BAA4CD}'
                Related=@('{11111111-1111-4111-8111-111111111111}');Errors=@();Calls=@(@{Api='synthetic';Code=0})
                Instances=@([pscustomobject]@{ProductCode='{11111111-1111-4111-8111-111111111111}';Context=2;UserSid='S-1-5-21-1';ProductName='WW-StudyApp';Version='0.8.0';InstallLocation=$script:expectedRoot;ProductState='5'})
            }
        }
        $script:apiCalls=0
        $script:expectedRoot=[IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Programs/WW-StudyApp')).TrimEnd('\')
        $output=Join-Path $PWD ('.scratch/installation-20261026/msi-inventory-test-'+[guid]::NewGuid().ToString('N'))
        [void][IO.Directory]::CreateDirectory($output)
        $owners=@(Get-FamilyOwners)
        if($owners.Count -ne 1 -or $script:apiCalls -ne 1){throw 'An authoritative MSI owner was lost merely because ARP was empty'}
        if($owners[0].ProductCode -cne '{11111111-1111-4111-8111-111111111111}' -or -not $owners[0].CurrentUser -or $owners[0].Version -cne '0.8.0' -or $owners[0].Root -cne $script:expectedRoot){throw 'API owner mapping differs'}
        $diagnostics=@([IO.Directory]::GetFiles($output,'msi-inventory-*.json'))
        if($diagnostics.Count -ne 1){throw 'Owner inventory diagnostics were not persisted'}
        $record=[IO.File]::ReadAllText($diagnostics[0]) | ConvertFrom-Json
        if(-not $record.Passed -or $record.Api.Instances.Count -ne 1 -or $record.Arp.Count -ne 0){throw 'API-only diagnostic evidence differs'}
        'authoritative API-only owner passed without native calls'
        '''
        result=subprocess.run([shutil.which("pwsh"),"-NoProfile","-Command",command],cwd=ROOT,capture_output=True,text=True,encoding="utf-8",timeout=30)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("authoritative API-only owner passed without native calls",result.stdout)

    def test_owner_inventory_skips_empty_foreign_entries_and_rejects_malformed_target(self):
        command=r'''
        Set-StrictMode -Version Latest
        $ErrorActionPreference='Stop'
        function Test-Path {param($LiteralPath) return $true}
        function Get-ChildItem {param($LiteralPath)
            if($script:noKeys){return}
            return [pscustomobject]@{PSPath=($LiteralPath+'\fixture');PSChildName='{11111111-1111-4111-8111-111111111111}'}
        }
        function Get-ItemProperty {param($LiteralPath) $script:reads++;return $script:properties}
        $definitions=@(
            @{Path='scripts/installer-policy.ps1';Name='Get-InstallerArpOwners';Scope='CurrentUser';Root='Root'},
            @{Path='scripts/installer-policy.ps1';Name='Get-InstallerArpOwners';Scope='Scope';Root='InstallLocation'}
        )
        $failures=@()
        foreach($definition in $definitions) {
            try {
                $tokens=$null;$errors=$null
                $ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PWD $definition.Path),[ref]$tokens,[ref]$errors)
                if($errors.Count){throw 'Invalid source'}
                $function=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq $definition.Name},$true)
                . ([scriptblock]::Create($function.Extent.Text))
                $script:noKeys=$false
                foreach($script:properties in @(
                    [pscustomobject]@{},
                    $null,
                    [pscustomobject]@{Unrelated='entry'},
                    [pscustomobject]@{DisplayName='Other application'},
                    [pscustomobject]@{DisplayName=$null},
                    [pscustomobject]@{DisplayName='ww-studyapp'}
                )) {
                    $script:reads=0
                    $owners=@(& $definition.Name)
                    if($owners.Count -ne 0 -or $script:reads -ne 3){throw "Empty/foreign inventory was not safely skipped: $($definition.Name)"}
                }
                $script:properties=[pscustomobject]@{DisplayName='WW-StudyApp';DisplayVersion='1.2.3';InstallLocation='C:\fixture\StudyApp'}
                $owners=@(& $definition.Name)
                if($owners.Count -ne 3){throw 'Valid target inventory lost registry roots'}
                for($index=0;$index -lt 3;$index++) {
                    $owner=$owners[$index]
                    if($owner.ProductCode -cne '{11111111-1111-4111-8111-111111111111}' -or $owner.($definition.Scope) -ne ($index -eq 0) -or $owner.Version -cne '1.2.3' -or $owner.($definition.Root) -cne 'C:\fixture\StudyApp'){throw 'Owner identity/scope/version/root mapping changed'}
                }
                foreach($script:properties in @(
                    [pscustomobject]@{DisplayName='WW-StudyApp';InstallLocation='C:\fixture\StudyApp'},
                    [pscustomobject]@{DisplayName='WW-StudyApp';DisplayVersion='1.2.3'}
                )) {
                    $rejected=$false
                    try {& $definition.Name | Out-Null} catch [Management.Automation.PropertyNotFoundException] {$rejected=$true}
                    if(-not $rejected){throw 'Malformed target registration was silently ignored or accepted'}
                }
                # Raw fields must be persisted before the strict malformed-target throw.
                $script:properties=[pscustomobject]@{DisplayName='WW-StudyApp';DisplayVersion='1.2.3'}
                $rawDiagnostics=@{Arp=@()}
                $rejected=$false
                try {& $definition.Name -CurrentSid 'S-1-5-21-1' -Diagnostics $rawDiagnostics | Out-Null} catch [Management.Automation.PropertyNotFoundException] {$rejected=$true}
                if(-not $rejected -or @($rawDiagnostics.Arp).Count -ne 1 -or @($rawDiagnostics.Arp)[0].ProductCode -cne '{11111111-1111-4111-8111-111111111111}'){throw 'Malformed target was not persisted before the strict failure'}
                foreach($script:properties in @(
                    [pscustomobject]@{DisplayName=@('WW-StudyApp','Other application');DisplayVersion='1.2.3';InstallLocation='C:\fixture\StudyApp'},
                    [pscustomobject]@{DisplayName=@('Other application','WW-StudyApp');DisplayVersion='1.2.3';InstallLocation='C:\fixture\StudyApp'}
                )) {
                    $script:reads=0
                    $owners=@(& $definition.Name)
                    if($owners.Count -ne 3){throw 'Multi-string DisplayName naming WW-StudyApp was skipped'}
                }
                foreach($script:properties in @(
                    [pscustomobject]@{DisplayName=@('Other application');DisplayVersion='1.2.3';InstallLocation='C:\fixture\StudyApp'},
                    [pscustomobject]@{DisplayName=@();DisplayVersion='1.2.3';InstallLocation='C:\fixture\StudyApp'}
                )) {
                    $script:reads=0
                    $owners=@(& $definition.Name)
                    if($owners.Count -ne 0 -or $script:reads -ne 3){throw 'Multi-string DisplayName without the product was not safely skipped'}
                }
                $script:noKeys=$true
                if(@(& $definition.Name).Count -ne 0){throw 'Empty registry inventory produced an owner'}
            } catch {$failures+="$($definition.Name): $($_.Exception.Message)"}
        }
        if($failures.Count){throw ($failures -join '; ')}
        '30 strict owner inventory cases passed without registry access'
        '''
        result=subprocess.run([shutil.which("pwsh"),"-NoProfile","-Command",command],cwd=ROOT,capture_output=True,text=True,encoding="utf-8",timeout=30)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("30 strict owner inventory cases passed without registry access",result.stdout)

    def test_msi_inventory_rejects_conflicts_and_persists_every_failed_comparison(self):
        command=r'''
        Set-StrictMode -Version Latest
        $ErrorActionPreference='Stop'
        . (Join-Path $PWD 'scripts/installer-policy.ps1')
        $base=Join-Path $PWD ('.scratch/installation-20261026/msi-inventory-matrix-'+[guid]::NewGuid().ToString('N'))
        [void][IO.Directory]::CreateDirectory($base)
        $product='{11111111-1111-4111-8111-111111111111}'
        $other='{22222222-2222-4222-8222-222222222222}'
        $root='C:\fixture\StudyApp';$sid='S-1-5-21-1'
        $seed=@{CurrentSid=$sid;Family='{4CF5A7C2-094D-4D97-AB25-24C6C2BAA4CD}';Related=@($product);Errors=@();Calls=@(@{Api='synthetic';Code=0});Instances=@(@{ProductCode=$product;Context=2;UserSid=$sid;ProductName='WW-StudyApp';Version='0.8.0';InstallLocation=$root;ProductState='5'})} | ConvertTo-Json -Depth 8
        $arpSeed=@{ProductCode=$product;Context=2;UserSid=$sid;ProductName='WW-StudyApp';Version='0.8.0';Root=$root;InstallLocation=$root;WindowsInstaller=1;RegistryPath='synthetic HKCU';CurrentUser=$true;Scope=$true} | ConvertTo-Json
        function Get-InstallerApiInventory {if($script:apiThrows){throw 'synthetic API boundary failure 1605'};return $script:api}
        function Get-InstallerArpOwners {param($CurrentSid,$RelatedProducts,$Diagnostics) $Diagnostics.Arp=@($script:arp);return $script:arp}
        function Invoke-InventoryCase {
            param([string]$Name,[scriptblock]$Change,[bool]$ShouldPass=$false,[int]$Count=1)
            $script:api=$seed|ConvertFrom-Json;$script:arp=@();$script:apiThrows=$false
            & $Change
            $directory=Join-Path $base $Name;[void][IO.Directory]::CreateDirectory($directory)
            $rejected=$false;$owners=@()
            try {$owners=@(Get-VerifiedInstallerOwners -OutputDirectory $directory -Label $Name -ExpectedRoot $root -ExpectedProducts @($product) -ExpectedVersion '0.8.0' -ExpectedCount $Count)} catch {$rejected=$true}
            if($rejected -eq $ShouldPass){throw "Unexpected inventory outcome: $Name"}
            $files=@([IO.Directory]::GetFiles($directory,'msi-inventory-*.json'))
            if($files.Count -ne 1){throw "Missing diagnostic: $Name"}
            $record=[IO.File]::ReadAllText($files[0])|ConvertFrom-Json
            $script:lastRecord=$record
            if($record.Passed -ne $ShouldPass -or $record.Label -cne $Name -or $record.Expected.Root -cne $root -or $record.Expected.Version -cne '0.8.0' -or $record.Expected.Count -ne $Count){throw "Incorrect diagnostic: $Name"}
            if($ShouldPass -and $owners.Count -ne $Count){throw "Wrong accepted owner count: $Name"}
            if(-not $ShouldPass -and $record.Errors.Count -eq 0){throw "Failed diagnostic lacks error: $Name"}
            if(-not $ShouldPass -and -not $script:apiThrows -and $record.Conflicts.Count -eq 0){throw "Conflict was silently discarded: $Name"}
            if($Name -ceq 'api-error' -and $record.Api.Calls[0].Code -ne 1605){throw 'Original API error code was discarded'}
            if($Name -ceq 'matching-arp' -and ($record.Arp.Count -ne 1 -or $record.Checks[1].ApiInstance -ne $true)){throw 'Exact API/ARP instance was not correlated'}
        }
        Invoke-InventoryCase 'api-only' {} $true
        Invoke-InventoryCase 'matching-arp' {$script:arp=@($arpSeed|ConvertFrom-Json)} $true
        Invoke-InventoryCase 'empty-clean' {$script:api.Instances=@();$script:api.Related=@()} $true 0
        Invoke-InventoryCase 'machine' {$script:api.Instances[0].Context=4;$script:api.Instances[0].UserSid=''}
        Invoke-InventoryCase 'managed' {$script:api.Instances[0].Context=1}
        Invoke-InventoryCase 'other-sid' {$script:api.Instances[0].UserSid='S-1-5-21-2'}
        Invoke-InventoryCase 'invalid-current-sid' {$script:api.CurrentSid='invalid'}
        Invoke-InventoryCase 'wrong-family' {$script:api.Family=$other}
        Invoke-InventoryCase 'wrong-name' {$script:api.Instances[0].ProductName='Other application'}
        Invoke-InventoryCase 'advertised' {$script:api.Instances[0].ProductState='1'}
        Invoke-InventoryCase 'wrong-root' {$script:api.Instances[0].InstallLocation='C:\other\StudyApp'}
        Invoke-InventoryCase 'relative-root' {$script:api.Instances[0].InstallLocation='relative\StudyApp'}
        Invoke-InventoryCase 'alias-root' {$script:api.Instances[0].InstallLocation='C:\fixture\..\StudyApp'}
        Invoke-InventoryCase 'wrong-version' {$script:api.Instances[0].Version='0.8.1'}
        Invoke-InventoryCase 'invalid-version' {$script:api.Instances[0].Version='garbage'}
        Invoke-InventoryCase 'wrong-product' {$script:api.Related=@($other);$script:api.Instances[0].ProductCode=$other}
        Invoke-InventoryCase 'invalid-guid' {$script:api.Related=@('bad');$script:api.Instances[0].ProductCode='bad'}
        Invoke-InventoryCase 'duplicate-related' {$script:api.Related=@($product,$product)}
        Invoke-InventoryCase 'duplicate-api' {$script:api.Instances=@($script:api.Instances[0],$script:api.Instances[0])}
        Invoke-InventoryCase 'family-without-instance' {$script:api.Instances=@()}
        Invoke-InventoryCase 'api-error' {$script:api.Errors=@('MsiGetProductInfoExW:State:1605');$script:api.Calls[0].Code=1605}
        Invoke-InventoryCase 'api-throws' {$script:apiThrows=$true}
        Invoke-InventoryCase 'unknown-arp' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].ProductCode=$other}
        Invoke-InventoryCase 'machine-arp-mirror' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].Context=4;$script:arp[0].UserSid=''} $true
        Invoke-InventoryCase 'machine-arp-version-mismatch' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].Context=4;$script:arp[0].UserSid='';$script:arp[0].Version='0.7.3'}
        Invoke-InventoryCase 'machine-arp-root-mismatch' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].Context=4;$script:arp[0].UserSid='';$script:arp[0].InstallLocation='C:\other\StudyApp';$script:arp[0].Root='C:\other\StudyApp'}
        Invoke-InventoryCase 'machine-arp-name-mismatch' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].Context=4;$script:arp[0].UserSid='';$script:arp[0].ProductName='Other application'}
        Invoke-InventoryCase 'machine-arp-two-owners' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].Context=4;$script:arp[0].UserSid='';$script:api.Instances=@($script:api.Instances[0],$script:api.Instances[0])}
        Invoke-InventoryCase 'machine-arp-foreign-sid' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].Context=4;$script:arp[0].UserSid='S-1-5-21-2'}
        Invoke-InventoryCase 'other-sid-arp' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].UserSid='S-1-5-21-2'}
        Invoke-InventoryCase 'wrong-root-arp' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].InstallLocation='C:\other\StudyApp'}
        Invoke-InventoryCase 'wrong-version-arp' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].Version='0.7.3'}
        Invoke-InventoryCase 'wrong-name-arp' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].ProductName='Other application'}
        Invoke-InventoryCase 'non-msi-arp' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].WindowsInstaller=0}
        # Registry values can be collections; each named comparison must stay a
        # Boolean so a malformed tuple is a conflict instead of a silent pass.
        Invoke-InventoryCase 'array-name-exact' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].ProductName=@('WW-StudyApp')}
        Invoke-InventoryCase 'array-name-other' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].ProductName=@('Other application')}
        if(-not ($script:lastRecord.Arp[0].ProductName -is [array])) {throw 'Raw multi-string ARP name was not preserved for diagnostics'}
        Invoke-InventoryCase 'array-name-mixed' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].ProductName=@('WW-StudyApp','Other application')}
        Invoke-InventoryCase 'empty-array-name' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].ProductName=@()}
        Invoke-InventoryCase 'array-msi-one' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].WindowsInstaller=@(1)}
        Invoke-InventoryCase 'array-msi-zero' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].WindowsInstaller=@(0)}
        Invoke-InventoryCase 'empty-array-msi' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].WindowsInstaller=@()}
        Invoke-InventoryCase 'array-version-marked' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].Version=@('0.8.0')}
        Invoke-InventoryCase 'array-root-marked' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].InstallLocation=@($root);$script:arp[0].Root=@($root)}
        # The native boundary returns Dictionary<string,object> rows with UInt32
        # context, so that exact shape must still be accepted.
        Invoke-InventoryCase 'dictionary-api' {$d=[Collections.Generic.Dictionary[string,object]]::new();$d['ProductCode']=$product;$d['Context']=[uint32]2;$d['UserSid']=$sid;$d['ProductName']='WW-StudyApp';$d['Version']='0.8.0';$d['InstallLocation']=$root;$d['ProductState']='5';$script:api.Instances=@($d)} $true
        Invoke-InventoryCase 'dictionary-arp' {$d=[Collections.Generic.Dictionary[string,object]]::new();$d['ProductCode']=$product;$d['Context']=[uint32]2;$d['UserSid']=$sid;$d['ProductName']='WW-StudyApp';$d['Version']='0.8.0';$d['Root']=$root;$d['InstallLocation']=$root;$d['WindowsInstaller']=[uint32]1;$script:arp=@($d)} $true
        Invoke-InventoryCase 'ordered-dictionary-arp' {$script:arp=@([ordered]@{ProductCode=$product;Context=2;UserSid=$sid;ProductName='WW-StudyApp';Version='0.8.0';Root=$root;InstallLocation=$root;WindowsInstaller=1})} $true
        # MSI writes ARPINSTALLLOCATION from [INSTALLFOLDER], which ends in a
        # separator after CostFinalize; the normalized comparison must accept it.
        Invoke-InventoryCase 'trailing-separator-arp' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].InstallLocation=$root+'\';$script:arp[0].Root=$root+'\'} $true
        Invoke-InventoryCase 'null-name-arp' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].ProductName=$null}
        Invoke-InventoryCase 'boolean-msi-arp' {$script:arp=@($arpSeed|ConvertFrom-Json);$script:arp[0].WindowsInstaller=$true}
        Invoke-InventoryCase 'duplicate-arp' {$row=$arpSeed|ConvertFrom-Json;$script:arp=@($row,$row)}
        Invoke-InventoryCase 'unexpected-present' {} $false 0
        '51 authoritative inventory cases passed without API or registry access'
        '''
        result=subprocess.run([shutil.which("pwsh"),"-NoProfile","-Command",command],cwd=ROOT,capture_output=True,text=True,encoding="utf-8",timeout=45)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("51 authoritative inventory cases passed without API or registry access",result.stdout)

    def test_live_snapshot_treats_database_as_runtime_state_and_detects_changes(self):
        command=r'''
        Set-StrictMode -Version Latest
        $ErrorActionPreference='Stop'
        $tokens=$null;$errors=$null
        $ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PWD 'scripts/test-maintenance.ps1'),[ref]$tokens,[ref]$errors)
        if($errors.Count){throw 'Invalid source'}
        foreach($name in @('Get-DataSnapshot','Assert-DataSnapshot')) {
            $definition=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq $name},$true)
            . ([scriptblock]::Create($definition.Extent.Text))
        }
        $root=Join-Path $PWD ('.scratch/installation-20261026/snapshot-test-'+[guid]::NewGuid().ToString('N'))
        [void][IO.Directory]::CreateDirectory($root)
        Set-Content -LiteralPath (Join-Path $root 'studyapp.sqlite3') -Value 'db' -NoNewline
        Set-Content -LiteralPath (Join-Path $root 'studyapp.sqlite3-wal') -Value 'wal' -NoNewline
        Set-Content -LiteralPath (Join-Path $root 'studyapp.lock') -Value 'lock' -NoNewline
        Set-Content -LiteralPath (Join-Path $root 'notes.md') -Value 'note' -NoNewline
        $live=Get-DataSnapshot $root -SkipRuntimeLocks
        if($live.ContainsKey('studyapp.lock')) {throw 'Live lock was counted'}
        if(-not $live['studyapp.sqlite3'].ContainsKey('runtime') -or -not $live['studyapp.sqlite3-wal'].ContainsKey('runtime')) {throw 'Live database files were not runtime state'}
        if(-not $live.ContainsKey('notes.md')) {throw 'Live user file was lost'}
        Assert-DataSnapshot $live (Get-DataSnapshot $root -SkipRuntimeLocks)
        Add-Content -LiteralPath (Join-Path $root 'studyapp.sqlite3-wal') -Value 'more'
        Assert-DataSnapshot $live (Get-DataSnapshot $root -SkipRuntimeLocks)
        Add-Content -LiteralPath (Join-Path $root 'notes.md') -Value 'more'
        $changed=$false
        try {Assert-DataSnapshot $live (Get-DataSnapshot $root -SkipRuntimeLocks)} catch {$changed=$true}
        if(-not $changed){throw 'Changed live user file was accepted'}
        $fixture=Get-DataSnapshot $root
        if(-not $fixture.ContainsKey('studyapp.lock')) {throw 'Fixture inventory lost the lock'}
        if($fixture['studyapp.sqlite3'].ContainsKey('runtime')) {throw 'Fixture inventory treated the database as runtime state'}
        'live snapshot semantics passed'
        '''
        result=subprocess.run([shutil.which("pwsh"),"-NoProfile","-Command",command],cwd=ROOT,capture_output=True,text=True,encoding="utf-8",timeout=30)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("live snapshot semantics passed",result.stdout)

    def test_msi_inventory_native_boundary_compiles_without_invocation_and_callers_diagnose(self):
        command=r'''
        Set-StrictMode -Version Latest
        $ErrorActionPreference='Stop'
        $tokens=$null;$errors=$null
        $ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PWD 'scripts/installer-policy.ps1'),[ref]$tokens,[ref]$errors)
        if($errors.Count){throw 'Invalid inventory source'}
        $definition=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq 'Get-InstallerApiInventory'},$true)
        $code=$definition.Find({param($node) $node -is [Management.Automation.Language.StringConstantExpressionAst] -and $node.Value.StartsWith('using System;')},$true)
        if($null -eq $code){throw 'Native inventory declarations missing'}
        Add-Type -TypeDefinition $code.Value
        # Compiling declarations does not invoke the native Collect entrypoint.
        . ([scriptblock]::Create($definition.Extent.Text))
        $saved=$env:GITHUB_ACTIONS
        try {$env:GITHUB_ACTIONS='false';$blocked=$false;try {Get-InstallerApiInventory|Out-Null} catch {if($_.Exception.Message -match 'isolated Windows CI runner'){$blocked=$true}};if(-not $blocked){throw 'Native inventory escaped its CI boundary'}} finally {$env:GITHUB_ACTIONS=$saved}
        foreach($source in @('scripts/test-maintenance.ps1','scripts/smoke-maintenance.ps1')) {
            $tokens=$null;$errors=$null
            $null=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PWD $source),[ref]$tokens,[ref]$errors)
            if($errors.Count){throw 'Invalid inventory caller'}
        }
        'native boundary compiled and remained uninvoked'
        '''
        result=subprocess.run([shutil.which("pwsh"),"-NoProfile","-Command",command],cwd=ROOT,capture_output=True,text=True,encoding="utf-8",timeout=45)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("native boundary compiled and remained uninvoked",result.stdout)
        native=(ROOT / "scripts/installer-policy.ps1").read_text(encoding="utf-8")
        self.assertIn('MsiEnumProductsExW(productCode,"S-1-1-0",7,j',native)
        self.assertIn('context==4?null:sid.ToString()',native)
        self.assertIn('"VersionString","InstallLocation","ProductName","State"',native)
        for path,labels in (
            ("scripts/test-maintenance.ps1",("reboot-3010-postcondition","reboot-finally-before-uninstall","running-installer-finally-before-uninstall","msi-adapter-finally-before-uninstall")),
            ("scripts/smoke-maintenance.ps1",("assert-owner","lifecycle-finally-before-uninstall")),
        ):
            source=(ROOT / path).read_text(encoding="utf-8")
            for label in labels:
                self.assertIn(label,source)

    def test_authenticated_result_modes_keep_identity_exit_and_window_guards(self):
        command=r'''
        Set-StrictMode -Version Latest
        $ErrorActionPreference='Stop'
        $tokens=$null;$errors=$null
        $ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PWD 'scripts/test-maintenance.ps1'),[ref]$tokens,[ref]$errors)
        if($errors.Count){throw 'Invalid source'}
        foreach($name in @('Get-AuthenticatedHelper','Wait-AuthenticatedResult')) {
            $definition=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq $name},$true)
            if($null -eq $definition){throw "Missing result guard: $name"}
            . ([scriptblock]::Create($definition.Extent.Text))
        }
        $runner=Join-Path $PWD ('.scratch/installation-20261026/msi-silent-result-test-'+[guid]::NewGuid().ToString('N'))
        $stage=Join-Path $runner 'stage';$execution=Join-Path $runner 'evidence'
        [void][IO.Directory]::CreateDirectory($stage);[void][IO.Directory]::CreateDirectory($execution)
        $runner=$runner.TrimEnd('\')+'\'
        $handoff=[pscustomobject]@{stage=$stage;helper_pid=123;helper_created=([datetime]'2026-01-01Z').ToFileTimeUtc();transaction_id='exact-transaction'}
        $script:fake=$null;$script:copies=@();$script:waits=0;$script:disposals=0
        function Get-Process {param($Id,$ErrorAction) return $script:fake}
        function Start-Sleep {param($Milliseconds) throw 'test-poll-without-required-evidence'}
        function Copy-Item {param($LiteralPath,$Destination) $script:copies+=,$Destination}
        function New-Helper {
            $helper=[pscustomobject]@{Id=123;SafeHandle='bound-handle';StartTime=([datetime]'2026-01-01Z');MainModule=[pscustomobject]@{FileName=(Join-Path $stage 'WW-StudyApp.exe')};ExitCode=0;Exited=$true}
            $helper | Add-Member ScriptMethod WaitForExit {param($timeout) $script:waits++;return $this.Exited}
            $helper | Add-Member ScriptMethod Dispose {$script:disposals++}
            return $helper
        }
        function Write-Result {param($Transaction='exact-transaction',$State='succeeded')
            @{transaction_id=$Transaction;state=$State} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $stage 'result.json')
        }
        function Assert-Rejected {param([scriptblock]$Action,[string]$Message)
            $reason=$null;try{& $Action | Out-Null}catch{$reason=$_.Exception.Message}
            if($null -eq $reason -or $reason -notlike "*$Message*"){throw "Wrong rejection: wanted $Message; got $reason"}
        }
        Write-Result
        $script:fake=New-Helper
        $observed=Get-AuthenticatedHelper $handoff
        # Retaining the authenticated handle permits observing a child after it
        # has left the process inventory, without treating absent PID as exit 0.
        $script:fake=$null
        $result=Wait-AuthenticatedResult $handoff 'msi' -ResultMode 'silent-update' -AuthenticatedHelper $observed
        if($result.transaction_id -cne 'exact-transaction' -or $script:waits -ne 1 -or $script:disposals -ne 1 -or $script:copies.Count -ne 1 -or $script:copies[0] -cne (Join-Path $execution 'msi-result.json')){throw 'Silent update did not authenticate exact success/exit/JSON-only evidence'}
        $script:fake=New-Helper
        Assert-Rejected {Wait-AuthenticatedResult $handoff 'default'} 'test-poll-without-required-evidence'
        $png=[byte[]]::new(100);([byte[]]@(137,80,78,71,13,10,26,10)).CopyTo($png,0)
        [IO.File]::WriteAllBytes((Join-Path $stage 'maintenance-result.png'),$png)
        $script:copies=@();$script:fake=New-Helper
        $null=Wait-AuthenticatedResult $handoff 'window'
        if($script:copies.Count -ne 2 -or $script:copies[0] -cne (Join-Path $execution 'window-result.png')){throw 'Window result did not preserve PNG evidence'}
        $script:fake=New-Helper;Write-Result -Transaction 'wrong'
        Assert-Rejected {Wait-AuthenticatedResult $handoff 'msi' -ResultMode 'silent-update' -AuthenticatedHelper (Get-AuthenticatedHelper $handoff)} 'exact transaction'
        $script:fake=New-Helper;Write-Result -State 'rolled_back'
        Assert-Rejected {Wait-AuthenticatedResult $handoff 'msi' -ResultMode 'silent-update' -AuthenticatedHelper (Get-AuthenticatedHelper $handoff)} 'exact transaction'
        Assert-Rejected {Wait-AuthenticatedResult $handoff 'msi' -ExpectedState 'rolled_back' -ResultMode 'silent-update'} 'requires succeeded'
        Write-Result
        $script:fake=$null
        Assert-Rejected {Get-AuthenticatedHelper $handoff} 'not observable'
        Assert-Rejected {Wait-AuthenticatedResult $handoff 'msi' -ResultMode 'silent-update'} 'previously bound'
        $script:fake=New-Helper;$observed=Get-AuthenticatedHelper $handoff;$observed.Transaction='other'
        Assert-Rejected {Wait-AuthenticatedResult $handoff 'msi' -ResultMode 'silent-update' -AuthenticatedHelper $observed} 'identity differs'
        $script:fake=New-Helper;$script:fake.StartTime=$script:fake.StartTime.AddSeconds(1)
        Assert-Rejected {Get-AuthenticatedHelper $handoff} 'creation time differs'
        $script:fake=New-Helper;$script:fake.MainModule.FileName=Join-Path $stage 'other.exe'
        Assert-Rejected {Get-AuthenticatedHelper $handoff} 'image differs'
        $script:fake=New-Helper;$script:fake.Id=456
        Assert-Rejected {Get-AuthenticatedHelper $handoff} 'process ID differs'
        $script:fake=New-Helper;$script:fake.ExitCode=1
        Assert-Rejected {Wait-AuthenticatedResult $handoff 'msi' -ResultMode 'silent-update' -AuthenticatedHelper (Get-AuthenticatedHelper $handoff)} 'helper failed'
        $script:fake=New-Helper;$script:fake.Exited=$false
        Assert-Rejected {Wait-AuthenticatedResult $handoff 'msi' -ResultMode 'silent-update' -AuthenticatedHelper (Get-AuthenticatedHelper $handoff)} 'helper did not exit'
        $script:fake=New-Helper;$png[0]=0
        [IO.File]::WriteAllBytes((Join-Path $stage 'maintenance-result.png'),$png)
        Assert-Rejected {Wait-AuthenticatedResult $handoff 'window'} 'screenshot is missing or invalid'
        '15 authenticated result guard cases passed'
        '''
        result=subprocess.run([shutil.which("pwsh"),"-NoProfile","-Command",command],cwd=ROOT,capture_output=True,text=True,encoding="utf-8",timeout=30)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("15 authenticated result guard cases passed",result.stdout)

    def test_only_msi_update_opts_out_of_result_window_with_early_handle_binding(self):
        source=(ROOT / "scripts/test-maintenance.ps1").read_text(encoding="utf-8")
        self.assertIn("-AuthenticatedHelper ([ref]$msiHelper)",source)
        self.assertIn("Wait-AuthenticatedResult $handoff 'msi' -ResultMode 'silent-update' -AuthenticatedHelper $msiHelper",source)
        self.assertEqual(source.count("-ResultMode 'silent-update'"),1)
        self.assertIn("result_window_expected=$false;result_json='msi-result.json'",source)
        self.assertNotIn("result_screenshot='msi-result.png'",source)
        bind=source.index('$AuthenticatedHelper.Value=Get-AuthenticatedHelper')
        self.assertLess(bind,source.index('$process.WaitForExit(600000)',bind))

    def test_msi_driver_binds_helper_before_waiting_and_retries_partial_handoff(self):
        command=r'''
        Set-StrictMode -Version Latest
        $ErrorActionPreference='Stop'
        $tokens=$null;$errors=$null
        $ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PWD 'scripts/test-maintenance.ps1'),[ref]$tokens,[ref]$errors)
        $definition=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq 'Invoke-Acceptance'},$true)
        . ([scriptblock]::Create($definition.Extent.Text))
        $output=Join-Path $PWD ('.scratch/installation-20261026/msi-silent-result-observer-'+[guid]::NewGuid().ToString('N'))
        $portable='verified.zip';$msi='verified.msi';$Version='1.2.3'
        $script:reads=0;$script:bound=$false;$script:waited=$false;$script:sleeps=0
        $driver=[pscustomobject]@{HasExited=$false;ExitCode=0;SafeHandle='own-driver-handle'}
        $driver | Add-Member ScriptMethod WaitForExit {param($timeout) if(-not $script:bound){throw 'Parent waited before authenticating helper'};$script:waited=$true;return $true}
        $driver | Add-Member ScriptMethod Dispose {}
        function Start-Process {return $script:driver}
        function Get-FileHash {return [pscustomobject]@{Hash=('a'*64)}}
        function Test-Path {return $true}
        function Start-Sleep {$script:sleeps++}
        function Get-Content {param($LiteralPath,[switch]$Raw)
            if($LiteralPath.EndsWith('msi-handoff.json')) {
                $script:reads++
                if($script:reads -eq 1){return '{'}
                return '{"transaction_id":"observed-transaction"}'
            }
            return (@{schema=1;native_windows_runner=$true;version=$Version;source_sha=$env:GITHUB_SHA;msi_sha256=('a'*64);complete=$true}|ConvertTo-Json)
        }
        function Get-AuthenticatedHelper {param($Handoff)
            if($Handoff.transaction_id -cne 'observed-transaction'){throw 'Wrong handoff bound'}
            $script:bound=$true;return [pscustomobject]@{Identity='authenticated-OS-handle'}
        }
        $observed=$null
        $null=Invoke-Acceptance 'mock-driver' (Join-Path $output 'owner') '--maintenance-execution-smoke' 'msi-owner' -AuthenticatedHelper ([ref]$observed)
        if(-not $script:bound -or -not $script:waited -or $script:reads -ne 2 -or $script:sleeps -ne 1 -or $observed.Identity -cne 'authenticated-OS-handle'){throw 'Early helper observation did not survive partial JSON'}
        'early helper handle binding and partial handoff retry passed'
        '''
        result=subprocess.run([shutil.which("pwsh"),"-NoProfile","-Command",command],cwd=ROOT,capture_output=True,text=True,encoding="utf-8",timeout=30)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("early helper handle binding and partial handoff retry passed",result.stdout)

    def test_msi_observer_failures_finish_owned_driver_and_preserve_unknown_helper(self):
        command=r'''
        Set-StrictMode -Version Latest
        $ErrorActionPreference='Stop'
        $tokens=$null;$errors=$null
        $ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PWD 'scripts/test-maintenance.ps1'),[ref]$tokens,[ref]$errors)
        $definition=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq 'Invoke-Acceptance'},$true)
        # Accelerate only this observer's bounded clock; execute its real error
        # handling and exact driver handle operations with simulated processes.
        . ([scriptblock]::Create($definition.Extent.Text.Replace('.AddSeconds(120)','.AddMilliseconds(20)')))
        $cleanup=$ast.Find({param($node) $node -is [Management.Automation.Language.IfStatementAst] -and $node.Clauses[0].Item1.Extent.Text -ceq '$msiCleanupSafe'},$true)
        if($null -eq $cleanup){throw 'Missing unknown-helper cleanup guard'}
        $cleanupBlock=[scriptblock]::Create($cleanup.Extent.Text)
        $output=Join-Path $PWD ('.scratch/installation-20261026/msi-silent-result-error-'+[guid]::NewGuid().ToString('N'))
        $portable='verified.zip';$msi='verified.msi';$Version='1.2.3';$msiCleanupSafe=$false
        function Start-Process {return $script:driver}
        function Test-Path {return $script:mode -cne 'timeout'}
        function Start-Sleep {}
        function Get-Content {param($LiteralPath,[switch]$Raw) if($script:mode -ceq 'partial'){return '{'};return '{"transaction_id":"exact"}'}
        function Get-AuthenticatedHelper {throw 'primary-helper-identity-error'}
        function Get-FamilyOwners {$script:cleanupCalls++;throw 'Unknown helper allowed owner cleanup'}
        function Invoke-NativeProcess {$script:cleanupCalls++;throw 'Unknown helper allowed uninstall'}
        foreach($script:mode in @('timeout','partial','identity','teardown-failure','unbound-driver')) {
            $script:waitCalls=0;$script:kills=0;$script:disposed=0;$script:cleanupCalls=0
            $script:driver=[pscustomobject]@{HasExited=$false;ExitCode=0;SafeHandle='own-driver-handle'}
            if($script:mode -ceq 'unbound-driver') {
                $script:driver.PSObject.Properties.Remove('SafeHandle')
                $script:driver | Add-Member ScriptProperty SafeHandle {throw 'primary-driver-bind-error'}
            }
            $script:driver | Add-Member ScriptMethod WaitForExit {param($timeout) $script:waitCalls++;return $script:waitCalls -gt 1}
            $script:driver | Add-Member ScriptMethod Kill {$script:kills++;if($script:mode -ceq 'teardown-failure'){throw 'secondary-driver-stop-error'}}
            $script:driver | Add-Member ScriptMethod Dispose {$script:disposed++}
            $observation=$null;$reason=$null
            try {$null=Invoke-Acceptance 'mock-driver' (Join-Path $output $script:mode) '--maintenance-execution-smoke' 'msi-owner' -AuthenticatedHelper ([ref]$observation)} catch {$reason=$_.Exception.Message}
            if($script:mode -ceq 'unbound-driver') {
                if($reason -notlike '*driver OS handle could not be bound*' -or $script:kills -ne 0 -or $script:waitCalls -ne 0){throw "Unbound driver was operated on: reason=$reason kills=$script:kills waits=$script:waitCalls"}
            } else {
                if($null -eq $reason -or $script:kills -ne 1 -or $script:waitCalls -lt 1){throw "Observer did not finish its exact driver: $script:mode"}
                if($script:mode -in @('identity','teardown-failure') -and $reason -notlike '*primary-helper-identity-error*'){throw 'Driver teardown masked primary failure'}
            }
            & $cleanupBlock
            if($script:disposed -ne 1 -or $script:cleanupCalls -ne 0 -or $null -ne $observation){throw 'Failed observation leaked driver or cleaned unknown helper fixture'}
        }
        '5 observer failure cleanup cases passed'
        '''
        result=subprocess.run([shutil.which("pwsh"),"-NoProfile","-Command",command],cwd=ROOT,capture_output=True,text=True,encoding="utf-8",timeout=30)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("5 observer failure cleanup cases passed",result.stdout)
        source=(ROOT / "scripts/test-maintenance.ps1").read_text(encoding="utf-8")
        self.assertIn('$msiCleanupSafe=$false',source)
        wait=source.index("$result=Wait-AuthenticatedResult $handoff 'msi'")
        self.assertLess(wait,source.index('$msiCleanupSafe=$true',wait))

    def test_runtime_pair_predicate_requires_both_distinct_dlls(self):
        command=r'''
        $tokens=$null;$errors=$null
        $ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PWD 'scripts/test-maintenance.ps1'),[ref]$tokens,[ref]$errors)
        if($errors.Count){throw 'Invalid source'}
        $definition=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq 'Test-RecoveryRuntimePairDistinct'},$true)
        . ([scriptblock]::Create($definition.Extent.Text))
        $current=@{'vcruntime140.dll'='new-first';'vcruntime140_1.dll'='new-second'}
        $cases=@(
          @{name='same';first='new-first';second='new-second';expected=$false},
          @{name='only-first';first='old-first';second='new-second';expected=$false},
          @{name='only-second';first='new-first';second='old-second';expected=$false},
          @{name='both-different';first='old-first';second='old-second';expected=$true}
        )
        foreach($case in $cases){$candidate=@{'vcruntime140.dll'=$case.first;'vcruntime140_1.dll'=$case.second};if((Test-RecoveryRuntimePairDistinct $candidate $current) -ne $case.expected){throw "Invalid runtime pair decision: $($case.name)"}}
        if(Test-RecoveryRuntimePairDistinct @{'vcruntime140.dll'='old'} $current){throw 'Incomplete pair accepted'}
        '5 real runtime predicate cases passed'
        '''
        result=subprocess.run([shutil.which("pwsh"),"-NoProfile","-Command",command],cwd=ROOT,capture_output=True,text=True,encoding="utf-8",timeout=30)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("5 real runtime predicate cases passed",result.stdout)

    def test_recovery_launches_verified_snapshot_and_clicks_production_recovery(self):
        path=ROOT / "crates/studyapp-desktop/src/maintenance_recovery_acceptance.rs"
        self.assertTrue(path.exists(),"Actual recovery UI acceptance child is required")
        source=path.read_text(encoding="utf-8")
        for actual in ('recovery_snapshot(', 'handoff_observer', 'window.click("maintenance-recover", cx)', 'render_to_image()', 'run_with_opened('):
            self.assertIn(actual,source)
        producer=(ROOT / "crates/studyapp-desktop/src/maintenance_acceptance.rs").read_text(encoding="utf-8")
        self.assertIn('std::process::exit(86)',producer)
        self.assertIn('"backed_up"',producer)
        self.assertIn('"installed"',producer)
        self.assertIn('recovery-boundary.json',producer)
        script=(ROOT / "scripts/test-maintenance.ps1").read_text(encoding="utf-8")
        self.assertIn("Join-Path $boundary.snapshot 'Uninstall.exe'",script)
        self.assertIn("'recovery-missing-runtime','recovery-mixed-runtime'",script)
        self.assertIn("Wait-AuthenticatedResult $recoveryHandoff",script)

    def test_mixed_runtime_requires_unmodified_signed_x64_runtime_inventory(self):
        script=(ROOT / "scripts/test-maintenance.ps1").read_text(encoding="utf-8")
        for actual in ('Get-AuthenticodeSignature', '0x8664', 'Microsoft Corporation', 'recovery-runtime-inventory.json', 'Get-RecoveryRuntimePair', 'current_package_hashes'):
            self.assertIn(actual,script)

    def test_live_data_snapshot_comparator_rejects_changed_missing_and_extra_files(self):
        shell=shutil.which("pwsh")
        self.assertIsNotNone(shell)
        command=r'''
        $errors=$null;$tokens=$null
        $ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PWD 'scripts/test-maintenance.ps1'),[ref]$tokens,[ref]$errors)
        if($errors.Count){throw 'Invalid source'}
        $definition=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq 'Assert-DataSnapshot'},$true)
        . ([scriptblock]::Create($definition.Extent.Text))
        function New-State {@{'studyapp.sqlite3'=@{size=5;sha256='a'};'user.md'=@{size=9;sha256='b'}}}
        Assert-DataSnapshot (New-State) (New-State)
        $mutants=@(
          {param($s) $s['studyapp.sqlite3'].sha256='changed'},
          {param($s) $s['user.md'].size=12},
          {param($s) [void]$s.Remove('user.md')},
          {param($s) $s['new-file']=@{size=1;sha256='c'}}
        )
        foreach($mutate in $mutants){$after=New-State;& $mutate $after;$rejected=$false;try{Assert-DataSnapshot (New-State) $after}catch{$rejected=$true};if(-not $rejected){throw 'Snapshot mutant accepted'}}
        '5 data snapshot cases passed'
        '''
        result=subprocess.run([shell,"-NoProfile","-Command",command],cwd=ROOT,capture_output=True,text=True,encoding="utf-8",timeout=30)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("5 data snapshot cases passed",result.stdout)

    def test_in_use_installers_keep_exact_live_process_and_validate_complete_state(self):
        source=(ROOT / "scripts/test-maintenance.ps1").read_text(encoding="utf-8")
        for actual in ('function Add-RunningInstallerCases', "foreach($kind in @('msi','setup'))", '$application.SafeHandle', '$application.MainModule.FileName', 'Assert-InstallerRollbackState', 'Assert-CurrentInstallerPayload', '$application.HasExited', "scenario='installer-running-app'", 'Get-DataSnapshot'):
            self.assertIn(actual,source)
        wix=(ROOT / "installer/Product.wxs").read_text(encoding="utf-8")
        self.assertIn('Id="MSIRESTARTMANAGERCONTROL" Value="Disable"',wix)
        setup=(ROOT / "installer/Setup.cs").read_text(encoding="utf-8")
        self.assertIn('MSIRESTARTMANAGERCONTROL=Disable',setup)

    def test_offline_retry_uses_dedicated_real_ui_transport_suite(self):
        source=(ROOT / "crates/studyapp-desktop/src/maintenance_acceptance.rs").read_text(encoding="utf-8")
        self.assertIn('crate::update_ui::run_repair_acceptance(',source)
        self.assertNotIn('root.join("offline.zip")',source)
        self.assertNotIn('"automatic_network_downloads":0',source)
        script=(ROOT / "scripts/test-maintenance.ps1").read_text(encoding="utf-8")
        self.assertIn("'--maintenance-execution-smoke' 'repair'",script)

    def test_cancellations_click_real_views_with_production_executor_bound(self):
        hook=(ROOT / "crates/studyapp-desktop/src/maintenance_cancel_acceptance.rs").read_text(encoding="utf-8")
        for actual in ('window.click("maintenance-cancel", cx)', 'window.click("maintenance-next", cx)', 'prepare_uninstall(', 'begin_uninstall_handoff(', 'calls.get()', 'render_to_image()'):
            self.assertIn(actual, hook)
        source=(ROOT / "crates/studyapp-desktop/src/maintenance_acceptance.rs").read_text(encoding="utf-8")
        self.assertIn('run_cancel_acceptance(',source)
        self.assertNotIn('state.stage() == stage && state.finish().is_err()',source)
        script=(ROOT / "scripts/test-maintenance.ps1").read_text(encoding="utf-8")
        self.assertIn("'cancel-selection','cancel-database-review','cancel-database-final'",script)

    def test_real_health_uses_production_ack_and_a_captured_process_handle(self):
        source=(ROOT / "crates/studyapp-desktop/src/maintenance_acceptance.rs").read_text(encoding="utf-8")
        self.assertIn("apply_portable_plan(&prepared)",source)
        self.assertIn("restart_and_wait_for_health(",source)
        self.assertIn('record(records, "actual-storage-ready-health"',source)
        process=(ROOT / "crates/studyapp-desktop/src/maintenance_acceptance_process.rs").read_text(encoding="utf-8")
        self.assertIn("QueryFullProcessImageNameW",process)
        self.assertIn("same_absolute_path(&image, expected)",process)
        self.assertIn("TerminateProcess(self.handle.0",process)

    def test_no_webview_gate_runs_views_with_an_unavailable_browser(self):
        source=(ROOT / "scripts/smoke-maintenance.ps1").read_text(encoding="utf-8")
        self.assertIn("$env:WEBVIEW2_BROWSER_EXECUTABLE_FOLDER=$unavailableWebview",source)
        self.assertIn("scenario='maintenance-without-webview2';passed=$true;executor='real'",source)
        self.assertIn("SetEnvironmentVariable('WEBVIEW2_BROWSER_EXECUTABLE_FOLDER',$oldBrowserFolder",source)
        self.assertIn("$execution.scenarios)+@($results",source)

    def test_legacy_cases_use_the_actual_schema1_helper_and_reconciliation(self):
        source = (ROOT / "crates/studyapp-desktop/src/maintenance_acceptance.rs").read_text(encoding="utf-8")
        self.assertIn('include_str!("update_helper.ps1")', source)
        self.assertIn("maintenance-legacy-native.ps1", source)
        driver = (ROOT / "scripts/maintenance-legacy-native.ps1").read_text(encoding="utf-8")
        for api in ("Read-ValidatedEntries", "Prepare-Update $plan", "Invoke-UpdateReplacement $plan", "unexpected_executable"):
            self.assertIn(api, driver)

    def test_cli_guard_precedes_fixture_creation(self):
        source = (ROOT / "crates/studyapp-desktop/src/maintenance_acceptance.rs").read_text(encoding="utf-8")
        self.assertLess(source.index('"GITHUB_ACTIONS"'), source.index("create_private_directory(&fixture_root)"))
        self.assertIn('"RUNNER_TEMP"', source)
        self.assertIn("root_components.starts_with(&runner_components)", source)
        self.assertIn("!fixture_root.try_exists()?", source)
        self.assertIn("same_absolute_path(&evidence", source)

    def test_scenarios_use_real_journal_confirmation_and_executor(self):
        source = (ROOT / "crates/studyapp-desktop/src/maintenance_acceptance.rs").read_text(encoding="utf-8")
        for api in ("apply_with_health(", "prepare_uninstall(", "apply_authenticated_uninstall(", "confirm_database_review()", 'confirm_database_final("删除数据库")', "reconcile_current_package("):
            self.assertIn(api, source)
        self.assertIn("injected_health_is_not_restart_evidence", source)
        self.assertIn("body()?", source)

    def test_script_refuses_local_native_execution_without_creating_output(self):
        shell = shutil.which("pwsh")
        self.assertIsNotNone(shell)
        path = ROOT / ".scratch/installation-20261026" / ("native-guard-" + uuid.uuid4().hex)
        env = dict(os.environ)
        env.pop("GITHUB_ACTIONS", None)
        env.pop("RUNNER_OS", None)
        result = subprocess.run([shell, "-NoProfile", "-File", str(ROOT / "scripts/test-maintenance.ps1"), "-Version", "1.2.3", "-UiTestBinaryPath", "missing", "-PortablePackage", "missing", "-MsiPath", "missing", "-OutputDirectory", str(path)], env=env, capture_output=True, text=True, encoding="utf-8", timeout=30)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Windows CI runner", result.stderr)
        self.assertFalse(path.exists())


if __name__ == "__main__":
    unittest.main()
