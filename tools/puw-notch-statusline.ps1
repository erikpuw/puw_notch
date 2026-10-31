# Status line for Claude Code: keeps its JSON (with the plan limits) where PUW Notch reads it.
$d = Join-Path $env:APPDATA 'PuwNotch'
New-Item -ItemType Directory -Force $d | Out-Null
$text = [Console]::In.ReadToEnd()
$tmp = Join-Path $d 'claude-limits.tmp'
[IO.File]::WriteAllText($tmp, $text)
Move-Item -Force $tmp (Join-Path $d 'claude-limits.json')
