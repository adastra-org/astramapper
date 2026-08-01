# Dump the C# reference's view of a sample of AWC audio containers, for
# cross-checking the Rust port stream by stream — including the decoded PCM.
#
# Same trick as rpf-oracle.ps1: CodeWalker.Core.dll is netstandard2.0, so
# Windows PowerShell 5.1 loads it and calls the real AwcFile reader. Expected
# values must come from here, never from reading this port's own output back.
#
#   pwsh -File scripts/awc-oracle.ps1
#   cargo test -p cw-core --test awc_gate -- --ignored --nocapture
#
# The PCM hash covers only the first `Samples` samples. CodeWalker's decoder
# over-allocates its output buffer and returns the whole thing, so its WAVs carry
# four bytes of silence per ADPCM block; this port trims to the declared sample
# count. Truncating here compares the audio and not the padding.
#
# The output lands in fixtures/, which is gitignored: it is metadata derived from
# a retail install and is not redistributable.

param(
  [string]$Gta = $(if ($env:ASTRAMAPPER_GTA_PATH) { $env:ASTRAMAPPER_GTA_PATH }
                   else { "E:\SteamLibrary\steamapps\common\Grand Theft Auto V" }),
  [string]$Dll = "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll",
  [string]$Out = "fixtures/awc_oracle.json",
  # A spread across the kinds of container the game ships: mono sfx, weapons,
  # streamed radio (multi-channel), speech, and a DLC pack.
  [string[]]$Archives = @(
    "x64\audio\sfx\WEAPONS_PLAYER.rpf",
    "x64\audio\sfx\ANIMALS.rpf",
    "x64\audio\sfx\PAIN.rpf",
    "x64\audio\sfx\RADIO_02_POP.rpf",
    "x64\audio\sfx\POLICE_SCANNER.rpf",
    "update\x64\dlcpacks\patchday3ng\dlc.rpf"
  ),
  # Files per archive, so no single archive fills the sample.
  [int]$PerArchive = 6,
  [int]$Limit = 40
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
$files = @()

foreach ($a in $Archives) {
  $full = Join-Path $Gta $a
  if (-not (Test-Path $full)) { Write-Host "skipping missing $full"; continue }
  Write-Host "scanning $full"

  $root = New-Object CodeWalker.GameFiles.RpfFile($full, $a)
  $root.ScanStructure($null, $null)
  if ($null -ne $root.LastException) { throw "$a`: $($root.LastError)" }

  $taken = 0
  foreach ($rpf in Get-AllRpfs $root) {
    foreach ($e in $rpf.AllEntries) {
      if (($files.Count -ge $Limit) -or ($taken -ge $PerArchive)) { break }
      if (-not ($e -is [CodeWalker.GameFiles.RpfFileEntry])) { continue }
      if ([System.IO.Path]::GetExtension($e.Name).ToLower() -ne ".awc") { continue }

      $data = $rpf.ExtractFile($e)
      if ($null -eq $data) { continue }

      try {
        $awc = New-Object CodeWalker.GameFiles.AwcFile
        $awc.Load($data, $e)
      } catch {
        Write-Host "  skipping $($e.Path): $_"
        continue
      }
      if (-not [string]::IsNullOrEmpty($awc.ErrorMessage)) {
        Write-Host "  skipping $($e.Path): $($awc.ErrorMessage)"
        continue
      }

      $streams = @()
      foreach ($s in $awc.Streams) {
        $fmt = $s.FormatChunk
        $sfm = $s.StreamFormat
        $codec = -1
        $rate = 0
        $samples = 0
        if ($null -ne $fmt) {
          $codec = [int]$fmt.Codec; $rate = [int]$fmt.SamplesPerSecond; $samples = [int]$fmt.Samples
        } elseif ($null -ne $sfm) {
          $codec = [int]$sfm.Codec; $rate = [int]$sfm.SamplesPerSecond; $samples = [int]$sfm.Samples
        }

        $pcmHash = $null
        $pcmLen = -1
        # Hash 0 marks the multi-channel carrier, which holds everyone's audio
        # and is not itself a playable stream.
        if (($s.Hash -ne 0) -and ($null -eq $s.MidiChunk) -and ($samples -gt 0)) {
          try {
            $pcm = $s.GetPcmData()
            if ($null -ne $pcm) {
              $want = [Math]::Min($samples * 2, $pcm.Length)
              $trimmed = New-Object byte[] $want
              [System.Buffer]::BlockCopy($pcm, 0, $trimmed, 0, $want)
              $pcmLen = $want
              $pcmHash = [System.BitConverter]::ToString($sha.ComputeHash($trimmed)).Replace("-", "").ToLower()
            }
          } catch {
            Write-Host "  $($e.Path) stream $($s.Hash): pcm failed: $_"
          }
        }

        $streams += [ordered]@{
          id         = [uint32]$s.Hash
          codec      = $codec
          sampleRate = $rate
          samples    = $samples
          isMidi     = [int]($null -ne $s.MidiChunk)
          chunkCount = [int]($s.Chunks.Length)
          pcmLength  = $pcmLen
          pcmSha256  = $pcmHash
        }
      }

      $files += [ordered]@{
        path                = $e.Path
        version             = [int]$awc.Version
        flags               = [int]$awc.Flags
        multiChannel        = [int]$awc.MultiChannelFlag
        wholeFileEncrypted  = [int]$awc.WholeFileEncrypted
        streamCount         = [int]$awc.StreamCount
        streams             = $streams
      }
      $taken++
      Write-Host "  $($e.Path): $($awc.StreamCount) streams, multichannel=$($awc.MultiChannelFlag)"
    }
    if (($files.Count -ge $Limit) -or ($taken -ge $PerArchive)) { break }
  }
  if ($files.Count -ge $Limit) { break }
}

$sha.Dispose()

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
[ordered]@{ files = $files } | ConvertTo-Json -Depth 8 -Compress | Out-File -FilePath $Out -Encoding utf8
Write-Host "wrote $Out with $($files.Count) awc files"
