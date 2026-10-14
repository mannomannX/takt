<#
.SYNOPSIS
Faehrt die Board-Suiten an einem festen Stand, waehrend im Arbeitsverzeichnis
weitergearbeitet wird.

.DESCRIPTION
Die Board-Suiten bauen ihre Abbilder aus den Quellen; eine Aenderung an `src`
waehrend des Laufs verfaelschte ihn (FB-193). Darum laeuft er in einem eigenen
git-Worktree auf dem Stand `-Rev` und mit eigenem Zielverzeichnis — ein
geteiltes liesse Cargo die Artefakte beider Baeume verwechseln. Der Worktree
entsteht je Lauf neu und wird danach entfernt, auch nach einem Fehlschlag;
er liegt immer unter demselben Pfad, damit das Zielverzeichnis seine
Artefakte wiederfindet.

Die Suiten laufen nacheinander; zugleich gefahren stoeren sie einander
(FB-464). Solange ein Lauf die Boards haelt, bricht ein zweiter ab
(`boards.lock` im Protokollverzeichnis, offen gehalten, bis der Lauf endet).

Je Lauf schreibt das Werkzeug ins Protokollverzeichnis das volle Protokoll
und eine Zusammenfassung (`<zeit>-<stand>.log`, `.txt`). Danach entfernt
`prune-target.ps1 -Since <Beginn>` aus dem Zielverzeichnis des Laufs, was er
nicht mehr braucht; der Abbild-Cache haelt ohnehin nur den laufenden Stand.

`-FromCi <lauf>` nimmt die Abbilder aus einem Lauf von
`.github/workflows/boards.yml` (M11 Schritt 34d): `gh run download` legt sie in
den Abbild-Cache des Zielverzeichnisses, ohne `-Rev` gilt der Commit, den der
Lauf gebaut hat, und weder `takt` noch ein Abbild wird hier gebaut — der Lauf
schreibt und prueft nur. Die Zusammenfassung nennt, wie viele Abbilder der
Cache trotzdem nicht hielt (`misses.log`); Ziel ist keines. Fehlt eines, baut
die Harness es mit dem `takt` aus einem frueheren Lauf oder scheitert daran.
Auch die Suiten selbst kommen aus dem Lauf, als nextest-Archiv vom
Windows-Runner (M11 Schritt 34e): Hier laeuft dann kein Compiler, und
`TAKT_ROOT` nennt den Testprozessen den Worktree als Quelle.

`-Bench` misst statt der Suiten: `takt bench --board <board>` je Board, die
Protokolle unter `<Logs>\<zeit>-<stand>-bench-<board>`. Bericht und
Hardware-Konfiguration (`corpus-try/hw/<board>.report`, `.hw`) schreibt der
Bench in den Hauptbaum, zum Pruefen und Committen. Mit `-FromCi` kommen die
Abbilder und `takt` aus dem Lauf von boards.yml, ohne lokalen Bau.

`-AllowMissing` nennt, was an diesem Rechner fehlen darf (`TAKT_ALLOW_MISSING`);
der Test dazu meldet sich als uebersprungen, und die Zusammenfassung sagt es.
Vorgabe ist die Bruecke an UART0 des C6, die nicht angesteckt ist.

`-Esp32c6Uart0 COM5` liest in jedem Lauf des C6 auch UART0 mit, die Konsole
des Kerns unter Zephyr (`TAKT_ESP32C6_UART0`, FB-757); scheitert ein Lauf,
steht das Gelesene in der Meldung. Die Bruecke treibt GPIO17: Der Test der
Drahtbruecke GPIO7 an GPIO17 braucht sie ausgesteckt.

Der Lauf belegt Speicher und Rechenzeit neben der Arbeit im Hauptbaum: die
volle Suite dort zugleich zu fahren, kann die Zusagegrenze des Rechners
sprengen. Exit-Code 0 heisst: jede gewaehlte Suite bestanden.

`-Filter` waehlt Tests wie `nextest -E` (etwa `test(=persistence_survives_a_reset)`),
um einzelne zu wiederholen; `TAKT_ESP32C6_ONLY` und `TAKT_F401_ONLY` der
Umgebung beschraenken den Korpusvergleich auf ein Programm.

