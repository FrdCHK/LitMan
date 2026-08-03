[CmdletBinding()]
param(
    [Parameter()][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version = '0.1.5',
    [Parameter()][switch]$Sign,
    [Parameter()][switch]$SkipValidation,
    [Parameter()][string]$WixBin
)

$ErrorActionPreference = 'Stop'

function Assert-NativeSuccess([string]$Step) {
    if ($LASTEXITCODE -ne 0) {
        throw "$Step failed with exit code $LASTEXITCODE."
    }
}

function Copy-FileWithRetry([string]$Source, [string]$Destination) {
    for ($attempt = 1; $attempt -le 10; $attempt++) {
        try {
            Copy-Item -LiteralPath $Source -Destination $Destination -Force -ErrorAction Stop
            return
        }
        catch {
            if ($attempt -eq 10) {
                throw
            }
            Start-Sleep -Milliseconds 250
        }
    }
}

function Resolve-WixTool([string]$Name) {
    if (-not [string]::IsNullOrWhiteSpace($WixBin)) {
        $candidate = Join-Path $WixBin "$Name.exe"
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            throw "WiX tool was not found: $candidate"
        }
        return $candidate
    }
    return (Get-Command "$Name.exe" -ErrorAction Stop).Source
}

function New-DeterministicGuid([string]$Purpose) {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hash = $sha256.ComputeHash([System.Text.Encoding]::UTF8.GetBytes("LitMan MSI $Purpose"))
        $bytes = New-Object byte[] 16
        [Array]::Copy($hash, $bytes, 16)
        return (New-Object Guid (,$bytes)).ToString('D').ToUpperInvariant()
    }
    finally {
        $sha256.Dispose()
    }
}

function Get-SourceRevision([string]$ProjectDir) {
    $sourceEntries = @(
        'Cargo.toml', 'Cargo.lock', 'LICENSE', 'README.md', 'rust-toolchain.toml',
        'crates', 'docs\en\src', 'docs\zh-CN\src', 'docs\en\book.toml',
        'docs\zh-CN\book.toml', 'packaging', 'scripts'
    )
    $files = foreach ($entry in $sourceEntries) {
        $path = Join-Path $ProjectDir $entry
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            Get-Item -LiteralPath $path
        }
        elseif (Test-Path -LiteralPath $path -PathType Container) {
            Get-ChildItem -LiteralPath $path -Recurse -File
        }
    }
    $records = foreach ($file in ($files | Sort-Object FullName)) {
        $relativePath = $file.FullName.Substring($ProjectDir.Length).TrimStart('\')
        $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
        "$relativePath=$hash"
    }
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [System.Text.Encoding]::UTF8.GetBytes(($records -join "`n"))
        return ([System.BitConverter]::ToString($sha256.ComputeHash($bytes))).Replace('-', '')
    }
    finally {
        $sha256.Dispose()
    }
}

$projectDir = Split-Path -Parent $PSScriptRoot
$distDir = Join-Path $projectDir 'dist'
$wixBuildDir = Join-Path $projectDir 'target\packaging\windows'

