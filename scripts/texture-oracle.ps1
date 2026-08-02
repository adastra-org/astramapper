# Dump the C# reference's view of a sample of texture dictionaries, for
# cross-checking the Rust texture port texture by texture — metadata, the DDS
# bytes it writes, and a sample of the pixels it decodes.
#
# Same trick as rpf-oracle.ps1: CodeWalker.Core.dll is netstandard2.0, so
# Windows PowerShell 5.1 loads it and calls the real YtdFile reader and DDSIO.
# Expected values must come from here, never from reading this port's own output
# back.
#
#   pwsh -File scripts/texture-oracle.ps1
#   cargo test -p cw-core --test texture_gate -- --ignored --nocapture
#
# Three things the gate needs and why they are shaped this way:
#
#   * `dataSha256` hashes `TextureData.FullData` — the payload as the reference
#     sizes it, which is `stride * height` quartered per level and *not* the
#     `ComputePitch` layout. Getting it exact proves the storage arithmetic.
#   * `ddsSha256` hashes `DDSIO.GetDDSFile`, which exercises the header, the
#     pixel-format table and the per-mip pitch in one value.
#   * `pixels` is a sample rather than the whole surface: a 2048-square texture
#     is 16 MB and the oracle covers 40 files. Positions are a fixed stride so
#     the gate can recompute them. Bytes are BGRA, which is what `GetPixels`
#     returns — it swaps red and blue for the WinForms bitmap it feeds.
#
# `GetPixels` has an empty `case DXGI_FORMAT_BC7_UNORM` and returns null, so BC7
# textures land here with `pixels = null`. There is no oracle for that format.
#
# The output lands in fixtures/, which is gitignored: it is metadata derived
# from a retail install and is not redistributable.

param(
  [string]$Gta = $(if ($env:ASTRAMAPPER_GTA_PATH) { $env:ASTRAMAPPER_GTA_PATH }
                   else { "E:\SteamLibrary\steamapps\common\Grand Theft Auto V" }),
  [string]$Dll = "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll",
  [string]$Out = "fixtures/texture_oracle.json",
  [string[]]$Archives = @("x64a.rpf", "common.rpf", "x64c.rpf", "x64e.rpf"),
  # Files per archive, so no single archive fills the sample.
  [int]$PerArchive = 12,
  [int]$Limit = 40,
  # How many pixels of mip 0 to record per texture.
  [int]$PixelSamples = 512
)

$ErrorActionPreference = "Stop"

Add-Type -Path $Dll
[CodeWalker.GameFiles.GTA5Keys]::LoadFromPath($Gta, $false, $null)

function Get-AllRpfs($rpf) {
  $list = @($rpf)
  foreach ($c in $rpf.Children) { $list += Get-AllRpfs $c }
  return $list
}

$sha = [System.Security.Cryptography.SHA256]::Create()

function Get-Sha256($bytes) {
  if ($null -eq $bytes -or $bytes.Length -eq 0) { return "" }
  return [System.BitConverter]::ToString($sha.ComputeHash($bytes)).Replace("-", "").ToLower()
}

# Every `stride`-th pixel of mip 0, flattened back into a BGRA byte array. The
# gate walks the same indices over its own decode.
function Get-PixelSample($px, $count, $samples) {
  $stride = [Math]::Max(1, [Math]::Floor($count / $samples))
  $taken = [System.Collections.Generic.List[byte]]::new()
  for ($i = 0; $i -lt $count; $i += $stride) {
    $at = $i * 4
    if (($at + 3) -ge $px.Length) { break }
    $taken.Add($px[$at]) | Out-Null
    $taken.Add($px[$at + 1]) | Out-Null
    $taken.Add($px[$at + 2]) | Out-Null
    $taken.Add($px[$at + 3]) | Out-Null
  }
  return [ordered]@{
    stride = [int]$stride
    bgra   = [System.Convert]::ToBase64String($taken.ToArray())
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

  $fromThisArchive = 0

  foreach ($rpf in Get-AllRpfs $root) {
    if ($fromThisArchive -ge $PerArchive -or $files.Count -ge $Limit) { break }

    foreach ($e in $rpf.AllEntries) {
      if ($fromThisArchive -ge $PerArchive -or $files.Count -ge $Limit) { break }
      if (-not ($e -is [CodeWalker.GameFiles.RpfResourceFileEntry])) { continue }
      if ([System.IO.Path]::GetExtension($e.Name).ToLower() -ne ".ytd") { continue }

      $data = $rpf.ExtractFile($e)
      if ($null -eq $data) { continue }

      try {
        $f = New-Object CodeWalker.GameFiles.YtdFile
        $f.Load($data, $e)
      } catch {
        Write-Host "  skipping $($e.Path): $_"
        continue
      }
      if ($null -eq $f.TextureDict) { continue }

      $textures = @()
      $items = $f.TextureDict.Textures.data_items
      if ($null -ne $items) {
        foreach ($t in $items) {
          if ($null -eq $t) { continue }

          # Both of these are reported rather than swallowed. A silently empty
          # field looks to the gate exactly like "the reference cannot do this
          # one", and it would skip the check instead of failing it.
          $ddsHash = ""
          try {
            $ddsHash = Get-Sha256 ([CodeWalker.Utils.DDSIO]::GetDDSFile($t))
          } catch {
            Write-Host "  GetDDSFile failed for $($e.Path)/$($t.Name): $_"
          }

          $pixels = $null
          try {
            $px = [CodeWalker.Utils.DDSIO]::GetPixels($t, 0)
            # Null is expected for BC7: `GetPixels` has an empty case for it.
            if ($null -ne $px) {
              $pixels = Get-PixelSample $px ([int]$t.Width * [int]$t.Height) $PixelSamples
            }
          } catch {
            Write-Host "  GetPixels failed for $($e.Path)/$($t.Name): $_"
          }

          $textures += [ordered]@{
            name       = $t.Name
            nameHash   = [uint32]$t.NameHash
            width      = [int]$t.Width
            height     = [int]$t.Height
            depth      = [int]$t.Depth
            stride     = [int]$t.Stride
            format     = [uint32]$t.Format
            levels     = [int]$t.Levels
            usage      = [int]$t.Usage
            usageFlags = [uint32]$t.UsageFlags
            extraFlags = [uint32]$t.ExtraFlags
            dataLength = $(if ($null -ne $t.Data) { [int]$t.Data.FullData.Length } else { 0 })
            dataSha256 = $(if ($null -ne $t.Data) { Get-Sha256 $t.Data.FullData } else { "" })
            ddsSha256  = $ddsHash
            pixels     = $pixels
          }
        }
      }

      $hashes = @()
      if ($null -ne $f.TextureDict.TextureNameHashes.data_items) {
        foreach ($h in $f.TextureDict.TextureNameHashes.data_items) { $hashes += [uint32]$h }
      }

      $files += [ordered]@{
        path         = $e.Path
        systemSize   = [int]$e.SystemSize
        graphicsSize = [int]$e.GraphicsSize
        version      = [int]$e.Version
        hashes       = $hashes
        textures     = $textures
      }
      $fromThisArchive += 1
    }
  }
}

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
[ordered]@{ files = $files } | ConvertTo-Json -Depth 10 -Compress | Out-File -FilePath $Out -Encoding utf8
Write-Host "wrote $Out with $($files.Count) texture dictionaries"
