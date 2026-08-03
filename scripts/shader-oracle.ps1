# Dump the C# reference's view of a sample of .ydr/.ydd drawable shader
# parameters and embedded texture dictionaries, for cross-checking the Rust
# texture-on-model port.
#
#   pwsh -File scripts/shader-oracle.ps1
#
# The output lands in fixtures/, which is gitignored.

param(
  [string]$Gta = $(if ($env:ASTRAMAPPER_GTA_PATH) { $env:ASTRAMAPPER_GTA_PATH }
                   else { "E:\SteamLibrary\steamapps\common\Grand Theft Auto V" }),
  [string]$Dll = "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll",
  [string]$Out = "fixtures/shader_oracle.json",
  [string[]]$Archives = @("x64a.rpf", "common.rpf", "x64c.rpf", "x64d.rpf"),
  # Model files that carry an embedded texture dictionary, read directly by
  # path so the embedded-dictionary half of the gate is exercised. Stock files
  # usually keep their pixels in sibling `.ytd` files; these are the exception.
  [string[]]$Extra = @(
    "update\update.rpf\dlc_patch\mp2024_02\x64\levels\gta5\props\prop_m24_2_barge_01.rpf\m24_2_prop_m42_mil_barge_01a.ydr",
    "update\update.rpf\dlc_patch\mp2024_02\x64\levels\gta5\props\prop_m24_2_crates_01.rpf\m24_2_prop_m42_armorcrate_01a.ydr",
    "update\update.rpf\dlc_patch\mp2024_02\x64\levels\gta5\props\prop_m24_2_crates_01.rpf\m24_2_prop_m42_container_01a.ydr"
  ),
  [int]$Limit = 12
)

# The `Archives` scan fills up to `Limit`; the `Extra` paths are appended after,
# so the sample can reach `Limit` archive files plus the extras. The gate reads
# whichever are present.

$ErrorActionPreference = "Stop"

Add-Type -Path $Dll
[CodeWalker.GameFiles.GTA5Keys]::LoadFromPath($Gta, $false, $null)

function Get-AllRpfs($rpf) {
  $list = @($rpf)
  foreach ($c in $rpf.Children) { $list += Get-AllRpfs $c }
  return $list
}

# The same load+dump used for archive entries, parameterised over the extension
# so the Extra paths below can reuse it. `$e` is the RpfResourceFileEntry that
# `Load` needs to set up the file's segments.
function Read-DrawablesFor($ext, $data, $e) {
  $drawables = @()
  if ($ext -eq ".ydr") {
    $f = New-Object CodeWalker.GameFiles.YdrFile
    $f.Load($data, $e)
    if ($null -ne $f.Drawable) { $drawables += Read-Drawable $f.Drawable }
  } else {
    $f = New-Object CodeWalker.GameFiles.YddFile
    $f.Load($data, $e)
    if ($null -ne $f.DrawableDict) {
      foreach ($d in $f.Drawables) { $drawables += Read-Drawable $d }
    }
  }
  return $drawables
}

# Walk every archive under `root` (nested included) looking for an entry by its
# full path. Returns @($rpf, $entry), or $null.
function Find-Entry($root, $path) {
  foreach ($rpf in Get-AllRpfs $root) {
    foreach ($e in $rpf.AllEntries) {
      if ($e.Path -eq $path) { return @($rpf, $e) }
    }
  }
  return $null
}

function Read-Shader($s) {
  $params = @()
  if ($null -ne $s.ParametersList -and $null -ne $s.ParametersList.Parameters) {
    $pl = $s.ParametersList.Parameters
    $hl = $s.ParametersList.Hashes
    for ($i = 0; $i -lt $pl.Length; $i++) {
      $p = $pl[$i]
      $name = if ($i -lt $hl.Length) { [uint32]$hl[$i] } else { 0 }
      $texhash = 0
      if ($p.Data -is [CodeWalker.GameFiles.TextureBase]) {
        $texhash = [uint32]$p.Data.NameHash
      }
      $params += [ordered]@{
        name = $name
        dataType = [int]$p.DataType
        texture = $texhash
      }
    }
  }
  return [ordered]@{
    name = [uint32]$s.Name.Hash
    fileName = [uint32]$s.FileName.Hash
    params = $params
  }
}

