# Dump the C# reference's view of a set of RPF archives, for cross-checking the
# Rust port entry by entry.
#
# CodeWalker.Core.dll is netstandard2.0, so Windows PowerShell 5.1 loads it and
# calls the real implementation. Expected values must come from here, never from
# reading this port's own output back.
#
#   pwsh -File scripts/rpf-oracle.ps1
#   cargo test -p cw-core --test rpf_gate -- --ignored --nocapture
#
# The output lands in fixtures/, which is gitignored: it is metadata derived
# from a retail install and is not redistributable. fixtures/manifest.json
# records what should be there.

param(
  [string]$Gta = $(if ($env:ASTRAMAPPER_GTA_PATH) { $env:ASTRAMAPPER_GTA_PATH }
                   else { "E:\SteamLibrary\steamapps\common\Grand Theft Auto V" }),
  [string]$Dll = "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll",
  [string]$Out = "fixtures/rpf_oracle.json",
  [string[]]$Archives = @("x64a.rpf", "common.rpf")
)

$ErrorActionPreference = "Stop"

Add-Type -Path $Dll
[CodeWalker.GameFiles.GTA5Keys]::LoadFromPath($Gta, $false, $null)

$result = @{}

foreach ($a in $Archives) {
  $full = Join-Path $Gta $a
  Write-Host "scanning $full"

  $rpf = New-Object CodeWalker.GameFiles.RpfFile($full, $a)
  $rpf.ScanStructure($null, $null)
  if ($rpf.LastException -ne $null) { throw "$a`: $($rpf.LastError)" }

  $sha = [System.Security.Cryptography.SHA256]::Create()
  $files = @()
  foreach ($e in $rpf.AllEntries) {
    $kind = "dir"
    if ($e -is [CodeWalker.GameFiles.RpfBinaryFileEntry])   { $kind = "bin" }
    if ($e -is [CodeWalker.GameFiles.RpfResourceFileEntry]) { $kind = "res" }
    $off = 0; $siz = 0; $hash = $null; $len = -1
    if ($e -is [CodeWalker.GameFiles.RpfFileEntry]) {
      $off = $e.FileOffset
      $siz = $e.GetFileSize()
      # Extracted bytes, not just the table metadata: this is what proves the
      # sector arithmetic, the decryption and the inflate all agree.
      $data = $rpf.ExtractFile($e)
      # $null on the left: `$data -ne $null` would compare element-wise over a
      # multi-megabyte byte array and exhaust memory.
      if ($null -ne $data) {
        $len  = $data.Length
        $hash = [System.BitConverter]::ToString($sha.ComputeHash($data)).Replace("-", "").ToLower()
      }
    }
    $files += [ordered]@{
      path = $e.Path; kind = $kind; offset = $off; size = $siz
      dataLength = $len; dataSha256 = $hash
    }
  }
  $sha.Dispose()

  $result[$a] = [ordered]@{
    entryCount  = $rpf.EntryCount
    namesLength = $rpf.NamesLength
    encryption  = [uint32]$rpf.Encryption
    folders     = $rpf.TotalFolderCount
    binaries    = $rpf.TotalBinaryFileCount
    resources   = $rpf.TotalResourceCount
    entries     = $files
  }
}

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
$result | ConvertTo-Json -Depth 6 -Compress | Out-File -FilePath $Out -Encoding utf8
Write-Host "wrote $Out"
