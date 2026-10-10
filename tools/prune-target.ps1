<#
.SYNOPSIS
Entfernt veraltete Build-Artefakte aus dem Zielverzeichnis von Takt.

.DESCRIPTION
Cargo legt je Testdatei und Crate eine eigene Fassung an, sobald sich
Abhaengigkeiten, Features, Toolchain oder Flags aendern — schon ein Lauf mit
`-p <paket>` statt `--workspace` vereinigt die Features anders (Cargo 1.98
kann das nur mit `-Zfeature-unification` abstellen) — und laesst die alten
liegen. Entfernt wird, was ein Bau bei Bedarf wieder anlegt:

  - in `debug\deps` und `release\deps` die aelteren Fassungen derselben
    Testdatei (`.exe`, `.pdb`, `.d`) — die neueste bleibt;
  - in `debug\incremental` die aelteren Caches desselben Crates — der
    neueste bleibt;
  - in `release\build` des Zielverzeichnisses und seiner Bauplaetze
    (`takt-board-build\<n>`) die Ausgaben der Bring-ups (`takt-bringup-*`),
    die aelter als `-Days` Tage sind: Ein Board-Lauf legt mehrere zugleich
    an. Mit `-Since` die, die seit diesem Zeitpunkt kein Bau beschrieben hat
    — der Board-Lauf (`board-run.ps1`) raeumt so gleich nach sich auf; ein
    Abbild, das er braucht, haelt der Abbild-Cache.

Bibliotheken und alles andere bleiben. Ohne `-Apply` nur der Bericht.
Waehrend ein Bau oder Testlauf in dieses Zielverzeichnis laeuft, nicht
anwenden.

.EXAMPLE
pwsh tools/prune-target.ps1
pwsh tools/prune-target.ps1 -Apply
pwsh tools/prune-target.ps1 -Target G:\rust\target-boards -Since (Get-Date).AddHours(-1) -Apply
#>
param(
    [string]$Target = $(if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'G:\rust\target' }),
    [int]$Days = 7,
    [Nullable[datetime]]$Since = $null,
    [switch]$Apply
)

$ErrorActionPreference = 'Stop'
$cutoff = if ($Since) { $Since } else { (Get-Date).AddDays(-$Days) }
$found = [System.Collections.Generic.List[object]]::new()

function Add-Item([string]$Kind, [System.IO.FileSystemInfo]$Item, [long]$Bytes) {
    $found.Add([pscustomobject]@{ Kind = $Kind; Item = $Item; Bytes = $Bytes })
}

function Get-Size([System.IO.DirectoryInfo]$Dir) {
    $o = [System.IO.EnumerationOptions]::new()
    $o.RecurseSubdirectories = $true
    $o.IgnoreInaccessible = $true
    $o.AttributesToSkip = 0
    $sum = 0L
    foreach ($f in $Dir.EnumerateFiles('*', $o)) { $sum += $f.Length }
    $sum
}

# Aeltere Fassungen derselben Testdatei: `<name>-<hash>.exe` und `.pdb`; die
# neueste Fassung je Name bleibt.
foreach ($profile in 'debug', 'release') {
    $deps = [System.IO.DirectoryInfo]::new((Join-Path $Target "$profile\deps"))
    if (-not $deps.Exists) { continue }
    $files = @($deps.EnumerateFiles())
    $byStem = @{}
    foreach ($f in $files) { $byStem[$f.Name] = $f }
    $tests = foreach ($f in $files) {
        if ($f.Name -match '^(.+)-([0-9a-f]{16})\.exe$') {
            [pscustomobject]@{ Name = $matches[1]; Stem = "$($matches[1])-$($matches[2])"; File = $f }
        }
    }
    foreach ($g in $tests | Group-Object Name | Where-Object Count -gt 1) {
        $newest = ($g.Group | Sort-Object { $_.File.LastWriteTime } -Descending | Select-Object -First 1).Stem
        foreach ($t in $g.Group | Where-Object Stem -ne $newest) {
            foreach ($ext in '.exe', '.pdb', '.d') {
                $f = $byStem["$($t.Stem)$ext"]
                if ($f) { Add-Item "$profile\deps" $f $f.Length }
            }
        }
    }
}

# Aeltere Caches desselben Crates: `<crate>-<hash>`; der neueste bleibt.
$incremental = [System.IO.DirectoryInfo]::new((Join-Path $Target 'debug\incremental'))
if ($incremental.Exists) {
    $crates = $incremental.EnumerateDirectories() | Group-Object { $_.Name -replace '-[0-9a-z]+$', '' }
    foreach ($g in $crates | Where-Object Count -gt 1) {
        foreach ($d in $g.Group | Sort-Object LastWriteTime -Descending | Select-Object -Skip 1) {
            Add-Item 'debug\incremental' $d (Get-Size $d)
        }
    }
}

# Die Build-Ausgaben der Bring-ups aelterer Quellstaende, auch in den
# Bauplaetzen der Boards.
$slots = Join-Path $Target 'takt-board-build'
$roots = @($Target) + @(if (Test-Path $slots) { Get-ChildItem $slots -Directory | ForEach-Object FullName })
foreach ($root in $roots) {
    $build = [System.IO.DirectoryInfo]::new((Join-Path $root 'release\build'))
    if (-not $build.Exists) { continue }
    foreach ($d in $build.EnumerateDirectories('takt-bringup-*') | Where-Object LastWriteTime -lt $cutoff) {
        Add-Item 'release\build' $d (Get-Size $d)
    }
}

foreach ($g in $found | Group-Object Kind) {
    '{0,-20} {1,6} Eintraege {2,8:N2} GB' -f $g.Name, $g.Count, (($g.Group | Measure-Object Bytes -Sum).Sum / 1GB)
}
$total = ($found | Measure-Object Bytes -Sum).Sum / 1GB
'{0,-20} {1,6} Eintraege {2,8:N2} GB' -f 'zusammen', $found.Count, $total

if (-not $Apply) {
    'Nur der Bericht; mit -Apply wird geloescht.'
    return
}
$removed = 0L
$failed = 0
foreach ($e in $found) {
    try {
        if ($e.Item -is [System.IO.DirectoryInfo]) { $e.Item.Delete($true) } else { $e.Item.Delete() }
        $removed += $e.Bytes
    } catch {
        $failed++
        Write-Warning "$($e.Item.FullName): $($_.Exception.Message)"
    }
}
'{0:N2} GB entfernt, {1} Eintraege nicht.' -f ($removed / 1GB), $failed