function Read-Drawable($d) {
  $shaders = @()
  if ($null -ne $d.ShaderGroup -and $null -ne $d.ShaderGroup.Shaders) {
    foreach ($s in $d.ShaderGroup.Shaders.data_items) {
      if ($null -ne $s) { $shaders += Read-Shader $s }
    }
  }
  $texdict = $null
  $sg = $d.ShaderGroup
  if (($null -ne $sg) -and ($null -ne $sg.TextureDictionary)) {
    $texnames = @()
    $td = $sg.TextureDictionary
    $texes = $td.Textures.data_items
    $hashes = $td.TextureNameHashes.data_items
    for ($i = 0; $i -lt $texes.Length; $i++) {
      $t = $texes[$i]
      $h = if ($i -lt $hashes.Length) { [uint32]$hashes[$i] } else { 0 }
      $texnames += [ordered]@{ hash = $h; name = $t.Name }
    }
    $texdict = [ordered]@{
      count = [int]$td.Textures.Count
      textures = $texnames
    }
  }
  return [ordered]@{
    shaders = $shaders
    textureDictionary = $texdict
  }
}

$files = @()
# Top-level archive name to its scanned root, shared by the archive loop below
# and the Extra loop that follows.
$rootIndex = @{}

foreach ($a in $Archives) {
  $full = Join-Path $Gta $a
  if (-not (Test-Path $full)) { Write-Host "skipping missing $full"; continue }
  Write-Host "scanning $full"

  $root = New-Object CodeWalker.GameFiles.RpfFile($full, $a)
  $root.ScanStructure($null, $null)
  if ($null -ne $root.LastException) { throw "$a : $($root.LastError)" }
  $rootIndex[$a] = $root

  foreach ($rpf in Get-AllRpfs $root) {
    foreach ($e in $rpf.AllEntries) {
      if ($files.Count -ge $Limit) { break }
      if (-not ($e -is [CodeWalker.GameFiles.RpfResourceFileEntry])) { continue }
      $ext = [System.IO.Path]::GetExtension($e.Name).ToLower()
      if ($ext -ne ".ydr" -and $ext -ne ".ydd") { continue }

      $data = $rpf.ExtractFile($e)
      if ($null -eq $data) { continue }

      try {
        $drawables = @(Read-DrawablesFor $ext $data $e)
      } catch {
        Write-Host "  skipping $($e.Path): $_"
        continue
      }

      $files += [ordered]@{
        path = $e.Path
        systemSize = [int]$e.SystemSize
        graphicsSize = [int]$e.GraphicsSize
        drawables = $drawables
      }
    }
    if ($files.Count -ge $Limit) { break }
  }
  if ($files.Count -ge $Limit) { break }
}

foreach ($extra in $Extra) {
  # The extras are chosen by hand to exercise the embedded-dictionary path, so
  # they are appended regardless of `$Limit` — the limit bounds the random
  # archive scan, not the deliberate sample.
  # The extras live under update.rpf, which the archive scan above may not
  # have opened. Open the top-level archive that owns the path on demand: the
  # path prefix up to and including the first `.rpf` segment, e.g.
  # `update\update.rpf`.
  $segments = $extra -split "\\"
  $stop = ($segments | Where-Object { $_ -like "*.rpf" } | Select-Object -First 1)
  if ($null -eq $stop) { Write-Host "  skipping (no owner archive) $extra"; continue }
  $owner = ($segments[0..([array]::IndexOf($segments, $stop))]) -join "\"
  if (-not $rootIndex.ContainsKey($owner)) {
    $full = Join-Path $Gta $owner
    if (Test-Path $full) {
      $r = New-Object CodeWalker.GameFiles.RpfFile($full, $owner)
      $r.ScanStructure($null, $null)
      if ($null -eq $r.LastException) { $rootIndex[$owner] = $r }
    }
  }
  $root = $rootIndex[$owner]
  if ($null -eq $root) { Write-Host "  skipping (no archive) $extra"; continue }
  $entry = Find-Entry $root $extra
  if ($null -eq $entry) { Write-Host "  skipping (not found) $extra"; continue }
  $rpf = $entry[0]
  $e = $entry[1]
  $data = $rpf.ExtractFile($e)
  if ($null -eq $data) { Write-Host "  skipping (extract) $extra"; continue }
  try {
    $ext = [System.IO.Path]::GetExtension($e.Name).ToLower()
    $drawables = @(Read-DrawablesFor $ext $data $e)
    $files += [ordered]@{
      path = $e.Path
      systemSize = [int]$e.SystemSize
      graphicsSize = [int]$e.GraphicsSize
      drawables = $drawables
    }
    Write-Host "  read $extra"
  } catch {
    Write-Host "  skipping ${extra}: $_"
  }
}

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
[ordered]@{ files = $files } | ConvertTo-Json -Depth 30 -Compress | Out-File -FilePath $Out -Encoding utf8
Write-Host "wrote $Out with $($files.Count) drawable files"
