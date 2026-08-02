# Dump the C# reference's view of a sample of .gxt2 text-table files, for
# cross-checking the Rust GXT2 port entry by entry.
#
# Same trick as rpf-oracle.ps1: CodeWalker.Core.dll is netstandard2.0, so
# Windows PowerShell 5.1 loads it and calls the real Gxt2File reader. Expected
# values must come from here, never from reading this port's own output back.
#
#   pwsh -File scripts/gxt2-oracle.ps1
#   cargo test -p cw-core --test gxt2_gate -- --ignored --nocapture
#
# The output lands in fixtures/, which is gitignored: it is metadata derived
# from a retail install and is not redistributable.

param(
  [string]$Gta = $(if ($env:ASTRAMAPPER_GTA_PATH) { $env:ASTRAMAPPER_GTA_PATH }
                   else { "E:\SteamLibrary\steamapps\common\Grand Theft Auto V" }),
  [string]$Dll = "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll",
  [string]$Out = "fixtures/gxt2_oracle.json",
  [string[]]$Archives = @("x64a.rpf", "x64b.rpf"),
  [int]$Limit = 3
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
  if ($null -ne $root.LastException) { throw ("$a : $($root.LastError)") }
  foreach ($rpf in Get-AllRpfs $root) {
    foreach ($e in $rpf.AllEntries) {
      if ($files.Count -ge $Limit) { break }
      if (-not $e.Name.ToLower().EndsWith(".gxt2")) { continue }
      $data = $rpf.ExtractFile($e)
      if ($null -eq $data) { continue }
      try {
        $g = New-Object CodeWalker.GameFiles.Gxt2File
        $g.Load($data, $e)
        $entries = @()
        foreach ($en in $g.TextEntries) {
          $entries += [ordered]@{ hash = [uint32]$en.Hash; text = [string]$en.Text }
        }
        $files += [ordered]@{
          path = $e.Path
          entryCount = [int]$g.EntryCount
          entries = $entries
        }
      } catch {
        Write-Host "  skipping $($e.Path): $_"
      }
    }
    if ($files.Count -ge $Limit) { break }
  }
  if ($files.Count -ge $Limit) { break }
}
[ordered]@{ files = $files } | ConvertTo-Json -Depth 20 -Compress | Out-File -FilePath $Out -Encoding utf8
Write-Host "wrote $Out with $($files.Count) gxt2 files"
