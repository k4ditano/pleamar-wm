param([Parameter(Mandatory = $true)][string]$Path)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$bitmap = [System.Drawing.Bitmap]::new($Path)
try {
    $blue = 0
    $red = 0
    for ($y = 4; $y -lt $bitmap.Height; $y += 8) {
        for ($x = 4; $x -lt $bitmap.Width; $x += 8) {
            $pixel = $bitmap.GetPixel($x, $y)
            if ($pixel.R -lt 80 -and $pixel.G -lt 110 -and $pixel.B -gt 100) { $blue++ }
            if ($pixel.R -gt 140 -and $pixel.G -lt 100 -and $pixel.B -lt 100) { $red++ }
        }
    }
    # The owned fixture has a blue client and a draggable red rectangle. A
    # correctly sized blank PNG cannot stand in for its actual GDI rendering.
    if ($blue -lt 100 -or $red -lt 50) { throw "Owned fixture pixels missing: blue=$blue red=$red" }
    @{ blue_samples = $blue; red_samples = $red } | ConvertTo-Json -Compress
} finally {
    $bitmap.Dispose()
}
