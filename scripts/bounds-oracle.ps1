# Dump the C# reference's view of a sample of .ybn collision files, for
# cross-checking the Rust bounds port block by block.
#
# Same trick as drawable-oracle.ps1: CodeWalker.Core.dll is netstandard2.0, so
# Windows PowerShell 5.1 loads it and calls the real YbnFile reader. Expected
# values must come from here, never from reading this port's own output back.
#
#   pwsh -File scripts/bounds-oracle.ps1
#   cargo test -p cw-core --test bounds_gate -- --ignored --nocapture
#
# The output lands in fixtures/, which is gitignored: it is metadata derived
# from a retail install and is not redistributable.

param(
  [string]$Gta = $(if ($env:ASTRAMAPPER_GTA_PATH) { $env:ASTRAMAPPER_GTA_PATH }
                   else { "E:\SteamLibrary\steamapps\common\Grand Theft Auto V" }),
  [string]$Dll = "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll",
  [string]$Out = "fixtures/bounds_oracle.json",
  [string[]]$Archives = @("common.rpf", "x64a.rpf", "x64b.rpf", "x64c.rpf"),
  [int]$Limit = 15
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

function Read-Polygon($p) {
  if ($null -eq $p) { return $null }
  switch ($p.Type) {
    "Triangle" {
      return [ordered]@{
        t = "Triangle"
        area = [float]$p.triArea
        i1 = [uint16]$p.triIndex1
        i2 = [uint16]$p.triIndex2
        i3 = [uint16]$p.triIndex3
        e1 = [uint16]$p.edgeIndex1
        e2 = [uint16]$p.edgeIndex2
        e3 = [uint16]$p.edgeIndex3
      }
    }
    "Sphere" {
      return [ordered]@{
        t = "Sphere"
        idx = [uint16]$p.sphereIndex
        radius = [float]$p.sphereRadius
      }
    }
    "Capsule" {
      return [ordered]@{
        t = "Capsule"
        i1 = [uint16]$p.capsuleIndex1
        radius = [float]$p.capsuleRadius
        i2 = [uint16]$p.capsuleIndex2
      }
    }
    "Box" {
      return [ordered]@{
        t = "Box"
        i1 = [int16]$p.boxIndex1
        i2 = [int16]$p.boxIndex2
        i3 = [int16]$p.boxIndex3
        i4 = [int16]$p.boxIndex4
      }
    }
    "Cylinder" {
      return [ordered]@{
        t = "Cylinder"
        i1 = [uint16]$p.cylinderIndex1
        radius = [float]$p.cylinderRadius
        i2 = [uint16]$p.cylinderIndex2
      }
    }
  }
  return [ordered]@{ t = "$($p.Type)"; raw = "$($p.GetType().Name)" }
}

function Read-Matrix($m) {
  if ($null -eq $m) { return $null }
  return @(
    [float]$m.Column1.X, [float]$m.Column1.Y, [float]$m.Column1.Z, [uint32]$m.Flags1,
    [float]$m.Column2.X, [float]$m.Column2.Y, [float]$m.Column2.Z, [uint32]$m.Flags2,
    [float]$m.Column3.X, [float]$m.Column3.Y, [float]$m.Column3.Z, [uint32]$m.Flags3,
    [float]$m.Column4.X, [float]$m.Column4.Y, [float]$m.Column4.Z, [uint32]$m.Flags4
  )
}

function Read-Aabb($a) {
  if ($null -eq $a) { return $null }
  return @(
    [float]$a.Min.X, [float]$a.Min.Y, [float]$a.Min.Z, [float]$a.Min.W,
    [float]$a.Max.X, [float]$a.Max.Y, [float]$a.Max.Z, [float]$a.Max.W
  )
}

function Read-BVH($bvh) {
  if ($null -eq $bvh) { return $null }
  $nodes = @()
  if ($null -ne $bvh.Nodes -and $null -ne $bvh.Nodes.data_items) {
    foreach ($n in $bvh.Nodes.data_items) {
      $nodes += [ordered]@{
        mn = @([int16]$n.MinX, [int16]$n.MinY, [int16]$n.MinZ)
        mx = @([int16]$n.MaxX, [int16]$n.MaxY, [int16]$n.MaxZ)
        itemId = [int16]$n.ItemId
        itemCount = [int16]$n.ItemCount
      }
    }
  }
  $trees = @()
  if ($null -ne $bvh.Trees -and $null -ne $bvh.Trees.data_items) {
    foreach ($t in $bvh.Trees.data_items) {
      $trees += [ordered]@{
        mn = @([int16]$t.MinX, [int16]$t.MinY, [int16]$t.MinZ)
        mx = @([int16]$t.MaxX, [int16]$t.MaxY, [int16]$t.MaxZ)
        n1 = [int16]$t.NodeIndex1
        n2 = [int16]$t.NodeIndex2
      }
    }
  }
  return [ordered]@{
    nodes = $nodes
    trees = $trees
    bbMin = @([float]$bvh.BoundingBoxMin.X, [float]$bvh.BoundingBoxMin.Y, [float]$bvh.BoundingBoxMin.Z)
    bbMax = @([float]$bvh.BoundingBoxMax.X, [float]$bvh.BoundingBoxMax.Y, [float]$bvh.BoundingBoxMax.Z)
  }
}

function Read-Bounds($b, $depth) {
  if ($null -eq $b) { return $null }

  $common = [ordered]@{
    type = "$($b.Type)"
    sphereRadius = [float]$b.SphereRadius
    margin = [float]$b.Margin
    volume = [float]$b.Volume
    boxMin = @([float]$b.BoxMin.X, [float]$b.BoxMin.Y, [float]$b.BoxMin.Z)
    boxMax = @([float]$b.BoxMax.X, [float]$b.BoxMax.Y, [float]$b.BoxMax.Z)
    boxCenter = @([float]$b.BoxCenter.X, [float]$b.BoxCenter.Y, [float]$b.BoxCenter.Z)
    sphereCenter = @([float]$b.SphereCenter.X, [float]$b.SphereCenter.Y, [float]$b.SphereCenter.Z)
    materialIndex = [int]$b.MaterialIndex
    proceduralId = [int]$b.ProceduralId
    roomId = [int]$b.RoomId
    pedDensity = [int]$b.PedDensity
    unkFlags = [int]$b.UnkFlags
    polyFlags = [int]$b.PolyFlags
    unkType = [int]$b.Unknown_3Ch
    inertia = @([float]$b.Unknown_60h.X, [float]$b.Unknown_60h.Y, [float]$b.Unknown_60h.Z)
  }

  if ($b -is [CodeWalker.GameFiles.BoundGeometry]) {
    $g = [CodeWalker.GameFiles.BoundGeometry]$b
    $verts = @()
    if ($null -ne $g.Vertices) {
      foreach ($v in $g.Vertices) {
        $verts += @([float]$v.X, [float]$v.Y, [float]$v.Z)
      }
    }
    $polys = @()
    if ($null -ne $g.Polygons) {
      foreach ($p in $g.Polygons) { $polys += Read-Polygon $p }
    }
    $mats = @()
    if ($null -ne $g.Materials) {
      foreach ($m in $g.Materials) { $mats += @([uint32]$m.Data1, [uint32]$m.Data2) }
    }
    $polymats = @()
    if ($null -ne $g.PolygonMaterialIndices) {
      foreach ($mi in $g.PolygonMaterialIndices) { $polymats += [int]$mi }
    }
    $vcols = @()
    if ($null -ne $g.VertexColours) {
      foreach ($vc in $g.VertexColours) { $vcols += @([int]$vc.R, [int]$vc.G, [int]$vc.B, [int]$vc.A) }
    }
    $octs = @()
    if ($null -ne $g.Octants) { foreach ($c in $g.Octants.Counts) { $octs += [uint32]$c } }

    $common["verticesCount"] = [int]$g.VerticesCount
    $common["polygonsCount"] = [int]$g.PolygonsCount
    $common["quantum"] = @([float]$g.Quantum.X, [float]$g.Quantum.Y, [float]$g.Quantum.Z)
    $common["centerGeom"] = @([float]$g.CenterGeom.X, [float]$g.CenterGeom.Y, [float]$g.CenterGeom.Z)
    $common["vertices"] = $verts
    $common["vertexColours"] = $vcols
    $common["polygons"] = $polys
    $common["materials"] = $mats
    $common["polygonMaterialIndices"] = $polymats
    $common["octantCounts"] = $octs
  }

  if ($b -is [CodeWalker.GameFiles.BoundBVH]) {
    $bv = [CodeWalker.GameFiles.BoundBVH]$b
    $common["bvh"] = Read-BVH $bv.BVH
  }

  if ($b -is [CodeWalker.GameFiles.BoundComposite]) {
    $c = [CodeWalker.GameFiles.BoundComposite]$b
    $kids = @()
    if ($null -ne $c.Children) {
      foreach ($k in $c.Children.data_items) { $kids += Read-Bounds $k ($depth + 1) }
    }
    $t1 = @()
    if ($null -ne $c.ChildrenTransformation1) { foreach ($m in $c.ChildrenTransformation1) { $t1 += Read-Matrix $m } }
    $t2 = @()
    if ($null -ne $c.ChildrenTransformation2) { foreach ($m in $c.ChildrenTransformation2) { $t2 += Read-Matrix $m } }
    $bbs = @()
    if ($null -ne $c.ChildrenBoundingBoxes) { foreach ($a in $c.ChildrenBoundingBoxes) { $bbs += Read-Aabb $a } }
    $f1 = @()
    if ($null -ne $c.ChildrenFlags1) { foreach ($f in $c.ChildrenFlags1) { $f1 += @([uint32]$f.Flags1, [uint32]$f.Flags2) } }
    $f2 = @()
    if ($null -ne $c.ChildrenFlags2) { foreach ($f in $c.ChildrenFlags2) { $f2 += @([uint32]$f.Flags1, [uint32]$f.Flags2) } }
    $common["children"] = $kids
    $common["childrenTransformation1"] = $t1
    $common["childrenTransformation2"] = $t2
    $common["childrenBoundingBoxes"] = $bbs
    $common["childrenFlags1"] = $f1
    $common["childrenFlags2"] = $f2
    $common["bvh"] = Read-BVH $c.BVH
  }

  return $common
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
      if ([System.IO.Path]::GetExtension($e.Name).ToLower() -ne ".ybn") { continue }

      $data = $rpf.ExtractFile($e)
      if ($null -eq $data) { continue }

      try {
        $f = New-Object CodeWalker.GameFiles.YbnFile
        $f.Load($data, $e)
        if ($null -eq $f.Bounds) { continue }
        $bounds = Read-Bounds $f.Bounds 0
      } catch {
        Write-Host "  skipping $($e.Path): $_"
        continue
      }

      $files += [ordered]@{
        path = $e.Path
        systemSize = [int]$e.SystemSize
        graphicsSize = [int]$e.GraphicsSize
        version = [int]$e.Version
        bounds = $bounds
      }
    }
    if ($files.Count -ge $Limit) { break }
  }
  if ($files.Count -ge $Limit) { break }
}

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
[ordered]@{ files = $files } | ConvertTo-Json -Depth 40 -Compress | Out-File -FilePath $Out -Encoding utf8
Write-Host "wrote $Out with $($files.Count) ybn files"
