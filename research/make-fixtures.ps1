$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
New-Item -ItemType Directory -Force research/fixtures | Out-Null
foreach ($case in @(@('1080p','1920x1080'), @('4k','3840x2160'))) {
    & ffmpeg -v error -f lavfi -i "testsrc2=size=$($case[1]):rate=30" -t 4 -c:v libx264 -preset fast -crf 18 -pix_fmt yuv420p -y "research/fixtures/motion_$($case[0]).mp4"
    if ($LASTEXITCODE -ne 0) { throw 'Fixture generation failed' }
}
