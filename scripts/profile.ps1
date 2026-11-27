# Profiles a session: what uses the CPU (by thread), the card and memory, with
# the layer's own timeline of a few frames. Starts the title the way
# play-nvidia.ps1 does (same switches), samples the PC every two seconds, and writes a report.
#   scripts\profile.ps1 -Exe <path of your copy's executable> [-Console series-x]
# -At       seconds after start for the timeline of -Frames frames (default 240, 4)
# -Seconds  how long to profile; the game is closed then (default 300; 0: until you close it)
# -Out      the folder for the report (default %LOCALAPPDATA%\victory-profile\<time>)
# -Launch   other options for play-nvidia.ps1, as text (e.g. '-Dlss off')
# -More     other settings for play-nvidia.ps1, as there
# -Again    only write the report again from the files in -Out
# Uses the diagnostics build (-Build diag). Play normally meanwhile:
# the report describes the scene you are in at -At seconds.
param(
    [string]$Exe = '',
    [ValidateSet('series-x', 'series-s')][string]$Console = 'series-s',
    [int]$At = 240,
    [int]$Frames = 4,
    [int]$Seconds = 300,
    [string]$Out = '',
    [string]$Launch = '',
    [hashtable]$More = @{},
    [switch]$Again
)
$ErrorActionPreference = 'Stop'
if (-not $Out) { $Out = Join-Path $env:LOCALAPPDATA ('victory-profile\' + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
New-Item -ItemType Directory -Force $Out | Out-Null
$log = Join-Path $Out 'victory.log'
$timeline = Join-Path $Out 'timeline.json'
if (-not $Again) {
if (-not $Exe) { throw '-Exe is needed (the path of your copy of the game)' }
$more = @{} + $More
$more.VICTORY_TIMELINE = "$At,$Frames,$timeline"
$more.VICTORY_CARD_SPANS = '1'

# The session, as play-nvidia.ps1 starts it, its output in the folder.
$play = Join-Path $PSScriptRoot 'play-nvidia.ps1'
$moreText = ($more.GetEnumerator() | ForEach-Object { "'$($_.Key)'='$($_.Value)'" }) -join ';'
$seriesS = if ($Console -eq 'series-s') { '-SeriesS' } else { '' }
$cmd = "& '$play' -Exe '$Exe' -Build diag $seriesS $Launch -More @{$moreText} *> '$log'"
$runner = Start-Process powershell -ArgumentList '-NoProfile', '-Command', $cmd -PassThru -WindowStyle Minimized
Write-Host "Profiling into $Out"

# Samples: the card (nvidia-smi), the process (CPU, memory, threads), the PC.
$samples = New-Object System.Collections.Generic.List[object]
$threadCpu = @{}
$first = $null
$cores = [Environment]::ProcessorCount
$smi = Get-Command nvidia-smi -ErrorAction SilentlyContinue
$started = Get-Date
$lastCpu = $null; $lastAt = $null
while ($true) {
    Start-Sleep -Seconds 2
    $p = Get-Process victory -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $p) {
        if ($first -or ((Get-Date) - $started).TotalSeconds -gt 120) { break } else { continue }
    }
    if (-not $first) { $first = Get-Date }
    $el = ((Get-Date) - $first).TotalSeconds
    if ($Seconds -gt 0 -and $el -ge $Seconds) { break }
    $now = Get-Date
    $cpu = $p.TotalProcessorTime.TotalSeconds
    $pct = if ($lastCpu -ne $null) { 100 * ($cpu - $lastCpu) / (($now - $lastAt).TotalSeconds * $cores) } else { 0 }
    $lastCpu = $cpu; $lastAt = $now
    $gpu = $null; $vram = $null
    if ($smi) {
        $q = (& nvidia-smi --query-gpu=utilization.gpu,memory.used --format=csv,noheader,nounits 2>$null) -split ','
        if ($q.Count -ge 2) { $gpu = [int]$q[0]; $vram = [int]$q[1] }
    }
    $os = Get-CimInstance Win32_OperatingSystem
    $mem = Get-CimInstance Win32_PerfFormattedData_PerfOS_Memory
    foreach ($t in $p.Threads) {
        try { $threadCpu[$t.Id] = $t.TotalProcessorTime.TotalSeconds } catch {}
    }
    $samples.Add([pscustomobject]@{
        s = [int]$el; cpu_pct = [math]::Round($pct, 1); gpu_pct = $gpu; vram_mb = $vram
        ws_mb = [int]($p.WorkingSet64 / 1MB); private_mb = [int]($p.PrivateMemorySize64 / 1MB)
        free_mb = [int]($os.FreePhysicalMemory / 1KB); pages_in_s = $mem.PagesInputPersec
    })
}
$samples | Export-Csv (Join-Path $Out 'samples.csv') -NoTypeInformation
$threadCpu.GetEnumerator() | ForEach-Object { [pscustomobject]@{ tid = $_.Key; cpu_s = $_.Value } } | Export-Csv (Join-Path $Out 'threads.csv') -NoTypeInformation
if ($Seconds -gt 0) { Get-Process victory -ErrorAction SilentlyContinue | Stop-Process -Force }
Start-Sleep -Seconds 3
} else {
    $samples = Import-Csv (Join-Path $Out 'samples.csv') | ForEach-Object { $_.s = [int]$_.s; $_ }
    $threadCpu = @{}
    $threads = Join-Path $Out 'threads.csv'
    if (Test-Path $threads) { Import-Csv $threads | ForEach-Object { $threadCpu[[int]$_.tid] = [double]$_.cpu_s } }
    $cores = [Environment]::ProcessorCount
}

# The log, its lines whole again (the console wrapped them).
$whole = New-Object System.Collections.Generic.List[string]
foreach ($line in (Get-Content $log -ErrorAction SilentlyContinue)) {
    if ($line -match '^\d{4}-\d\d-\d\dT' -or $whole.Count -eq 0) { $whole.Add($line) } else { $whole[$whole.Count - 1] += $line }
}
[IO.File]::WriteAllLines((Join-Path $Out 'victory-lines.log'), $whole)
function Last([string]$pat) { $whole | Where-Object { $_ -match $pat } | Select-Object -Last 1 }
function Bare([string]$line) { ($line -replace '^\S+\s+\S+\s+\S+\s', '') -replace '\s{2,}', ' ' }

# The report.
$r = New-Object System.Text.StringBuilder
function Add([string]$line) { [void]$r.AppendLine($line) }
Add "# Profile of a session ($Console)"
Add ''
$world = $samples | Where-Object { $_.s -ge 120 }
if (-not $world) { $world = $samples }
function Avg($xs, $f) { $v = $xs | Where-Object { $_.$f -ne $null } | ForEach-Object { $_.$f }; if ($v) { [math]::Round(($v | Measure-Object -Average).Average, 1) } else { 'n/a' } }
Add '## The PC, sampled every 2 s (from 120 s on)'
Add ''
Add '| what | average |'
Add '|---|---|'
Add "| CPU of the game (all $cores logical CPUs = 100 %) | $(Avg $world 'cpu_pct') % |"
Add "| GPU busy (nvidia-smi) | $(Avg $world 'gpu_pct') % |"
Add "| Card memory in use | $(Avg $world 'vram_mb') MB |"
Add "| Game memory: private / in RAM | $(Avg $world 'private_mb') / $(Avg $world 'ws_mb') MB |"
Add "| Free RAM | $(Avg $world 'free_mb') MB |"
Add "| Pages read back from disk | $(Avg $world 'pages_in_s') /s |"
Add ''

# Frame line and the card's clock, from the log.
Add '## Frame, the card and what the layer adds (last lines of the log)'
Add ''
foreach ($pat in ' fps, ', "the card's own stamps per frame", "the card's clock per frame", 'preparing per frame, the submitting thread', 'our passes over copies in the card', 'asking about title memory per frame', "the pass over the heap's views per frame", 'texture refreshes per frame', 'bytes of host copies over the bus per frame') {
    $m = Last $pat
    if ($m) { Add ('- ' + (Bare $m)); Add '' }
}

# Threads: names from the timeline, CPU seconds from the samples.
$names = @{}
$json = Get-ChildItem $Out -Filter 'timeline*.json' | Select-Object -First 1
if ($json) {
    $tl = Get-Content $json.FullName -Raw | ConvertFrom-Json
    $events = if ($tl.traceEvents) { $tl.traceEvents } else { $tl }
    foreach ($e in $events | Where-Object { $_.ph -eq 'M' -and $_.name -eq 'thread_name' }) { $names[[int]$e.tid] = $e.args.name }
    # Time inside each span name, summed over the frames recorded.
    $spans = $events | Where-Object { $_.ph -eq 'X' -and $_.dur } | Group-Object name | ForEach-Object {
        [pscustomobject]@{ name = $_.Name; ms = [math]::Round(($_.Group | Measure-Object dur -Sum).Sum / 1000, 1); n = $_.Count }
    } | Sort-Object ms -Descending
    Add "## Where the time went in the $Frames frames of the timeline (all threads, ms)"
    Add ''
    Add '| span | ms | times |'
    Add '|---|---|---|'
    foreach ($s in $spans | Select-Object -First 30) { Add "| $($s.name) | $($s.ms) | $($s.n) |" }
    Add ''
}
$total = ($threadCpu.Values | Measure-Object -Sum).Sum
Add '## CPU time by thread over the session (top 20)'
Add ''
Add '| thread | name | CPU s | share |'
Add '|---|---|---|---|'
foreach ($kv in $threadCpu.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 20) {
    $n = if ($names.ContainsKey([int]$kv.Key)) { $names[[int]$kv.Key] } else { '(the title or system)' }
    # The title's own threads are named after its image and their entry.
    $n = $n -replace '^victory-image-[0-9a-f]+\+', "the title's code, entry +"
    Add "| $($kv.Key) | $n | $([math]::Round($kv.Value, 1)) | $([math]::Round(100 * $kv.Value / [math]::Max($total, 1), 1)) % |"
}
Add ''

# Each frame's critical chain, from the timeline's own summary.
$txt = Get-ChildItem $Out -Filter 'timeline*.txt' | Select-Object -First 1
if ($txt) { Add '## The chain each frame came out of (timeline summary)'; Add ''; Get-Content $txt.FullName -TotalCount 60 | ForEach-Object { Add ('    ' + $_) }; Add '' }

# A timing run (-More @{VICTORY_GPU_TIMING='1'}; by pipeline with
# VICTORY_D3D12_DRED='1' too): the card's time by kind of list and pipeline, from the last such block of the log. Timing
# sends each list on its own, so the frame is slower; the shares hold.
$byKind = [array]::FindLastIndex($whole.ToArray(), [Predicate[string]] { param($l) $l -match "the card's time by command \(timing run\), all" })
$m = Last "the card's time by list \(timing run\)"
if ($m) { Add "## The card's time by kind of list (timing run)"; Add ''; Add ('- ' + (Bare $m).Trim()); Add '' }
if ($byKind -ge 0) {
    Add "## The card's time by pipeline (timing run with VICTORY_D3D12_DRED=1)"
    Add ''
    Add ('- ' + (Bare $whole[$byKind]).Trim())
    Add ''
    Add '| ms a frame | commands a frame | longest ms | what ran |'
    Add '|---|---|---|---|'
    for ($i = $byKind + 1; $i -lt $whole.Count -and $i -le $byKind + 60; $i++) {
        $c = [regex]::Match($whole[$i], "by command \(timing run\): ([\d.]+) ms a frame in ([\d.]+) commands a frame, the longest ([\d.]+) ms: (.*)")
        if ($c.Success) { Add "| $($c.Groups[1].Value) | $($c.Groups[2].Value) | $($c.Groups[3].Value) | $($c.Groups[4].Value -replace '\|', '/') |" }
    }
    Add ''
}

# Memory as the layer counts it.
Add '## Memory as the layer counts it (last lines of the log)'
Add ''
foreach ($pat in 'copystat: memory: .*MB private', 'copystat: memory of textures:', 'copystat: memory in blocks', 'copystat: memory, what holds it', 'copystat: memory let go since') {
    $m = Last $pat
    if ($m) { Add ('- ' + (Bare $m)); Add '' }
}
$report = Join-Path $Out 'report.md'
[IO.File]::WriteAllText($report, $r.ToString())
Write-Host "Report: $report"
Write-Host "Timeline for https://ui.perfetto.dev : $($json.FullName)"