.EXAMPLE
pwsh -NoProfile -File tools/board-run.ps1
pwsh -NoProfile -File tools/board-run.ps1 -Rev e6093b4 -Boards esp32c6
pwsh -NoProfile -File tools/board-run.ps1 -Boards esp32c6 -Filter 'test(=the_board_agrees_with_the_interpreter)'
pwsh -NoProfile -File tools/board-run.ps1 -FromCi 18423456789
pwsh -NoProfile -File tools/board-run.ps1 -FromCi 18423456789 -Bench -Boards stm32f401
pwsh -NoProfile -File tools/board-run.ps1 -FromCi 18423456789 -Boards esp32c6 -Esp32c6Uart0 COM5 -Filter 'test(/zephyr/)'
#>
param(
    [string]$Rev = 'HEAD',
    [ValidateSet('both', 'esp32c6', 'stm32f401')][string]$Boards = 'both',
    [string]$Worktree = 'G:\takt-boards',
    [string]$Target = 'G:\rust\target-boards',
    [string]$Logs = 'G:\rust\board-runs',
    [string]$Esp32c6Port = 'COM4',
    [string]$F401Port = 'COM7',
    [string]$AllowMissing = 'uart-bridge',
    [string]$Esp32c6Uart0 = '',
    [string]$Filter = '',
    [string]$FromCi = '',
    [switch]$Bench
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$suites = if ($Boards -eq 'both') { 'esp32c6', 'stm32f401' } else { , $Boards }
$images = Join-Path $Target 'takt-board-images'
New-Item -ItemType Directory -Force $Logs | Out-Null
$lockPath = Join-Path $Logs 'boards.lock'
try {
    $lock = [System.IO.File]::Open($lockPath, 'OpenOrCreate', 'ReadWrite', 'None')
} catch {
    Write-Error "Ein anderer Lauf haelt die Boards ($lockPath)."
    exit 2
}

# Die Abbilder eines Laufs von boards.yml in den Abbild-Cache: je Board der
# Stand `<tripel>-<stand>`, der fruehere desselben Boards geht. Dazu das
# Archiv der Board-Suiten. Liefert den Commit, den der Lauf gebaut hat, je
# Stand eine Zeile und den Pfad des Archivs.
function Import-CiImages([string]$Run) {
    $download = Join-Path $Target 'takt-board-ci'
    if (Test-Path $download) { Remove-Item -Recurse -Force $download }
    $names = $suites | ForEach-Object { '-n'; "board-images-$_" }
    Push-Location $repo
    try {
        gh run download $Run @names -D $download | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "gh run download $Run scheiterte" }
    } finally {
        Pop-Location
    }
    $commits = @(Get-ChildItem $download -Recurse -Filter 'stand-*.txt' | Get-Content |
        Where-Object { $_ -match '^commit ([0-9a-f]{40})$' } | ForEach-Object { $matches[1] } | Sort-Object -Unique)
    if ($commits.Count -ne 1) { throw "Lauf $Run nennt nicht genau einen Commit: $($commits -join ', ')" }
    New-Item -ItemType Directory -Force $images | Out-Null
    $stands = foreach ($stand in Get-ChildItem $download -Recurse -Directory | Where-Object Name -match '-[0-9a-f]{16}$') {
        $triple = $stand.Name -replace '-[0-9a-f]{16}$', ''
        Get-ChildItem $images -Directory -Filter "$triple-*" | Remove-Item -Recurse -Force
        $into = Join-Path $images $stand.Name
        Move-Item $stand.FullName $into
        '{0}: {1} Abbilder' -f $stand.Name, @(Get-ChildItem $into -Filter '*.elf').Count
    }
    Remove-Item -Recurse -Force $download
    $tests = Join-Path $Target 'takt-board-ci-tests'
    if (Test-Path $tests) { Remove-Item -Recurse -Force $tests }
    Push-Location $repo
    try {
        gh run download $Run -n board-tests-windows -D $tests | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "gh run download $Run -n board-tests-windows scheiterte" }
    } finally {
        Pop-Location
    }
    $archive = Join-Path $tests 'board-tests.tar.zst'
    if (-not (Test-Path $archive)) { throw "Lauf $Run brachte kein Archiv der Board-Suiten" }
    $takt = Join-Path $tests 'release\takt.exe'
    if ($Bench -and -not (Test-Path $takt)) { throw "Lauf $Run brachte kein takt fuer den Bench" }
    [pscustomobject]@{ Commit = $commits[0]; Stands = @($stands); Archive = $archive; Takt = $takt }
}

