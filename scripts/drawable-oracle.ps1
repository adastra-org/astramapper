# Dump the C# reference's view of a sample of drawables, for cross-checking the
# Rust resource port geometry by geometry.
#
# Same trick as rpf-oracle.ps1: CodeWalker.Core.dll is netstandard2.0, so
# Windows PowerShell 5.1 loads it and calls the real YdrFile/YddFile readers.
# Expected values must come from here, never from reading this port's own output
# back.
#
#   pwsh -File scripts/drawable-oracle.ps1
#   cargo test -p cw-core --test drawable_gate -- --ignored --nocapture
#
# The output lands in fixtures/, which is gitignored: it is metadata derived
# from a retail install and is not redistributable.

param(
  [string]$Gta = $(if ($env:ASTRAMAPPER_GTA_PATH) { $env:ASTRAMAPPER_GTA_PATH }
                   else { "E:\SteamLibrary\steamapps\common\Grand Theft Auto V" }),
  [string]$Dll = "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll",
  [string]$Out = "fixtures/drawable_oracle.json",
  [string[]]$Archives = @("x64a.rpf", "common.rpf", "x64c.rpf"),
  [int]$Limit = 40
)

$ErrorActionPreference = "Stop"

Add-Type -Path $Dll
[CodeWalker.GameFiles.GTA5Keys]::LoadFromPath($Gta, $false, $null)

# Every archive in a scanned tree, the parent included.
function Get-AllRpfs($rpf) {
  $list = @($rpf)
  foreach ($c in $rpf.Children) { $list += Get-AllRpfs $c }
  return $list
}

function Read-Drawable($d) {
  $models = @()
  foreach ($lod in @("High", "Med", "Low", "VLow")) {
    $list = $d.DrawableModels.$lod
    if ($null -eq $list) { continue }
    foreach ($m in $list) {
      $geoms = @()
      if ($null -ne $m.Geometries) {
        foreach ($g in $m.Geometries) {
          if ($null -eq $g) { continue }
          $geoms += [ordered]@{
            shaderId     = [int]$g.ShaderID
            verticesCount = [int]$g.VerticesCount
            indicesCount = [int]$g.IndicesCount
            vertexStride = [int]$g.VertexStride
            declFlags    = if ($null -ne $g.VertexBuffer.Info) { [uint32]$g.VertexBuffer.Info.Flags } else { 0 }
            declTypes    = if ($null -ne $g.VertexBuffer.Info) { [uint64]$g.VertexBuffer.Info.Types } else { 0 }
            declStride   = if ($null -ne $g.VertexBuffer.Info) { [int]$g.VertexBuffer.Info.Stride } else { 0 }
          }
        }
      }
      $models += [ordered]@{
        lod        = $lod
        boneIndex  = [int]$m.BoneIndex
        hasSkin    = [int]$m.HasSkin
        geometries = $geoms
      }
    }
  }

  $shaders = @()
  if ($null -ne $d.ShaderGroup -and $null -ne $d.ShaderGroup.Shaders) {
    foreach ($s in $d.ShaderGroup.Shaders.data_items) {
      if ($null -eq $s) { continue }
      $shaders += [ordered]@{ name = [uint32]$s.Name.Hash; fileName = [uint32]$s.FileName.Hash }
    }
  }

  return [ordered]@{
    name       = $d.Name
    radius     = [float]$d.BoundingSphereRadius
    bbMin      = @([float]$d.BoundingBoxMin.X, [float]$d.BoundingBoxMin.Y, [float]$d.BoundingBoxMin.Z)
    bbMax      = @([float]$d.BoundingBoxMax.X, [float]$d.BoundingBoxMax.Y, [float]$d.BoundingBoxMax.Z)
    lodDist    = @([float]$d.LodDistHigh, [float]$d.LodDistMed, [float]$d.LodDistLow, [float]$d.LodDistVlow)
    shaders    = $shaders
    models     = $models
  }
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

      $ext = [System.IO.Path]::GetExtension($e.Name).ToLower()
      if ($ext -ne ".ydr" -and $ext -ne ".ydd") { continue }

      $data = $rpf.ExtractFile($e)
      if ($null -eq $data) { continue }

      try {
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
      } catch {
        Write-Host "  skipping $($e.Path): $_"
        continue
      }

      $files += [ordered]@{
        path        = $e.Path
        systemSize  = [int]$e.SystemSize
        graphicsSize = [int]$e.GraphicsSize
        version     = [int]$e.Version
        drawables   = $drawables
      }
    }
    if ($files.Count -ge $Limit) { break }
  }
  if ($files.Count -ge $Limit) { break }
}

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
[ordered]@{ files = $files } | ConvertTo-Json -Depth 10 -Compress | Out-File -FilePath $Out -Encoding utf8
Write-Host "wrote $Out with $($files.Count) drawable files"
