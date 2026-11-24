param([string]$Destination)
$ErrorActionPreference='Stop'
$project=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $project
try {
    if (!$Destination) { $Destination=Join-Path $project 'dist' }
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    $Destination=(Resolve-Path $Destination).Path
    $meta = cargo metadata --format-version 1 --no-deps --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $package=$meta.packages | Where-Object name -eq 'bastion-bridge-gpui'
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Rust release build failed' }
    $stage=Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString())
    New-Item -ItemType Directory $stage | Out-Null
    try {
        Copy-Item (Join-Path $meta.target_directory 'release/bastion-bridge-gpui.exe') (Join-Path $stage 'BastionTerm.exe')
        Copy-Item LICENSE,*.md,assets/OFL.txt,assets/LICENSE-GPUI,assets/LICENSE-vt100,assets/LICENSE-ICU4X,assets/LICENSE-muda,assets/browser/LICENSE-xterm,assets/browser/LICENSE-fit $stage
        python scripts/dependency-notices.py x86_64-pc-windows-msvc (Join-Path $stage 'DEPENDENCY-NOTICES.txt')
        if ($LASTEXITCODE -ne 0) { throw 'Dependency notices failed' }
        Copy-Item vendor/vte/LICENSE-APACHE (Join-Path $stage 'LICENSE-vte-apache')
        Copy-Item vendor/vte/LICENSE-MIT (Join-Path $stage 'LICENSE-vte')
        $archive=Join-Path $Destination ("bastionterm-"+$package.version+"-windows-x64.zip")
        Compress-Archive "$stage/*" $archive -Force
        Get-FileHash -Algorithm SHA256 $archive
    } finally { Remove-Item -LiteralPath $stage -Recurse -Force }
} finally { Pop-Location }
