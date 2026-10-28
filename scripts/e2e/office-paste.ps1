# Paste the clipboard into a new Word document and a new PowerPoint slide with a plain Paste
# (not Paste Special), and check that both embed it as a ChemDraw OLE object. Windows only.
#
#   cargo test --lib ole::clipboard::tests::put_benzene_on_clipboard -- --ignored
#   powershell -File scripts/e2e/office-paste.ps1 -Out $env:TEMP\paste [-Activate]
#
# Works only on new, windowless documents, and quits Word/PowerPoint only when this script
# started them (PowerPoint attaches to a running instance; the user's documents are untouched).
# The pasted files are saved as <Out>.docx and <Out>.pptx for inspection.
#   -Activate : also have PowerPoint load the object through its server (ChemDraw) and print the
#               SMILES ChemDraw reads from it.
# Exits with 1 if either program did not embed a ChemDraw object.
param([Parameter(Mandatory = $true)][string]$Out, [switch]$Activate)
$ErrorActionPreference = 'Stop'

function Get-Ids($name) { @(Get-Process $name -ErrorAction SilentlyContinue | ForEach-Object Id) }
function Release($o) { if ($o) { [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($o) } }
$ok = $true

$before = Get-Ids WINWORD
$word = New-Object -ComObject Word.Application
$newWord = @(Get-Ids WINWORD | Where-Object { $before -notcontains $_ }).Count -gt 0
try {
    if ($newWord) { $word.Visible = $false; $word.DisplayAlerts = 0 }
    # Documents.Add(Template, NewTemplate, DocumentType, Visible = false)
    $doc = $word.Documents.Add([Type]::Missing, $false, 0, $false)
    try {
        $doc.Content.Paste()
        $embedded = $false
        foreach ($s in $doc.InlineShapes) {
            $prog = ''
            try { $prog = $s.OLEFormat.ProgID } catch {}
            "word: inline shape type=$($s.Type) progid=$prog size=$([math]::Round($s.Width, 1))x$([math]::Round($s.Height, 1))pt"
            # 1 = wdInlineShapeEmbeddedOLEObject
            if ($s.Type -eq 1 -and $prog -like 'ChemDraw*') { $embedded = $true }
        }
        if (-not $embedded) { "word: FAIL (no embedded ChemDraw object)"; $ok = $false }
        $doc.SaveAs2("$Out.docx", 16)
    } finally { $doc.Close([ref]0); Release $doc }
} finally {
    if ($newWord) { $word.Quit([ref]0) }
    Release $word
}

$before = Get-Ids POWERPNT
$ppt = New-Object -ComObject PowerPoint.Application
$newPpt = @(Get-Ids POWERPNT | Where-Object { $before -notcontains $_ }).Count -gt 0
try {
    $pres = $ppt.Presentations.Add(0)  # WithWindow = msoFalse
    try {
        $slide = $pres.Slides.Add(1, 12)  # ppLayoutBlank
        [void]$slide.Shapes.Paste()
        $embedded = $false
        foreach ($s in $slide.Shapes) {
            $prog = ''
            try { $prog = $s.OLEFormat.ProgID } catch {}
            "ppt: shape type=$($s.Type) progid=$prog size=$([math]::Round($s.Width, 1))x$([math]::Round($s.Height, 1))pt"
            # 7 = msoEmbeddedOLEObject
            if ($s.Type -eq 7 -and $prog -like 'ChemDraw*') {
                $embedded = $true
                if ($Activate) {
                    try {
                        $object = $s.OLEFormat.Object
                        "ppt: ChemDraw reads SMILES $($object.Objects.Data('chemical/x-smiles'))"
                        Release $object
                    } catch { "ppt: activation failed: $($_.Exception.Message)"; $ok = $false }
                }
            }
        }
        if (-not $embedded) { "ppt: FAIL (no embedded ChemDraw object)"; $ok = $false }
        $pres.SaveAs("$Out.pptx")
    } finally { $pres.Close(); Release $pres }
} finally {
    if ($newPpt) { $ppt.Quit() }
    Release $ppt
}
[GC]::Collect()
[GC]::WaitForPendingFinalizers()
if (-not $ok) { exit 1 }
"ok"
