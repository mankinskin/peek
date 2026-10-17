param(
    [string]$ManifestPath = (Join-Path $PSScriptRoot '..\Cargo.toml'),
    [int]$BuildTimeoutSeconds = 120,
    [int]$HandshakeTimeoutSeconds = 10,
    [string]$PeekServerName = 'peek-mcp'
)

$ErrorActionPreference = 'Stop'

function Limit-Text([string]$Text) {
    if ($Text.Length -gt 4000) { return $Text.Substring(0, 4000) + "`n[truncated]" }
    return $Text
}

function Assert-Equal([object]$Actual, [object]$Expected, [string]$Label) {
    if ($Actual -ne $Expected) { throw "$Label expected '$Expected', got '$Actual'" }
}

function Invoke-Cargo([string]$Arguments, [int]$TimeoutSeconds) {
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = 'cargo'
    $start.Arguments = $Arguments
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    [void]$process.Start()
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        Stop-Process -Id $process.Id -Force
        [void]$process.WaitForExit(5000)
        throw "cargo timed out after $TimeoutSeconds seconds (PID $($process.Id))"
    }
    $result = @{ ExitCode = $process.ExitCode; Stdout = $stdout.GetAwaiter().GetResult(); Stderr = $stderr.GetAwaiter().GetResult() }
    $process.Dispose()
    if ($result.ExitCode -ne 0) {
        throw "cargo failed with exit $($result.ExitCode): $(Limit-Text $result.Stderr)"
    }
    return $result
}

function Read-Response([System.Diagnostics.Process]$Process, [int]$Id, [string]$Name, [int]$TimeoutSeconds) {
    $read = $Process.StandardOutput.ReadLineAsync()
    if (-not $read.Wait($TimeoutSeconds * 1000)) { throw "$Name did not respond within $TimeoutSeconds seconds" }
    $line = $read.GetAwaiter().GetResult()
    if ([string]::IsNullOrWhiteSpace($line)) { throw "$Name closed stdout before response $Id" }
    $response = $line | ConvertFrom-Json
    Assert-Equal $response.id $Id "$Name response id"
    return $response
}

function Invoke-McpSmoke([string]$Binary, [string]$ExpectedName, [string]$TargetDirectory, [int]$TimeoutSeconds) {
    $executable = Join-Path $TargetDirectory "$Binary.exe"
    if (-not (Test-Path $executable)) { throw "Missing built binary: $executable" }

    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $executable
    $start.UseShellExecute = $false
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    [void]$process.Start()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $primary = $null
    $shutdown = ''

    try {
        $process.StandardInput.WriteLine('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"w18-mcp-smoke","version":"1"}}}')
        $process.StandardInput.Flush()
        $initialize = Read-Response $process 1 $Binary $TimeoutSeconds
        Assert-Equal $initialize.result.protocolVersion '2025-03-26' "$Binary protocol version"
        Assert-Equal $initialize.result.serverInfo.name $ExpectedName "$Binary server identity"
        if ($null -eq $initialize.result.capabilities.tools) { throw "$Binary did not advertise tools capability" }

        $process.StandardInput.WriteLine('{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}')
        $process.StandardInput.WriteLine('{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}')
        $process.StandardInput.Flush()
        $tools = Read-Response $process 2 $Binary $TimeoutSeconds
        if ($null -eq $tools.result.tools -or $tools.result.tools.Count -lt 1) { throw "$Binary returned no tools" }
    }
    catch {
        $primary = $_
    }
    finally {
        try { $process.StandardInput.Close() } catch {}
        if (-not $process.HasExited -and -not $process.WaitForExit($TimeoutSeconds * 1000)) {
            Stop-Process -Id $process.Id -Force
            [void]$process.WaitForExit(5000)
            $shutdown = "killed exact PID $($process.Id) after timeout"
        }
        elseif ($process.HasExited) {
            $shutdown = "exited $($process.ExitCode) after stdin close"
        }
        else {
            $shutdown = "exited $($process.ExitCode) after stdin close"
        }
        $stderr = if ($process.HasExited) { $stderrTask.GetAwaiter().GetResult() } else { '<stderr unavailable after forced termination>' }
        $process.Dispose()
    }

    if ($null -ne $primary) {
        throw "$($primary.Exception.Message)`n$Binary shutdown: $shutdown`nstderr: $(Limit-Text $stderr)"
    }
    if ($shutdown -ne 'exited 0 after stdin close') {
        throw "$Binary unexpected shutdown: $shutdown`nstderr: $(Limit-Text $stderr)"
    }
    Write-Output "${Binary}: initialize + initialized + tools/list passed; clean $shutdown"
}

try {
    $manifest = (Resolve-Path $ManifestPath).Path
    $quotedManifest = '"' + $manifest + '"'
    $metadataResult = Invoke-Cargo "metadata --manifest-path $quotedManifest --format-version 1 --no-deps" $BuildTimeoutSeconds
    $metadata = $metadataResult.Stdout | ConvertFrom-Json
    Invoke-Cargo "build --manifest-path $quotedManifest --all-features --bin peek-mcp --bin compact-terminal-mcp" $BuildTimeoutSeconds | Out-Null
    $target = Join-Path $metadata.target_directory 'debug'
    Invoke-McpSmoke 'peek-mcp' $PeekServerName $target $HandshakeTimeoutSeconds
    Invoke-McpSmoke 'compact-terminal-mcp' 'compact-terminal-mcp' $target $HandshakeTimeoutSeconds
}
catch {
    Write-Error $_
    exit 1
}
