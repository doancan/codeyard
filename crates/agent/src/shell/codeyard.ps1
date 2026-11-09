# Codeyard shell integration (Windows PowerShell 5.1 and PowerShell 7). Passed with -NoExit -EncodedCommand, so it runs
# after the user's profile; it only wraps `prompt` and PSReadLine's PSConsoleHostReadLine, prints nothing of its own and
# keeps $LASTEXITCODE. The editor counts a command as finished (133;D) only after it saw it start (133;C).
try {
    if ($env:TERM_PROGRAM -ne 'codeyard' -or $Global:__CyLoaded) { return }
    $Global:__CyLoaded = $true
    $Global:__CyRunning = $false
    $Global:__CyPrompt = $function:prompt

    function Global:prompt {
        $ok = $?
        $code = $Global:LASTEXITCODE
        $esc = [char]27
        $bel = [char]7
        $marks = ''
        if ($Global:__CyRunning) {
            # A program's own exit code when it set one; 1 for a failed cmdlet. $LASTEXITCODE outlives the program that
            # set it: a failure with a new error record and the old code is the cmdlet's.
            $newError = $Global:Error.Count -gt 0 -and -not [object]::ReferenceEquals($Global:Error[0], $Global:__CyLastError)
            $native = $code -is [int] -and $code -ne 0 -and ($code -ne $Global:__CyExitBefore -or -not $newError)
            $status = if ($ok) { 0 } elseif ($native) { $code } else { 1 }
            $marks += "$esc]133;D;$status$bel"
            $Global:__CyRunning = $false
        }
        $here = $ExecutionContext.SessionState.Path.CurrentLocation
        if ($here.Provider.Name -eq 'FileSystem') {
            # C:\Users\a b -> /C:/Users/a b; \\server\share -> //server/share. Only % needs escaping.
            $path = $here.ProviderPath.Replace('\', '/').Replace('%', '%25')
            if (-not $path.StartsWith('/')) { $path = "/$path" }
            $marks += "$esc]7;file://$env:COMPUTERNAME$path$bel"
        }
        $marks += "$esc]133;A$bel"
        $text = if ($Global:__CyPrompt) { (& $Global:__CyPrompt) -join '' } else { "PS $($here.Path)> " }
        $Global:LASTEXITCODE = $code
        $marks + $text
    }

    # PSReadLine reads the command line; without it (another host) prompts and folders are still marked.
    if (Test-Path Function:\PSConsoleHostReadLine) {
        $Global:__CyReadLine = $function:PSConsoleHostReadLine
        function Global:PSConsoleHostReadLine {
            $line = & $Global:__CyReadLine
            if ($line -match '\S') {
                [Console]::Write("$([char]27)]133;C$([char]7)")
                $Global:__CyRunning = $true
                $Global:__CyExitBefore = $Global:LASTEXITCODE
                $Global:__CyLastError = if ($Global:Error.Count -gt 0) { $Global:Error[0] } else { $null }
            }
            $line
        }
    }
} catch {
}