try {
    if ($FromCi) {
        $ci = Import-CiImages $FromCi
        $ci.Stands | Out-Host
        if (-not $PSBoundParameters.ContainsKey('Rev')) { $Rev = $ci.Commit }
    }
    $sha = (git -C $repo rev-parse --verify "$Rev^{commit}").Trim()
    if ($LASTEXITCODE -ne 0) { throw "Stand `$Rev` unbekannt" }
    if ($FromCi -and $sha -ne $ci.Commit) {
        Write-Warning "Lauf $FromCi baute $($ci.Commit), gefahren wird ${sha}: Nur gleiche Quellen finden ihre Abbilder."
    }
    # Ein Rest eines abgebrochenen Laufs geht zuerst.
    if (Test-Path $Worktree) { git -C $repo worktree remove --force $Worktree }
    git -C $repo worktree prune
    git -C $repo worktree add --quiet --detach $Worktree $sha
    if ($LASTEXITCODE -ne 0) { throw "Worktree $Worktree nicht auf $sha" }

    $since = Get-Date
    $name = '{0:yyyy-MM-dd_HHmm}-{1}' -f $since, $sha.Substring(0, 8)
    $log = Join-Path $Logs "$name.log"
    $env:CARGO_TARGET_DIR = $Target
    $env:TMP = 'G:\rust\tmp'
    $env:TEMP = 'G:\rust\tmp'
    $env:TAKT_ESP32C6_PORT = $Esp32c6Port
    $env:TAKT_F401_PORT = $F401Port
    $env:TAKT_ALLOW_MISSING = $AllowMissing
    if ($Esp32c6Uart0) {
        $env:TAKT_ESP32C6_UART0 = $Esp32c6Uart0
    } else {
        Remove-Item Env:TAKT_ESP32C6_UART0 -ErrorAction SilentlyContinue
    }
    # Die Quellen des Laufs fuer die Testprozesse, auch fuer die aus dem
    # Archiv, die anderswo gebaut wurden.
    $env:TAKT_ROOT = $Worktree
    # Je Lauf zaehlt die Harness neu, was der Abbild-Cache nicht hielt.
    $misses = Join-Path $images 'misses.log'
    if (Test-Path $misses) { Remove-Item $misses }

    $results = [ordered]@{}
    Push-Location $Worktree
    try {
        # Die Bring-ups lehnen ein `takt` ab, das aelter ist als seine Quellen
        # (FB-193); mit den Abbildern aus CI baut keines.
        $cli = 0
        if (-not $FromCi) {
            cargo build --release -p takt-cli -j 2 *>> $log
            $cli = $LASTEXITCODE
        }
        foreach ($b in $suites) {
            if ($cli -ne 0) { $results[$b] = 'nicht gelaufen: der Bau von takt scheiterte'; continue }
            if ($Bench) {
                $takt = if ($FromCi) { $ci.Takt } else { Join-Path $Target 'release\takt.exe' }
                $hw = Join-Path $repo "corpus-try\hw\$b"
                & $takt bench --board $b --logs (Join-Path $Logs "$name-bench-$b") `
                    --hardware "$hw.hw" --conformance "$hw.report" *>> $log
                $results[$b] = '{0} (Bench)' -f $(if ($LASTEXITCODE -eq 0) { 'bestanden' } else { 'gescheitert' })
                continue
            }
            if ($FromCi) {
                $expr = if ($Filter) { "binary(=board_$b) & ($Filter)" } else { "binary(=board_$b)" }
                cargo nextest run --archive-file $ci.Archive --workspace-remap $Worktree `
                    --run-ignored ignored-only --no-fail-fast -E $expr *>> $log
            } else {
                $select = if ($Filter) { @('-E', $Filter) } else { @() }
                cargo nextest run -p takt-conformance --test "board_$b" --run-ignored ignored-only --no-fail-fast `
                    --build-jobs 2 @select *>> $log
            }
            $code = $LASTEXITCODE
            $summary = Select-String -Path $log -Pattern '^\s+Summary ' | Select-Object -Last 1
            $results[$b] = '{0} ({1})' -f $(if ($code -eq 0) { 'bestanden' } else { 'gescheitert' }),
                ($summary.Line ?? 'keine Zusammenfassung').Trim()
        }
    } finally {
        Pop-Location
    }

    $failed = Select-String -Path $log -Pattern '^\s+(FAIL|ABORT|TIMEOUT) \[' | ForEach-Object { $_.Line.Trim() } |
        Sort-Object -Unique
    $lines = @("Stand $sha", "Beginn $($since.ToString('s')), Ende $((Get-Date).ToString('s'))")
    if ($FromCi) { $lines += "Abbilder aus Lauf $FromCi von boards.yml:"; $lines += $ci.Stands | ForEach-Object { "  $_" } }
    if ($Filter) { $lines += "nur: $Filter" }
    $lines += $results.GetEnumerator() | ForEach-Object { '{0}: {1}' -f $_.Key, $_.Value }
    $missed = @(if (Test-Path $misses) { Get-Content $misses })
    $lines += "Abbilder gebaut, weil der Cache sie nicht hielt: $($missed.Count)"
    $lines += $missed | Select-Object -First 10 | ForEach-Object { "  $_" }
    if ($AllowMissing) { $lines += "fehlen darf: $AllowMissing (die Tests dazu uebersprungen)" }
    $lines += $failed
    $lines | Set-Content -Encoding utf8 (Join-Path $Logs "$name.txt")
    $lines

    # Was dieser Lauf nicht mehr braucht, geht gleich.
    & (Join-Path $repo 'tools\prune-target.ps1') -Target $Target -Since $since -Apply *>> $log
    if ($results.Values | Where-Object { -not $_.StartsWith('bestanden') }) { exit 1 }
    exit 0
} finally {
    if (Test-Path $Worktree) { git -C $repo worktree remove --force $Worktree }
    git -C $repo worktree prune
    $lock.Dispose()
}
