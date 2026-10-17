param(
    [string]$ManifestPath = (Join-Path $PSScriptRoot '..\Cargo.toml')
)

$ErrorActionPreference = 'Stop'

function Assert-Equal([object]$Actual, [object]$Expected, [string]$Label) {
    if ($Actual -ne $Expected) {
        throw "$Label expected '$Expected', got '$Actual'"
    }
}

function Read-Response([System.Diagnostics.Process]$Process, [int]$Id, [string]$Name) {
    $read = $Process.StandardOutput.ReadLineAsync()
    if (-not $read.Wait(10000)) {
        throw "$Name did not respond within 10 seconds"
    }
    $line = $read.GetAwaiter().GetResult()
    if ([string]::IsNullOrWhiteSpace($line)) {
        throw "$Name closed stdout before response $Id"
    }
    $response = $line | ConvertFrom-Json
    Assert-Equal $response.id $Id "$Name response id"
    return $response
}

function Invoke-McpSmoke([string]$Binary, [string]$ExpectedName, [string]$TargetDirectory) {
    $executable = Join-Path $TargetDirectory "$Binary.exe"
    if (-not (Test-Path $executable)) {
        throw "Missing built binary: $executable"
    }

    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $executable
    $start.UseShellExecute = $false
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    [void]$process.Start()

    try {
        $process.StandardInput.WriteLine('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"w18-mcp-smoke","version":"1"}}}')
        $process.StandardInput.Flush()
        $initialize = Read-Response $process 1 $Binary
        Assert-Equal $initialize.result.protocolVersion '2025-03-26' "$Binary protocol version"
        Assert-Equal $initialize.result.serverInfo.name $ExpectedName "$Binary server identity"
        if ($null -eq $initialize.result.capabilities.tools) {
            throw "$Binary did not advertise tools capability"
        }

        $process.StandardInput.WriteLine('{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}')
        $process.StandardInput.WriteLine('{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}')
        $process.StandardInput.Flush()
        $tools = Read-Response $process 2 $Binary
        if ($null -eq $tools.result.tools -or $tools.result.tools.Count -lt 1) {
            throw "$Binary returned no tools"
        }

        $process.StandardInput.Close()
        if (-not $process.WaitForExit(10000)) {
            throw "$Binary did not shut down within 10 seconds after stdin close"
        }
        $stderr = $process.StandardError.ReadToEnd()
        if ($process.ExitCode -ne 0) {
            throw "$Binary unexpected shutdown: exit $($process.ExitCode); stderr: $stderr"
        }
        Write-Output "${Binary}: initialize + initialized + tools/list passed; clean EOF exit 0"
    }
    finally {
        $process.Dispose()
    }
}

try {
    $manifest = (Resolve-Path $ManifestPath).Path
    & cargo build --manifest-path $manifest --all-features --bin peek-mcp --bin compact-terminal-mcp
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit $LASTEXITCODE"
    }

    $target = Join-Path (Split-Path $manifest -Parent) 'target\debug'
    Invoke-McpSmoke 'peek-mcp' 'peek-mcp' $target
    Invoke-McpSmoke 'compact-terminal-mcp' 'compact-terminal-mcp' $target
}
catch {
    Write-Error $_
    exit 1
}
