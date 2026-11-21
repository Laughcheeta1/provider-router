[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $Binary,
    [Parameter(Mandatory)][string] $Output,
    [string] $Version,
    [string] $Compiler,
    [string] $SignCommand
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-RouterArchitecture([string] $Path) {
    $reader = [IO.BinaryReader]::new([IO.File]::OpenRead($Path))
    try {
        if ($reader.BaseStream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5A4D) {
            throw 'The payload is not a Windows executable.'
        }
        $reader.BaseStream.Position = 0x3C
        $peOffset = $reader.ReadInt32()
        if ($peOffset -lt 64 -or $peOffset -gt ($reader.BaseStream.Length - 6)) {
            throw 'The payload has an invalid PE header offset.'
        }
        $reader.BaseStream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) {
            throw 'The payload has an invalid PE signature.'
        }
        switch ($reader.ReadUInt16()) {
            0x8664 { return 'x64os' }
            0xAA64 { return 'arm64' }
            default { throw 'Build an x64 or ARM64 Provider Router executable.' }
        }
    }
    finally {
        $reader.Dispose()
    }
}

if (-not $Version) {
    $manifest = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot '../../Cargo.toml')
    $match = [regex]::Match($manifest, '(?m)^version\s*=\s*"([^"]+)"')
    $Version = $match.Groups[1].Value
}
if ($Version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
    throw 'Use a release version in major.minor.patch format.'
}

if (-not $Compiler) {
    $command = Get-Command ISCC.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($command) {
        $Compiler = $command.Source
    }
    else {
        foreach ($programFiles in @(${env:ProgramFiles(x86)}, $env:ProgramFiles)) {
            if (-not $programFiles) { continue }
            foreach ($directory in @('Inno Setup 6', 'Inno Setup 7')) {
                $candidate = Join-Path $programFiles "$directory\ISCC.exe"
                if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                    $Compiler = $candidate
                    break
                }
            }
            if ($Compiler) { break }
        }
    }
}
if (-not $Compiler) {
    throw 'Install Inno Setup 6.4 or later, or pass the path to ISCC.exe with -Compiler.'
}
$compilerFile = Get-Item -LiteralPath $Compiler
if ($SignCommand -and -not $SignCommand.Contains('$f')) {
    throw 'The Inno Setup signing command must contain the $f file placeholder.'
}

$payload = (Get-Item -LiteralPath $Binary).FullName
$architecture = Get-RouterArchitecture $payload
$destination = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Output)
if ([IO.Path]::GetExtension($destination) -ine '.exe') {
    throw 'Pass the full .exe output path with -Output.'
}
if ($destination -ieq $payload) {
    throw 'The installer output must be different from the router executable.'
}
$outputDirectory = [IO.Path]::GetDirectoryName($destination)
$outputName = [IO.Path]::GetFileNameWithoutExtension($destination)
$null = [IO.Directory]::CreateDirectory($outputDirectory)
$staging = Join-Path ([IO.Path]::GetTempPath()) ('provider-router-inno-' + [IO.Path]::GetRandomFileName())
$null = New-Item -ItemType Directory -Path $staging
try {
    $stagedPayload = Join-Path $staging 'provider-router-payload.exe'
    Copy-Item -LiteralPath $payload -Destination $stagedPayload
    $arguments = @("/DRouterVersion=$Version", "/DRouterArchitecture=$architecture",
        "/DRouterPayload=$stagedPayload", "/O$outputDirectory", "/F$outputName")
    if ($SignCommand) {
        $arguments += @('/DRouterSigning=1', "/SproviderRouter=$SignCommand")
    }
    & $compilerFile.FullName @arguments (Join-Path $PSScriptRoot 'provider-router.iss')
    if ($LASTEXITCODE -ne 0) {
        throw "Inno Setup failed with exit code $LASTEXITCODE."
    }
}
finally {
    Remove-Item -LiteralPath $staging -Recurse -Force
}
