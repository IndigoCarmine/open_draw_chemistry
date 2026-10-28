# Copy an embedded ChemDraw object out of Word and PowerPoint and paste it back here: this app's
# copy of benzene (put on the clipboard by the test binary) is pasted into a new, windowless Word document and a
# new PowerPoint slide, the embedded object is copied from each with .Copy(), and this app's
# paste (`ole::paste`, run from the test binary) must read the same structure back. Windows only.
#
#   cargo test --lib --no-run      # note the test binary it prints (…\deps\open_draw_chemistry-….exe)
#   powershell -File scripts/e2e/office-copy-paste.ps1 -Test <test binary> [-Dump]
#
#   -Dump : also print every clipboard format and the object's storage layout after each copy.
#
# Works only on new, windowless documents, and quits Word/PowerPoint only when this script
# started them (the user's documents are untouched). It overwrites the clipboard.
# Exits with 1 if a paste did not bring back the structure that was copied.
param([Parameter(Mandatory = $true)][string]$Test, [switch]$Dump)
$ErrorActionPreference = 'Stop'

function Get-Ids($name) { @(Get-Process $name -ErrorAction SilentlyContinue | ForEach-Object Id) }
function Release($o) { if ($o) { [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($o) } }
function Run-Test($name) {
    $out = & $Test --ignored --exact $name --nocapture --test-threads=1 2>&1 | ForEach-Object { "$_" }
    # With --nocapture the first line printed follows the test's name on its line.
    $out | ForEach-Object { $_ -replace '^test \S+ \.\.\. ', '' } |
        Where-Object { $_ -match '^(format|  |Embed|Embedded|app text|cdx|embedded|pasted|error)' }
}
# Print what this app's paste makes of the clipboard; returns whether it matches the reference.
function Check-Paste($label) {
    if ($Dump) {
        Run-Test 'ole::paste::tests::dump_live_clipboard' | ForEach-Object { Write-Host "${label}: $_" }
    }
    $line = Run-Test 'ole::tests::paste_live_clipboard_into_editor' | Select-Object -Last 1
    Write-Host "${label}: $line"
    return [bool]($line -match "pasted=True atoms=$($script:atoms) bonds=$($script:bonds) ")
}
# This app's copy of benzene, as Ctrl+C on the canvas publishes it.
function Copy-FromApp {
    & $Test --ignored --exact ole::clipboard::tests::put_benzene_on_clipboard --test-threads=1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "could not put the app's copy on the clipboard" }
}
$ok = $true

# What this app's own copy holds: the reference counts.
Copy-FromApp
$reference = Run-Test 'ole::tests::paste_live_clipboard_into_editor' | Select-Object -Last 1
"app copy: $reference"
if ($reference -notmatch 'pasted=True atoms=(\d+) bonds=(\d+)') { "no structure on the clipboard"; exit 1 }
$script:atoms = $Matches[1]; $script:bonds = $Matches[2]

$before = Get-Ids WINWORD
$word = New-Object -ComObject Word.Application
$newWord = @(Get-Ids WINWORD | Where-Object { $before -notcontains $_ }).Count -gt 0
try {
    if ($newWord) { $word.Visible = $false; $word.DisplayAlerts = 0 }
    $doc = $word.Documents.Add([Type]::Missing, $false, 0, $false)
    try {
        $doc.Content.Paste()
        $shape = $doc.InlineShapes | Where-Object { $_.Type -eq 1 } | Select-Object -First 1
        if (-not $shape) { "word: FAIL (nothing embedded to copy)"; $ok = $false }
        else {
            "word: copying inline shape progid=$($shape.OLEFormat.ProgID)"
            $shape.Range.Copy()
            if (-not (Check-Paste 'word')) { "word: FAIL"; $ok = $false }
        }
    } finally { $doc.Close([ref]0); Release $doc }
} finally {
    if ($newWord) { $word.Quit([ref]0) }
    Release $word
}

# Word's copy goes when Word does: start again from the app's.
Copy-FromApp
$before = Get-Ids POWERPNT
$ppt = New-Object -ComObject PowerPoint.Application
$newPpt = @(Get-Ids POWERPNT | Where-Object { $before -notcontains $_ }).Count -gt 0
try {
    $pres = $ppt.Presentations.Add(0)  # WithWindow = msoFalse
    try {
        $slide = $pres.Slides.Add(1, 12)  # ppLayoutBlank
        [void]$slide.Shapes.Paste()
        $shape = $slide.Shapes | Where-Object { $_.Type -eq 7 } | Select-Object -First 1
        if (-not $shape) { "ppt: FAIL (nothing embedded to copy)"; $ok = $false }
        else {
            "ppt: copying shape progid=$($shape.OLEFormat.ProgID)"
            $shape.Copy()
            if (-not (Check-Paste 'ppt')) { "ppt: FAIL"; $ok = $false }
        }
    } finally { $pres.Close(); Release $pres }
} finally {
    if ($newPpt) { $ppt.Quit() }
    Release $ppt
}
[GC]::Collect()
[GC]::WaitForPendingFinalizers()
if (-not $ok) { exit 1 }
"ok"
