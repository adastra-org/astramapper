# Dump the C# reference's view of a sample of .yft fragment files, for
# cross-checking the Rust fragment port block by block.
#
# Same trick as bounds-oracle.ps1: CodeWalker.Core.dll is netstandard2.0, so
# Windows PowerShell 5.1 loads it and calls the real YftFile reader. Expected
# values must come from here, never from reading this port's own output back.
#
#   pwsh -File scripts/frag-oracle.ps1
#   cargo test -p cw-core --test frag_gate -- --ignored --nocapture
#
# The output lands in fixtures/, which is gitignored: it is metadata derived
# from a retail install and is not redistributable.

param(
  [string]$Gta = $(if ($env:ASTRAMAPPER_GTA_PATH) { $env:ASTRAMAPPER_GTA_PATH }
                   else { "E:\SteamLibrary\steamapps\common\Grand Theft Auto V" }),
  [string]$Dll = "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll",
  [string]$Out = "fixtures/frag_oracle.json",
  [string[]]$Archives = @("x64a.rpf", "x64b.rpf", "common.rpf"),
  [int]$Limit = 12
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
  if ($null -eq $d) { return $null }
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
            declFlags    = if ($null -ne $g.VertexBuffer -and $null -ne $g.VertexBuffer.Info) { [uint32]$g.VertexBuffer.Info.Flags } else { 0 }
            declTypes    = if ($null -ne $g.VertexBuffer -and $null -ne $g.VertexBuffer.Info) { [uint64]$g.VertexBuffer.Info.Types } else { 0 }
            declStride   = if ($null -ne $g.VertexBuffer -and $null -ne $g.VertexBuffer.Info) { [int]$g.VertexBuffer.Info.Stride } else { 0 }
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
    fragMatrix = @(
      [float]$d.FragMatrix.Column1.X, [float]$d.FragMatrix.Column1.Y, [float]$d.FragMatrix.Column1.Z, [uint32]$d.FragMatrix.Flags1,
      [float]$d.FragMatrix.Column2.X, [float]$d.FragMatrix.Column2.Y, [float]$d.FragMatrix.Column2.Z, [uint32]$d.FragMatrix.Flags2,
      [float]$d.FragMatrix.Column3.X, [float]$d.FragMatrix.Column3.Y, [float]$d.FragMatrix.Column3.Z, [uint32]$d.FragMatrix.Flags3,
      [float]$d.FragMatrix.Column4.X, [float]$d.FragMatrix.Column4.Y, [float]$d.FragMatrix.Column4.Z, [uint32]$d.FragMatrix.Flags4
    )
    shaders    = $shaders
    models     = $models
  }
}

function Read-FragDrawable($d) {
  if ($null -eq $d) { return $null }
  return [ordered]@{
    drawable = Read-Drawable $d
    boundType = if ($null -ne $d.Bound) { "$($d.Bound.Type)" } else { $null }
  }
}

function Read-Lod($lod) {
  if ($null -eq $lod) { return $null }
  $children = @()
  if ($null -ne $lod.Children -and $null -ne $lod.Children.data_items) {
    foreach ($c in $lod.Children.data_items) {
      $children += [ordered]@{
        pristineMass = [float]$c.PristineMass
        damagedMass  = [float]$c.DamagedMass
        groupIndex   = [uint16]$c.GroupIndex
        boneTag      = [uint16]$c.BoneTag
        drawable1    = Read-FragDrawable $c.Drawable1
        drawable2    = Read-FragDrawable $c.Drawable2
      }
    }
  }
  $groups = @()
  if ($null -ne $lod.Groups -and $null -ne $lod.Groups.data_items) {
    foreach ($g in $lod.Groups.data_items) {
      $groups += [ordered]@{
        strength = [float]$g.Strength
        childGroupIndex = [byte]$g.ChildGroupIndex
        glassWindowIndex = [byte]$g.GlassWindowIndex
        name = "$($g.Name)"
      }
    }
  }
  return [ordered]@{
    children = $children
    groups   = $groups
    boundType = if ($null -ne $lod.Bound) { "$($lod.Bound.Type)" } else { $null }
  }
}

function Read-Fragment($f) {
  if ($null -eq $f) { return $null }
  $arr = @()
  if ($null -ne $f.DrawableArray) {
    foreach ($d in $f.DrawableArray.data_items) { $arr += Read-FragDrawable $d }
  }
  $arrnames = @()
  if ($null -ne $f.DrawableArrayNames) {
    foreach ($n in $f.DrawableArrayNames.data_items) { $arrnames += [string]$n.Value }
  }
  $lodg = $null
  if ($null -ne $f.PhysicsLODGroup) {
    $lodg = [ordered]@{
      lod1 = Read-Lod $f.PhysicsLODGroup.PhysicsLOD1
      lod2 = Read-Lod $f.PhysicsLODGroup.PhysicsLOD2
      lod3 = Read-Lod $f.PhysicsLODGroup.PhysicsLOD3
    }
  }
  return [ordered]@{
    name         = $f.Name
    radius       = [float]$f.BoundingSphereRadius
    center       = @([float]$f.BoundingSphereCenter.X, [float]$f.BoundingSphereCenter.Y, [float]$f.BoundingSphereCenter.Z)
    drawable     = Read-FragDrawable $f.Drawable
    drawableCloth = Read-FragDrawable $f.DrawableCloth
    drawableArray = $arr
    drawableArrayNames = $arrnames
    physicsLODGroup = $lodg
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
      if ([System.IO.Path]::GetExtension($e.Name).ToLower() -ne ".yft") { continue }

      $data = $rpf.ExtractFile($e)
      if ($null -eq $data) { continue }

      try {
        $f = New-Object CodeWalker.GameFiles.YftFile
        $f.Load($data, $e)
        $frag = Read-Fragment $f.Fragment
        if ($null -eq $frag) { continue }
      } catch {
        Write-Host "  skipping $($e.Path): $_"
        continue
      }

      $files += [ordered]@{
        path        = $e.Path
        systemSize  = [int]$e.SystemSize
        graphicsSize = [int]$e.GraphicsSize
        version     = [int]$e.Version
        fragment    = $frag
      }
    }
    if ($files.Count -ge $Limit) { break }
  }
  if ($files.Count -ge $Limit) { break }
}

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
[ordered]@{ files = $files } | ConvertTo-Json -Depth 40 -Compress | Out-File -FilePath $Out -Encoding utf8
Write-Host "wrote $Out with $($files.Count) yft files"
