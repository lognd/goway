# Print the CPU a Windows executable is built for (x64, arm64, or the raw
# machine code) by reading its PE header. Used to prove the ARM64 installer
# is a native aarch64 binary, not an x64 one running under emulation.
#   pe-machine.ps1 goway-setup-arm64.exe
param([Parameter(Mandatory = $true)][string]$Path)
$ErrorActionPreference = 'Stop'
$bytes = [System.IO.File]::ReadAllBytes((Resolve-Path $Path))
$pe = [BitConverter]::ToInt32($bytes, 0x3c)
$machine = [BitConverter]::ToUInt16($bytes, $pe + 4)
switch ($machine) {
    0x8664 { 'x64' }
    0xAA64 { 'arm64' }
    default { '0x{0:X}' -f $machine }
}
