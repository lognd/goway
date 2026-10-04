# Locate goway-setup in a protected place and run it by absolute path (used by winadmin.rs for
# the administrator routes; only single quotes, so the text can sit inside other quoting).
#
# Never a search path: PATH, PATHEXT and the current directory are not consulted, because any
# program the Windows user runs can plant a goway-setup in a user-writable PATH directory, and
# this runs with an administrator token. A copy is accepted only when it, and every directory
# from the one holding it up to the first directory under the machine-wide base, is owned by
# SYSTEM, Administrators or TrustedInstaller, is not a link, and grants nobody else any write,
# append, delete or permission-changing right.

function Test-GowayProtectedPath {
    param([string]$Path)
    $trusted = @('S-1-5-18', 'S-1-5-32-544', 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
    $mask = [Security.AccessControl.FileSystemRights]'WriteData, AppendData, WriteExtendedAttributes, WriteAttributes, Delete, DeleteSubdirectoriesAndFiles, ChangePermissions, TakeOwnership'
    $item = Get-Item -LiteralPath $Path -Force
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { return $false }
    $acl = Get-Acl -LiteralPath $Path
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($trusted -notcontains $owner) { return $false }
    foreach ($ace in $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
        if ($ace.AccessControlType -ne 'Allow') { continue }
        if ($ace.PropagationFlags -band [Security.AccessControl.PropagationFlags]::InheritOnly) { continue }
        if ($trusted -contains $ace.IdentityReference.Value) { continue }
        if ([int]($ace.FileSystemRights -band $mask) -ne 0) { return $false }
    }
    return $true
}

function Find-GowaySetup {
    $found = @()
    if ($env:ProgramFiles) {
        $dir = Join-Path $env:ProgramFiles 'goway'
        $found += , @((Join-Path $dir 'goway-setup.exe'), @($dir))
    }
    if ($env:ProgramData) {
        $root = Join-Path $env:ProgramData 'goway'
        if (Test-Path -LiteralPath $root -PathType Container) {
            foreach ($profileDir in @(Get-ChildItem -LiteralPath $root -Directory -Force)) {
                $bin = Join-Path $profileDir.FullName 'bin'
                $found += , @((Join-Path $bin 'goway-setup.exe'), @($root, $profileDir.FullName, $bin))
            }
        }
    }
    foreach ($candidate in $found) {
        $file = $candidate[0]
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { continue }
        $ok = Test-GowayProtectedPath $file
        foreach ($dir in $candidate[1]) {
            if ($ok) { $ok = (Test-Path -LiteralPath $dir -PathType Container) -and (Test-GowayProtectedPath $dir) }
        }
        if ($ok) { return $file }
    }
    return $null
}

function Invoke-GowaySetup {
    param([string[]]$SetupArgs)
    $setup = Find-GowaySetup
    if (-not $setup) {
        [Console]::Error.WriteLine('goway-setup was not found in a protected location (under Program Files, or in the administrator-only directory under ProgramData that a host install creates). An administrator session never runs a copy that a normal user could have replaced. Run the command by hand in an administrator PowerShell from the installer you downloaded')
        exit 4
    }
    & $setup @SetupArgs
}
