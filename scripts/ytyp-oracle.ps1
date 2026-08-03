# Dump the C# reference's view of a sample of .ytyp files, for cross-checking
# the Rust YtypFile port archetype by archetype.
#
# Same trick as the other oracles: CodeWalker.Core.dll is netstandard2.0, so
# PowerShell loads it and calls the real YtypFile reader.
#
#   pwsh -File scripts/ytyp-oracle.ps1
#   cargo test -p cw-core --test ymap_gate -- --ignored --nocapture
#
# The output lands in fixtures/, which is gitignored.

param(
  [string]$Gta = $(if ($env:ASTRAMAPPER_GTA_PATH) { $env:ASTRAMAPPER_GTA_PATH }
                   else { "E:\SteamLibrary\steamapps\common\Grand Theft Auto V" }),
  [string]$Dll = "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll",
  [string]$Out = "fixtures/ytyp_oracle.json",
  [string[]]$Archives = @("x64a.rpf", "x64f.rpf", "x64h.rpf", "x64j.rpf"),
  [int]$Limit = 30
)

$ErrorActionPreference = "Stop"

Add-Type -Path $Dll
[CodeWalker.GameFiles.GTA5Keys]::LoadFromPath($Gta, $false, $null)

function Get-AllRpfs($rpf) {
  $list = @($rpf)
  foreach ($c in $rpf.Children) { $list += Get-AllRpfs $c }
  return $list
}

$files = @()

foreach ($a in $Archives) {
  $full = Join-Path $Gta $a
  if (-not (Test-Path $full)) { Write-Host "skipping missing $full"; continue }
  Write-Host "scanning $full"

  $root = New-Object CodeWalker.GameFiles.RpfFile($full, $a)
  $root.ScanStructure($null, $null)
  if ($null -ne $root.LastException) { throw "$a`: $($root.LastError)" }

  foreach ($rpf in Get-AllRpfs $root) {
    foreach ($e in $rpf.AllEntries) {
      if ($files.Count -ge $Limit) { break }
      if (-not ($e -is [CodeWalker.GameFiles.RpfResourceFileEntry])) { continue }
      if ([System.IO.Path]::GetExtension($e.Name).ToLower() -ne ".ytyp") { continue }

      $data = $rpf.ExtractFile($e)
      if ($null -eq $data) { continue }

      try {
        $f = New-Object CodeWalker.GameFiles.YtypFile($e)
        $f.Load($data, $e)
      } catch {
        Write-Host "  skipping $($e.Path): $_"
        continue
      }
      if ($null -eq $f.AllArchetypes) { continue }

      $archs = @()
      foreach ($arch in $f.AllArchetypes) {
        $kind = "base"
        $rooms = @()
        if ($arch -is [CodeWalker.GameFiles.TimeArchetype]) { $kind = "time" }
        elseif ($arch -is [CodeWalker.GameFiles.MloArchetype]) {
          $kind = "mlo"
          $mlo = [CodeWalker.GameFiles.MloArchetype]$arch
          if ($null -ne $mlo.rooms) {
            foreach ($room in $mlo.rooms) {
              $rooms += [ordered]@{ name = $room.RoomName }
            }
          }
        }
        $archs += [ordered]@{
          kind      = $kind
          name      = [uint32]$arch._BaseArchetypeDef.name.Hash
          assetName = [uint32]$arch._BaseArchetypeDef.assetName.Hash
          hash      = [uint32]$arch.Hash.Hash
          assetType = [int]$arch._BaseArchetypeDef.assetType
          rooms     = $rooms
        }
      }

      $files += [ordered]@{
        path       = $e.Path
        systemSize = [int]$e.SystemSize
        nameHash   = [uint32]$f.NameHash
        archetypes = $archs
      }
      Write-Host "  $($e.Path): $($archs.Count) archetypes"
    }
    if ($files.Count -ge $Limit) { break }
  }
  if ($files.Count -ge $Limit) { break }
}

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
$json = [ordered]@{ files = $files } | ConvertTo-Json -Depth 12 -Compress
[System.IO.File]::WriteAllText(
  [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Out)),
  $json,
  (New-Object System.Text.UTF8Encoding($false)))
Write-Host "wrote $Out with $($files.Count) .ytyp files"
