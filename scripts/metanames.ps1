# Generate crates/cw-core/assets/metanames.bin from the reference's MetaNames.cs.
#
# `MetaNames.TryGetString` is `Enum.IsDefined` over a 20k-entry `MetaName` enum:
# the name table that turns a hash in a .ymt back into `CScenarioPointRegion` or
# `archetypeName`. Porting it as Rust source would be a 700 KB file for the
# compiler to chew on for no benefit, so it becomes a data asset instead, the
# same way `magic.dat` already is.
#
#   pwsh -File scripts/metanames.ps1
#
# Format, all little-endian:
#   "CWNM"  u32 version (1)  u32 count
#   count * { u32 hash, u32 offset }     sorted by hash, so lookup is a bsearch
#   blob of NUL-terminated ASCII names, addressed by offset
#
# Duplicate hashes keep the first declaration, which is what `((MetaName)h)`
# resolves to. A leading '@' is stripped, as `TryGetString` does for `@null`.

param(
    [string]$Source = "E:\Ad Astra Tools\CodeWalkerAstra\CodeWalker.Core\GameFiles\MetaTypes\MetaNames.cs",
    [string]$Output = (Join-Path $PSScriptRoot "..\crates\cw-core\assets\metanames.bin")
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $Source)) {
    throw "MetaNames.cs not found at $Source. See docs/porting-guide.md for where the reference lives."
}

$lines = Get-Content -LiteralPath $Source
$inEnum = $false
$byHash = [ordered]@{}
$duplicates = 0

foreach ($line in $lines) {
    if (-not $inEnum) {
        if ($line -match "enum\s+MetaName\s*:") { $inEnum = $true }
        continue
    }
    if ($line -match "^\s*\}") { break }

    # `Name = 1234,` with an optional trailing // comment. Values are decimal
    # throughout; a hex literal would need a second branch here.
    if ($line -notmatch "^\s*(@?[A-Za-z_][A-Za-z0-9_]*)\s*=\s*([0-9]+)\s*,") { continue }

    $name = $Matches[1]
    if ($name.StartsWith("@")) { $name = $name.Substring(1) }
    $hash = [uint32]$Matches[2]

    if ($byHash.Contains($hash)) { $duplicates++; continue }
    $byHash[$hash] = $name
}

if ($byHash.Count -eq 0) { throw "No MetaName entries parsed out of $Source." }

$sorted = $byHash.GetEnumerator() | Sort-Object { [uint32]$_.Key }

$blob = New-Object System.IO.MemoryStream
$index = New-Object System.IO.MemoryStream
foreach ($entry in $sorted) {
    $offset = [uint32]$blob.Length
    $bytes = [System.Text.Encoding]::ASCII.GetBytes([string]$entry.Value)
    $blob.Write($bytes, 0, $bytes.Length)
    $blob.WriteByte(0)

    $index.Write([BitConverter]::GetBytes([uint32]$entry.Key), 0, 4)
    $index.Write([BitConverter]::GetBytes($offset), 0, 4)
}

$out = New-Object System.IO.MemoryStream
$header = [System.Text.Encoding]::ASCII.GetBytes("CWNM")
$out.Write($header, 0, 4)
$out.Write([BitConverter]::GetBytes([uint32]1), 0, 4)
$out.Write([BitConverter]::GetBytes([uint32]$byHash.Count), 0, 4)
$indexBytes = $index.ToArray()
$out.Write($indexBytes, 0, $indexBytes.Length)
$blobBytes = $blob.ToArray()
$out.Write($blobBytes, 0, $blobBytes.Length)

$resolved = [System.IO.Path]::GetFullPath($Output)
[System.IO.File]::WriteAllBytes($resolved, $out.ToArray())

Write-Host "$($byHash.Count) names, $duplicates duplicate hashes skipped"
Write-Host "wrote $resolved ($($out.Length) bytes)"
