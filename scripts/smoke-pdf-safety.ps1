[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

function Assert-NativeSuccess([string]$Step) {
    if ($LASTEXITCODE -ne 0) {
        throw "$Step failed with exit code $LASTEXITCODE."
    }
}

$projectDir = Split-Path -Parent $PSScriptRoot
$temporaryRoot = [System.IO.Path]::GetTempPath()
$workDir = Join-Path $temporaryRoot ("litman-safety-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $workDir | Out-Null

try {
    Push-Location $projectDir
    try {
        $pdfRoot = Join-Path $workDir 'pdfs'
        cargo run -p litman-core --example generate_fixtures -- $pdfRoot
        Assert-NativeSuccess 'Fixture generation'
        cargo build -p litman-cli --locked
        Assert-NativeSuccess 'CLI build'

        $before = @{}
        Get-ChildItem -LiteralPath $pdfRoot -Filter '*.pdf' -File | ForEach-Object {
            $before[$_.Name] = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
        }

        $config = Join-Path $workDir 'library.toml'
        $cli = Join-Path $projectDir 'target\debug\litman.exe'
        & $cli init --config $config --root $pdfRoot --language zh-CN
        Assert-NativeSuccess 'Library initialization'
        & $cli --config $config scan
        Assert-NativeSuccess 'Library scan'
        $search = (& $cli --config $config search '中文' --format json) -join "`n"
        Assert-NativeSuccess 'Chinese search'
        if (-not $search.Contains('中文文献管理')) {
            throw 'Chinese fixture was not returned by search.'
        }

        foreach ($entry in $before.GetEnumerator()) {
            $path = Join-Path $pdfRoot $entry.Key
            $after = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
            if ($after -ne $entry.Value) {
                throw "Ordinary operation changed $($entry.Key)."
            }
        }

        $publisher = Join-Path $workDir 'publisher.pdf'
        Copy-Item -LiteralPath (Join-Path $pdfRoot 'xmp-prism.pdf') -Destination $publisher
        $selectedHash = (Get-FileHash -LiteralPath (Join-Path $pdfRoot 'info-only.pdf') -Algorithm SHA256).Hash
        $publisherHash = (Get-FileHash -LiteralPath $publisher -Algorithm SHA256).Hash
        cargo run -p litman-core --example replacement_smoke -- $config 'info-only.pdf' $publisher
        Assert-NativeSuccess 'Explicit replacement smoke test'

        $backup = Join-Path $pdfRoot 'LitMan-backups\2008MNRAS.386..619C_bk.pdf'
        $active = Join-Path $pdfRoot '2008MNRAS.386..619C.pdf'
        if ((Get-FileHash -LiteralPath $backup -Algorithm SHA256).Hash -ne $selectedHash) {
            throw 'Replacement backup hash did not match the selected PDF.'
        }
        if ((Get-FileHash -LiteralPath $active -Algorithm SHA256).Hash -ne $publisherHash) {
            throw 'Installed publisher hash did not match the selected download.'
        }
        if ((Get-FileHash -LiteralPath $publisher -Algorithm SHA256).Hash -ne $publisherHash) {
            throw 'Replacement changed the selected external source file.'
        }
        foreach ($entry in $before.GetEnumerator()) {
            if ($entry.Key -eq 'info-only.pdf') {
                continue
            }
            $path = Join-Path $pdfRoot $entry.Key
            if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $entry.Value) {
                throw "Replacement changed unrelated PDF $($entry.Key)."
            }
        }
        Write-Host 'Ordinary PDF immutability and explicit replacement safety smoke tests passed'
    }
    finally {
        Pop-Location
    }
}
finally {
    $resolvedWork = [System.IO.Path]::GetFullPath($workDir)
    $resolvedTemporary = [System.IO.Path]::GetFullPath($temporaryRoot)
    if (-not $resolvedWork.StartsWith($resolvedTemporary, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not ([System.IO.Path]::GetFileName($resolvedWork)).StartsWith('litman-safety-', [System.StringComparison]::Ordinal)) {
        throw "Refusing to remove unexpected smoke-test directory: $resolvedWork"
    }
    Remove-Item -LiteralPath $resolvedWork -Recurse -Force
}
