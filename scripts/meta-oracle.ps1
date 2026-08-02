# Dump the C# reference's view of a sample of `.ymt` files, for cross-checking
# the Rust metadata port node by node.
#
# The reference has no tree API: `MetaXml.GetXml` writes XML text. So this loads
# each file, asks for that XML, parses it back with XmlDocument, and flattens it
# into the leaves in document order. The Rust gate flattens its own tree the
# same way and compares, which is a comparison of what was *read* rather than of
# how it was printed.
#
#   pwsh -File scripts/meta-oracle.ps1
#   cargo test -p cw-core --test meta_gate -- --ignored --nocapture
#
# The output lands in fixtures/, which is gitignored: it is metadata derived
# from a retail install and is not redistributable.

param(
  [string]$Gta = $(if ($env:ASTRAMAPPER_GTA_PATH) { $env:ASTRAMAPPER_GTA_PATH }
                   else { "E:\SteamLibrary\steamapps\common\Grand Theft Auto V" }),
  [string]$Dll = "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll",
  [string]$Out = "fixtures/meta_oracle.json",
  [string[]]$Archives = @("common.rpf", "x64a.rpf", "x64b.rpf", "x64c.rpf", "x64e.rpf",
                          "x64f.rpf", "x64g.rpf", "update/update.rpf"),
  # Per container format, so the three walkers are all covered. A `.ymt` is RSC,
  # PSO or RBF and the install is mostly the first two.
  [int]$PerFormat = 25,
  # Per root structure, so a directory full of near-identical files does not
  # spend the whole budget on one shape.
  [int]$PerRoot = 4,
  # Leaves per file. A scenario region runs to hundreds of thousands of nodes
  # and the comparison is just as strong on a prefix.
  [int]$MaxLeaves = 4000
)

$ErrorActionPreference = "Stop"

Add-Type -Path $Dll
[CodeWalker.GameFiles.GTA5Keys]::LoadFromPath($Gta, $false, $null)

function Get-AllRpfs($rpf) {
  $list = @($rpf)
  foreach ($c in $rpf.Children) { $list += Get-AllRpfs $c }
  return $list
}

# Flatten an element tree into its leaves: name plus whatever the writer put
# where a value goes. `<error>` elements are the reference's own diagnostics and
# are skipped on both sides.
function Get-Leaves($node, $leaves, $max) {
  foreach ($child in $node.ChildNodes) {
    if ($leaves.Count -ge $max) { return }
    if ($child.NodeType -ne [System.Xml.XmlNodeType]::Element) { continue }
    if ($child.Name -eq "error") { continue }

    $hasElementChild = $false
    foreach ($g in $child.ChildNodes) {
      if ($g.NodeType -eq [System.Xml.XmlNodeType]::Element) { $hasElementChild = $true; break }
    }

    if ($hasElementChild) {
      Get-Leaves $child $leaves $max
      continue
    }

    $value = $null
    if ($child.HasAttribute("value")) {
      $value = $child.GetAttribute("value")
    } elseif ($child.HasAttribute("x")) {
      $parts = @()
      foreach ($a in @("x", "y", "z", "w")) {
        if ($child.HasAttribute($a)) { $parts += $child.GetAttribute($a) }
      }
      $value = ($parts -join ", ")
    } else {
      $value = $child.InnerText
    }

    $leaves.Add([ordered]@{ name = $child.Name; value = $value }) | Out-Null
  }
}

$files = @()
$counts = @{ "RSC" = 0; "PSO" = 0; "RBF" = 0; "unknown" = 0 }
$roots = @{}
function Test-Full { return ($counts["RSC"] -ge $PerFormat) -and ($counts["PSO"] -ge $PerFormat) -and ($counts["RBF"] -ge $PerFormat) }

foreach ($a in $Archives) {
  $full = Join-Path $Gta $a
  if (-not (Test-Path $full)) { Write-Host "skipping missing $full"; continue }
  Write-Host "scanning $full"

  $root = New-Object CodeWalker.GameFiles.RpfFile($full, $a)
  $root.ScanStructure($null, $null)
  if ($null -ne $root.LastException) { throw "$a`: $($root.LastError)" }

  foreach ($rpf in Get-AllRpfs $root) {
    foreach ($e in $rpf.AllEntries) {
      if (Test-Full) { break }
      if ([System.IO.Path]::GetExtension($e.Name).ToLower() -ne ".ymt") { continue }

      $data = $rpf.ExtractFile($e)
      if ($null -eq $data) { continue }

      $isResource = ($e -is [CodeWalker.GameFiles.RpfResourceFileEntry])
      if ($isResource -and ($counts["RSC"] -ge $PerFormat)) { continue }

      try {
        $f = New-Object CodeWalker.GameFiles.YmtFile($e)
        $f.Load($data, $e)
        $filename = ""
        $xml = [CodeWalker.GameFiles.MetaXml]::GetXml($f, [ref]$filename)
      } catch {
        Write-Host "  skipping $($e.Path): $_"
        continue
      }
      if ([string]::IsNullOrWhiteSpace($xml)) { continue }

      $doc = New-Object System.Xml.XmlDocument
      try { $doc.LoadXml($xml) } catch { Write-Host "  bad xml for $($e.Path)"; continue }
      if ($null -eq $doc.DocumentElement) { continue }

      $leaves = New-Object System.Collections.ArrayList
      Get-Leaves $doc.DocumentElement $leaves $MaxLeaves

      $format = "unknown"
      if ($isResource) { $format = "RSC" }
      elseif ($null -ne $f.Pso) { $format = "PSO" }
      elseif ($null -ne $f.Rbf) { $format = "RBF" }
      if ($counts[$format] -ge $PerFormat) { continue }
      $rootName = $doc.DocumentElement.Name
      if (-not $roots.ContainsKey($rootName)) { $roots[$rootName] = 0 }
      if ($roots[$rootName] -ge $PerRoot) { continue }
      $roots[$rootName] += 1
      $counts[$format] += 1

      $files += [ordered]@{
        path       = $e.Path
        isResource = [bool]$isResource
        systemSize = [int]$(if ($isResource) { $e.SystemSize } else { 0 })
        format     = $format
        rootName   = $doc.DocumentElement.Name
        leafCount  = $leaves.Count
        truncated  = [bool]($leaves.Count -ge $MaxLeaves)
        leaves     = $leaves
      }
      Write-Host "  $($e.Path): $format, $($doc.DocumentElement.Name), $($leaves.Count) leaves"
    }
    if (Test-Full) { break }
  }
  if (Test-Full) { break }
}

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
$json = [ordered]@{ files = $files } | ConvertTo-Json -Depth 10 -Compress
[System.IO.File]::WriteAllText(
  [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Out)),
  $json,
  (New-Object System.Text.UTF8Encoding($false)))
Write-Host "wrote $Out with $($files.Count) metadata files"
