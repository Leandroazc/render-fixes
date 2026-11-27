# Fetches the AMD files FSR 3 frame generation needs (about 30 MB) from AMD's FidelityFX SDK on GitHub
# (GPUOpen-LibrariesAndSDKs/FidelityFX-SDK, MIT licence) into amd-sdk\FidelityFX here (git-ignored), where
# scripts\play-foxhound.ps1 -FsrFrameGen looks for them. They are AMD's signed files; the signature is checked.
# Nothing of AMD's is needed to build the layer: the files are loaded when the game runs.
param(
    [string]$Tag = 'v2.4.0'
)
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$into = Join-Path $root 'amd-sdk\FidelityFX'
$bin = Join-Path $into 'bin'
New-Item -ItemType Directory -Force $bin | Out-Null
$base = "https://raw.githubusercontent.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/$Tag/Kits/FidelityFX"
$files = @(
    @{ From = 'signedbin/amd_fidelityfx_loader_dx12.dll'; To = 'bin\amd_fidelityfx_loader_dx12.dll' },
    @{ From = 'signedbin/amd_fidelityfx_framegeneration_dx12.dll'; To = 'bin\amd_fidelityfx_framegeneration_dx12.dll' },
    @{ From = 'docs/license.md'; To = 'license.md' }
)
Write-Host "AMD FidelityFX SDK $Tag frame generation, from github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK:"
foreach ($f in $files) {
    $to = Join-Path $into $f.To
    if (Test-Path $to) { Write-Host "  $($f.To) is there already"; continue }
    Invoke-WebRequest -Uri "$base/$($f.From)" -OutFile "$to.part" -UseBasicParsing -ErrorAction Stop
    Move-Item "$to.part" $to -Force
    Write-Host ("  {0}  {1:N1} MB" -f $f.To, ((Get-Item $to).Length / 1MB))
}
foreach ($dll in Get-ChildItem $bin -Filter '*.dll') {
    $s = Get-AuthenticodeSignature $dll.FullName
    if ($s.Status -ne 'Valid' -or $s.SignerCertificate.Subject -notmatch 'O=Advanced Micro Devices') {
        Remove-Item $dll.FullName -Force
        throw "$($dll.Name) is not signed by AMD ($($s.Status)); removed."
    }
}
Write-Host "  signed by AMD; licence: $(Join-Path $into 'license.md')"
Write-Host ''
Write-Host 'Done. scripts\play-foxhound.ps1 -FsrFrameGen uses them (on by itself on cards without DLSS frame generation).'
