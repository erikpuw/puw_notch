# PermissionRequest hook for Claude Code: shows the question on PUW Notch (Allow / Deny, or the
# options of a multiple-choice question) and answers for Claude. Anything else (notch not running,
# no click in time, "Trong app" pressed, a question the banner cannot hold) prints nothing, so
# Claude asks in its own window as usual. Do not name a variable $input here: PowerShell then takes
# the standard input for itself and the request never arrives.
$utf8 = New-Object System.Text.UTF8Encoding($false)

# One request to the notch: its answer line ('allow', 'deny', 'choice:N' or 'ask'), or $null.
function Ask-Notch($title, $detail, $labels) {
    $detail = ([string]$detail -replace '\s+', ' ').Trim()
    if ($detail.Length -gt 240) { $detail = $detail.Substring(0, 240) + '...' }
    $shown = @($labels | ForEach-Object { if ($_.Length -gt 12) { $_.Substring(0, 11) + '.' } else { $_ } })
    $pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', 'puw-notch-approve', [System.IO.Pipes.PipeDirection]::InOut)
    $pipe.Connect(300)
    $writer = New-Object System.IO.StreamWriter($pipe, $utf8)
    $reader = New-Object System.IO.StreamReader($pipe, $utf8)
    $message = @{ app = 'Claude'; title = $title; detail = $detail }
    if ($shown.Count) { $message.options = $shown }
    $writer.WriteLine(($message | ConvertTo-Json -Compress))
    $writer.Flush()
    return $reader.ReadLine()
}

try {
    [Console]::OutputEncoding = $utf8
    $stdin = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), $utf8)
    $request = $stdin.ReadToEnd() | ConvertFrom-Json
    $tool = [string]$request.tool_name
    if ($tool -eq 'ExitPlanMode') { exit 0 }

    if ($tool -eq 'AskUserQuestion') {
        # Each single-choice question with 2 or 3 options gets its banner, one after the other; the
        # banner's last button hands the question back to Claude's own window.
        $updated = $request.tool_input
        $answers = @{}
        foreach ($q in @($updated.questions)) {
            $labels = @(@($q.options) | ForEach-Object { [string]$_.label })
            if ($q.multiSelect -or $labels.Count -lt 2 -or $labels.Count -gt 3) { exit 0 }
            $title = if ($q.header) { [string]$q.header } else { 'Claude' }
            $answer = Ask-Notch $title $q.question ($labels + 'Trong app')
            if ($answer -notlike 'choice:*') { exit 0 }
            $index = [int]($answer.Substring(7))
            if ($index -ge $labels.Count) { exit 0 }
            $answers[[string]$q.question] = $labels[$index]
        }
        $updated | Add-Member -NotePropertyName answers -NotePropertyValue $answers -Force
        @{ hookSpecificOutput = @{ hookEventName = 'PermissionRequest'; decision = @{ behavior = 'allow'; updatedInput = $updated } } } | ConvertTo-Json -Depth 10 -Compress
        exit 0
    }

    $detail = ''
    foreach ($key in 'command', 'file_path', 'url', 'pattern', 'description') {
        if ($request.tool_input.$key) { $detail = [string]$request.tool_input.$key; break }
    }
    if (-not $detail) { $detail = ($request.tool_input | ConvertTo-Json -Compress -Depth 3) }
    $answer = Ask-Notch $tool $detail @()
    if ($answer -eq 'allow') {
        '{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}'
    } elseif ($answer -eq 'deny') {
        '{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"Denied on PUW Notch."}}}'
    }
} catch { }
