# Dump the C# reference's view of a sample of clip dictionaries, for
# cross-checking the Rust `.ycd` port clip by clip.
#
# Same trick as the other oracles: CodeWalker.Core.dll is netstandard2.0, so
# PowerShell loads it and calls the real YcdFile reader. Expected values must
# come from here, never from reading this port's own output back.
#
#   pwsh -File scripts/clip-oracle.ps1
#   cargo test -p cw-core --test clip_gate -- --ignored --nocapture
#
# The output lands in fixtures/, which is gitignored: it is metadata derived
# from a retail install and is not redistributable.
#
# Both maps are hash tables in the file and dictionaries in the reference, so
# the gate matches on hash rather than on position. Order is not a fact about
# the format.

param(
  [string]$Gta = $(if ($env:ASTRAMAPPER_GTA_PATH) { $env:ASTRAMAPPER_GTA_PATH }
                   else { "E:\SteamLibrary\steamapps\common\Grand Theft Auto V" }),
  [string]$Dll = "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll",
  [string]$Out = "fixtures/clip_oracle.json",
  [string[]]$Archives = @("x64a.rpf", "x64c.rpf", "x64f.rpf"),
  [int]$Limit = 24
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
      if ([System.IO.Path]::GetExtension($e.Name).ToLower() -ne ".ycd") { continue }

      $data = $rpf.ExtractFile($e)
      if ($null -eq $data) { continue }

      try {
        $f = New-Object CodeWalker.GameFiles.YcdFile
        $f.Load($data, $e)
      } catch {
        Write-Host "  skipping $($e.Path): $_"
        continue
      }
      if ($null -eq $f.ClipDictionary) { continue }

      $clips = @()
      foreach ($cme in $f.ClipMapEntries) {
        $c = $cme.Clip
        if ($null -eq $c) { continue }

        $anims = @()
        $start = 0.0; $end = 0.0; $rate = 0.0; $duration = 0.0
        if ($c -is [CodeWalker.GameFiles.ClipAnimation]) {
          $start = [float]$c.StartTime
          $end = [float]$c.EndTime
          $rate = [float]$c.Rate
          $duration = $end - $start
          if ($null -ne $c.Animation) { $anims += [uint32]$c.Animation.Hash.Hash }
        } elseif ($c -is [CodeWalker.GameFiles.ClipAnimationList]) {
          $duration = [float]$c.Duration
          $end = $duration
          if ($null -ne $c.Animations) {
            $first = $true
            foreach ($cae in $c.Animations) {
              if ($null -eq $cae) { continue }
              if ($first) { $rate = [float]$cae.Rate; $first = $false }
              if ($null -ne $cae.Animation) { $anims += [uint32]$cae.Animation.Hash.Hash }
            }
          }
        }

        $clips += [ordered]@{
          hash          = [uint32]$cme.Hash.Hash
          type          = [int]$c.Type
          name          = $c.Name
          shortName     = $c.ShortName
          startTime     = $start
          endTime       = $end
          rate          = $rate
          duration      = $duration
          animations    = $anims
          tagCount      = [int]($(if ($null -ne $c.Tags) { $c.Tags.TagCount1 } else { 0 }))
          propertyCount = [int]($(if ($null -ne $c.Properties) { $c.Properties.PropertyEntriesCount } else { 0 }))
        }
      }

      $anims = @()
      foreach ($ame in $f.AnimMapEntries) {
        $an = $ame.Animation
        if ($null -eq $an) { continue }
        $seqFrames = @()
        if ($null -ne $an.Sequences -and $null -ne $an.Sequences.data_items) {
          foreach ($s in $an.Sequences.data_items) {
            if ($null -eq $s) { continue }
            $seqFrames += [int]$s.NumFrames
          }
        }
        $boneIds = @()
        if ($null -ne $an.BoneIds -and $null -ne $an.BoneIds.data_items) {
          foreach ($b in $an.BoneIds.data_items) {
            $boneIds += [ordered]@{ boneId = [int]$b.BoneId; track = [int]$b.Track; unk0 = [int]$b.Unk0 }
          }
        }
        $anims += [ordered]@{
          hash              = [uint32]$ame.Hash.Hash
          frames            = [int]$an.Frames
          sequenceFrameLimit = [int]$an.SequenceFrameLimit
          duration          = [float]$an.Duration
          usageCount        = [uint32]$an.UsageCount
          sequenceFrames    = $seqFrames
          boneIds           = $boneIds
        }
      }

      $files += [ordered]@{
        path       = $e.Path
        systemSize = [int]$e.SystemSize
        version    = [int]$e.Version
        clips      = $clips
        animations = $anims
      }
      Write-Host "  $($e.Path): $($clips.Count) clips, $($anims.Count) animations"
    }
    if ($files.Count -ge $Limit) { break }
  }
  if ($files.Count -ge $Limit) { break }
}

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
# WriteAllText rather than Out-File: Windows PowerShell 5.1's `-Encoding utf8`
# emits a BOM, and `serde_json` refuses a document that starts with one.
$json = [ordered]@{ files = $files } | ConvertTo-Json -Depth 10 -Compress
[System.IO.File]::WriteAllText(
  [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Out)),
  $json,
  (New-Object System.Text.UTF8Encoding($false)))
Write-Host "wrote $Out with $($files.Count) clip dictionaries"
