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

`-AllowMissing` nennt, was an diesem Rechner fehlen darf (`TAKT_ALLOW_MISSING`);
der Test dazu meldet sich als uebersprungen, und die Zusammenfassung sagt es.
Vorgabe ist die Bruecke an UART0 des C6, die nicht angesteckt ist.

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
    [string]$Filter = ''
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
New-Item -ItemType Directory -Force $Logs | Out-Null
$lockPath = Join-Path $Logs 'boards.lock'
try {
    $lock = [System.IO.File]::Open($lockPath, 'OpenOrCreate', 'ReadWrite', 'None')
} catch {
    Write-Error "Ein anderer Lauf haelt die Boards ($lockPath)."
    exit 2
}

try {
    $sha = (git -C $repo rev-parse --verify "$Rev^{commit}").Trim()
    if ($LASTEXITCODE -ne 0) { throw "Stand `$Rev` unbekannt" }
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

    $results = [ordered]@{}
    Push-Location $Worktree
    try {
        # Die Bring-ups lehnen ein `takt` ab, das aelter ist als seine Quellen (FB-193).
        cargo build --release -p takt-cli -j 2 *>> $log
        $cli = $LASTEXITCODE
        $suites = if ($Boards -eq 'both') { 'esp32c6', 'stm32f401' } else { , $Boards }
        foreach ($b in $suites) {
            if ($cli -ne 0) { $results[$b] = 'nicht gelaufen: der Bau von takt scheiterte'; continue }
            $select = if ($Filter) { @('-E', $Filter) } else { @() }
            cargo nextest run -p takt-conformance --test "board_$b" --run-ignored ignored-only --no-fail-fast `
                --build-jobs 2 @select *>> $log
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
    if ($Filter) { $lines += "nur: $Filter" }
    $lines += $results.GetEnumerator() | ForEach-Object { '{0}: {1}' -f $_.Key, $_.Value }
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
