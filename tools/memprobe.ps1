# Spitzenspeicher je Test der Suite, mit allem, was er startet (Compiler,
# Linker, gebaute Programme, Solver), bis die Stoppdatei existiert; alle
# 30 s und am Ende eine CSV nach $Out, dazu der kleinste freie Commit des
# Rechners. Daraus folgt die Gruppe `memory` in `.config/nextest.toml`:
#
#   pwsh -NoProfile -File tools/memprobe.ps1        # im Hintergrund
#   cargo nextest run --workspace; New-Item G:\rust\tmp\memprobe.stop
param([string]$Out = "G:\rust\tmp\memprobe.csv", [string]$Stop = "G:\rust\tmp\memprobe.stop")
Remove-Item $Stop -ErrorAction SilentlyContinue
$filter = "ExecutablePath LIKE 'G:\\%' OR ExecutablePath LIKE 'D:\\%' OR Name = 'z3.exe' OR Name = 'clang.exe' OR Name = 'link.exe' OR Name = 'lld-link.exe'"

# Ein Testprozess: ein Binary der Suite mit dem Testnamen als erstem
# Argument, das keine Option ist.
function TestKey($p) {
    if (-not ([string]$p.ExecutablePath).StartsWith('G:\rust\target\', 'OrdinalIgnoreCase')) { return $null }
    $cmd = [string]$p.CommandLine
    if ($cmd -match '^("[^"]+"|\S+)\s*(.*)$') {
        foreach ($t in ($Matches[2] -split '\s+')) {
            if ($t -and -not $t.StartsWith('-')) { return ($p.Name -replace '-[0-9a-f]{16}\.exe$', '') + "::" + $t }
        }
    }
    return $null
}

# Der Test, zu dem ein Prozess gehoert, ueber seine Eltern.
function Owner($p, $byPid) {
    $cur = $p
    for ($i = 0; $i -lt 8 -and $cur; $i++) {
        $k = TestKey $cur
        if ($k) { return $k }
        $cur = $byPid[$cur.ParentProcessId]
    }
    return $null
}

$peak = @{}
$minFree = [int64]::MaxValue
function Dump {
    $rows = $peak.GetEnumerator() | ForEach-Object {
        [pscustomobject]@{ test = $_.Key; sum_private_mb = [int]($_.Value[0] / 1MB); max_proc_mb = [int]($_.Value[1] / 1MB); tools = $_.Value[2] }
    } | Sort-Object sum_private_mb -Descending
    $rows | Export-Csv -Path $Out -NoTypeInformation -Encoding utf8
    "min_commit_free_mb,{0}" -f [int]($minFree / 1MB) | Out-File -FilePath ($Out + ".free") -Encoding utf8
}
$last = Get-Date
while (-not (Test-Path $Stop)) {
    $procs = Get-CimInstance Win32_Process -Filter $filter
    $byPid = @{}
    foreach ($p in $procs) { $byPid[$p.ProcessId] = $p }
    # Je Test die Summe ueber seine Prozesse in diesem Augenblick.
    $now = @{}
    foreach ($p in $procs) {
        $key = Owner $p $byPid
        if (-not $key) { continue }
        $private = [int64]$p.PrivatePageCount
        if (-not $now.ContainsKey($key)) { $now[$key] = @(0, 0, @{}) }
        $now[$key][0] += $private
        if ($private -gt $now[$key][1]) { $now[$key][1] = $private }
        $now[$key][2][$p.Name] = 1
    }
    foreach ($k in $now.Keys) {
        if (-not $peak.ContainsKey($k)) { $peak[$k] = @(0, 0, "") }
        if ($now[$k][0] -gt $peak[$k][0]) { $peak[$k][0] = $now[$k][0] }
        if ($now[$k][1] -gt $peak[$k][1]) { $peak[$k][1] = $now[$k][1] }
        $names = @($peak[$k][2] -split ' ' | Where-Object { $_ }) + @($now[$k][2].Keys) | Sort-Object -Unique
        $peak[$k][2] = ($names -join ' ')
    }
    $os = Get-CimInstance Win32_OperatingSystem
    $free = [int64]$os.FreeVirtualMemory * 1KB
    if ($free -lt $minFree) { $minFree = $free }
    if (((Get-Date) - $last).TotalSeconds -gt 30) { Dump; $last = Get-Date }
    Start-Sleep -Milliseconds 700
}
Dump
