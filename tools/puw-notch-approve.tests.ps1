param([string]$HookPath = (Join-Path $PSScriptRoot 'puw-notch-approve.ps1'))
$ErrorActionPreference = 'Stop'
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($HookPath, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
$formatter = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Format-ToolDetail' }, $true)
if (-not $formatter) { throw 'Format-ToolDetail is missing: fallback still prints escaped JSON.' }
. ([scriptblock]::Create($formatter.Extent.Text))
function Assert-Equal($actual, $expected, $label) {
    if ($actual -cne $expected) { throw "$label expected [$expected], got [$actual]" }
}
$sample = '{"language":"javascript","cwd":"E:\\workspace\\jarvis","code":"\nconst fs=require(\u0027fs\u0027);\nconst files=[\u0027engine/tools/ziweil_engine.py\u0027];"}' | ConvertFrom-Json
$detail = ((Format-ToolDetail $sample) -replace '\s+', ' ').Trim()
Assert-Equal $detail "language: javascript; cwd: E:\workspace\jarvis; code: const fs=require('fs'); const files=['engine/tools/ziweil_engine.py'];" 'MCP code and cwd'
# Literal backslashes in an already decoded command must remain literal, especially Windows paths.
$literal = '{"path":"C:\\new\\tools\\file.txt","code":"String.raw`\\n\\u0027`"}' | ConvertFrom-Json
Assert-Equal (Format-ToolDetail $literal) 'path: C:\new\tools\file.txt; code: String.raw`\n\u0027`' 'literal escapes'
$nested = '{"items":["one","two"],"flags":{"enabled":true,"count":0}}' | ConvertFrom-Json
Assert-Equal (Format-ToolDetail $nested) 'items: [one, two]; flags: enabled: True; count: 0' 'nested values'
Assert-Equal (Format-ToolDetail $null) '' 'null input'
$emoji = '{"message":"\ud83d\ude00"}' | ConvertFrom-Json
Assert-Equal (Format-ToolDetail $emoji) ('message: ' + [char]::ConvertFromUtf32(0x1f600)) 'Unicode surrogate pair'
$wire = @{ app = 'Codex'; title = 'mcp'; detail = $detail } | ConvertTo-Json -Compress
Assert-Equal (($wire | ConvertFrom-Json).detail) $detail 'pipe JSON round trip'
Write-Output 'PASS: 6 approval formatting checks; hook syntax valid.'
