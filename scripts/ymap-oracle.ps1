# Dump the C# reference's view of a sample of .ymap files, for cross-checking
# the Rust YmapFile port entity by entity.
#
# Same trick as the other oracles: CodeWalker.Core.dll is netstandard2.0, so
# PowerShell loads it and calls the real YmapFile reader.
#
#   pwsh -File scripts/ymap-oracle.ps1
#   cargo test -p cw-core --test ymap_gate -- --ignored --nocapture
#
# The output lands in fixtures/, which is gitignored.

param(
  [string]$Gta = $(if ($env:ASTRAMAPPER_GTA_PATH) { $env:ASTRAMAPPER_GTA_PATH }
                   else { "E:\SteamLibrary\steamapps\common\Grand Theft Auto V" }),
  [string]$Dll = "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll",
  [string]$Out = "fixtures/ymap_oracle.json",
  [string[]]$Archives = @("update\update.rpf", "x64a.rpf", "x64f.rpf"),
  [int]$Limit = 15
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
      if ([System.IO.Path]::GetExtension($e.Name).ToLower() -ne ".ymap") { continue }

      $data = $rpf.ExtractFile($e)
      if ($null -eq $data) { continue }

      try {
        $f = New-Object CodeWalker.GameFiles.YmapFile($e)
        $f.Load($data, $e)
      } catch {
        Write-Host "  skipping $($e.Path): $_"
        continue
      }
      if ($null -eq $f.AllEntities) { continue }

      $entIndex = @{}
      for ($i = 0; $i -lt $f.AllEntities.Length; $i++) {
        $entIndex[$f.AllEntities[$i]] = $i
      }
      $ents = @()
      foreach ($ent in $f.AllEntities) {
        $childIdx = @()
        if ($null -ne $ent.Children) {
          foreach ($c in $ent.Children) { $childIdx += [int]$entIndex[$c] }
        }
        $ents += [ordered]@{
          archetypeName = [uint32]$ent._CEntityDef.archetypeName.Hash
          flags         = [uint32]$ent._CEntityDef.flags
          parentIndex   = [int]$ent._CEntityDef.parentIndex
          isMlo         = $ent.IsMlo
          position      = @([float]$ent._CEntityDef.position.X,
                            [float]$ent._CEntityDef.position.Y,
                            [float]$ent._CEntityDef.position.Z)
          children      = $childIdx
        }
      }

      $rootIdx = @()
      foreach ($r in $f.RootEntities) { $rootIdx += [int]$entIndex[$r] }
      $mloIdx = @()
      if ($null -ne $f.MloEntities) {
        foreach ($m in $f.MloEntities) { $mloIdx += [int]$entIndex[$m] }
      }

      $files += [ordered]@{
        path         = $e.Path
        systemSize   = [int]$e.SystemSize
        name         = [uint32]$f._CMapData.name.Hash
        parent       = [uint32]$f._CMapData.parent.Hash
        flags        = [uint32]$f._CMapData.flags
        entities     = $ents
        rootEntities = $rootIdx
        mloEntities  = $mloIdx
      }
      Write-Host "  $($e.Path): $($ents.Count) entities"
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
Write-Host "wrote $Out with $($files.Count) .ymap files"