New-Item -ItemType Directory -Force -Path $distDir, $wixBuildDir | Out-Null
Push-Location $projectDir
try {
    cargo build --workspace --release --locked
    Assert-NativeSuccess 'Cargo release build'
    mdbook build docs/en
    Assert-NativeSuccess 'English manual build'
    mdbook build docs/zh-CN
    Assert-NativeSuccess 'Simplified Chinese manual build'

    $portableExe = Join-Path $distDir "LitMan-$Version-portable-x64.exe"
    Copy-FileWithRetry (Join-Path $projectDir 'target\release\litman-gui.exe') $portableExe

    $portableStage = Join-Path $wixBuildDir "portable-$Version"
    if (Test-Path -LiteralPath $portableStage) {
        Remove-Item -LiteralPath $portableStage -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path `
        $portableStage, `
        (Join-Path $portableStage 'manual\en'), `
        (Join-Path $portableStage 'manual\zh-CN') | Out-Null
    Copy-FileWithRetry (Join-Path $projectDir 'target\release\litman-gui.exe') (Join-Path $portableStage 'LitMan.exe')
    Copy-FileWithRetry (Join-Path $projectDir 'target\release\litman.exe') (Join-Path $portableStage 'litman-cli.exe')
    Copy-Item -LiteralPath (Join-Path $projectDir 'LICENSE') -Destination $portableStage
    Copy-Item -LiteralPath (Join-Path $projectDir 'crates\litman-gui\assets\LICENSE-NOTO.txt') -Destination $portableStage
    Copy-Item -Path (Join-Path $projectDir 'docs\en\book\*') -Destination (Join-Path $portableStage 'manual\en') -Recurse
    Copy-Item -Path (Join-Path $projectDir 'docs\zh-CN\book\*') -Destination (Join-Path $portableStage 'manual\zh-CN') -Recurse
    $portableZip = Join-Path $distDir "LitMan-$Version-portable-x64.zip"
    if (Test-Path -LiteralPath $portableZip) {
        Remove-Item -LiteralPath $portableZip -Force
    }
    Compress-Archive -Path (Join-Path $portableStage '*') -DestinationPath $portableZip -CompressionLevel Optimal

    $heat = Resolve-WixTool 'heat'
    $candle = Resolve-WixTool 'candle'
    $light = Resolve-WixTool 'light'
    $enFragment = Join-Path $wixBuildDir 'manual-en.wxs'
    $zhFragment = Join-Path $wixBuildDir 'manual-zh-CN.wxs'

    & $heat dir (Join-Path $projectDir 'docs\en\book') -nologo -cg EnglishManual -dr DocsEn -srd -ag -sfrag -var var.EnglishManualDir -out $enFragment
    Assert-NativeSuccess 'English manual harvesting'
    & $heat dir (Join-Path $projectDir 'docs\zh-CN\book') -nologo -cg ChineseManual -dr DocsZh -srd -ag -sfrag -var var.ChineseManualDir -out $zhFragment
    Assert-NativeSuccess 'Simplified Chinese manual harvesting'

    $wixObjects = @(
        (Join-Path $wixBuildDir 'main.wixobj'),
        (Join-Path $wixBuildDir 'manual-en.wixobj'),
        (Join-Path $wixBuildDir 'manual-zh-CN.wixobj')
    )
    $wixOutput = $wixBuildDir.TrimEnd('\') + '\'
    $sourceRevision = Get-SourceRevision $projectDir
    $productCode = New-DeterministicGuid "product $Version source $sourceRevision"
    & $candle -nologo -arch x64 `
        "-dVersion=$Version" `
        "-dProductCode=$productCode" `
        "-dProjectDir=$projectDir" `
        "-dSourceDir=$(Join-Path $projectDir 'target\release')" `
        "-dEnglishManualDir=$(Join-Path $projectDir 'docs\en\book')" `
        "-dChineseManualDir=$(Join-Path $projectDir 'docs\zh-CN\book')" `
        -out $wixOutput `
        (Join-Path $projectDir 'packaging\windows\main.wxs') $enFragment $zhFragment
    Assert-NativeSuccess 'WiX compilation'

    $msi = Join-Path $distDir "LitMan-$Version-x64.msi"
    $lightArguments = @('-nologo', '-ext', 'WixUIExtension', '-cultures:en-us')
    if ($SkipValidation) {
        $lightArguments += '-sval'
    }
    $lightArguments += @('-out', $msi)
    $lightArguments += $wixObjects
    & $light @lightArguments
    Assert-NativeSuccess 'MSI linking'

    if ($Sign) {
        $thumbprint = $env:LITMAN_CERT_THUMBPRINT
        if ([string]::IsNullOrWhiteSpace($thumbprint)) {
            throw 'Set LITMAN_CERT_THUMBPRINT before using -Sign.'
        }
        $timestampUrl = if ($env:LITMAN_TIMESTAMP_URL) { $env:LITMAN_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
        & signtool.exe sign /sha1 $thumbprint /fd SHA256 /tr $timestampUrl /td SHA256 $msi
        Assert-NativeSuccess 'Authenticode signing'
    }

    Get-FileHash -Algorithm SHA256 $msi, $portableExe, $portableZip
}
finally {
    Pop-Location
}
