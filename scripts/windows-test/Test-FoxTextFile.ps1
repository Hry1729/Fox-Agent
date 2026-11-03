#Requires -Version 5.1
<#
.SYNOPSIS
    Encoding and line endings of a text file Fox wrote (B5-1: .bat / .cmd files)
    (Windows PowerShell 5.1 and PowerShell 7).

.DESCRIPTION
    Reads the raw bytes and reports:
        bytes, bom (none / utf8 / utf16le / utf16be), crlf, loneLf, loneCr,
        validUtf8, hasNonAscii, gbkRoundTrip (bytes decode as code page 936 and encode
        back to the same bytes), ansiCodePage (GetACP of this machine), firstLine,
        startsWithChcp65001, guess (ascii / gbk / utf8 / utf8-bom / utf16 / unknown).
    "batchOk" is true when the file has no lone LF and is either GBK (936, no BOM) or
    UTF-8 whose first line is "chcp 65001 >nul". Nothing is changed.

.PARAMETER Path
    One or more files.

.PARAMETER OutPath
    JSON file to write (array). Without it the result objects are returned.

.EXAMPLE
    .\Test-FoxTextFile.ps1 -Path 'D:\fox-test\<project>\<file>.bat' -OutPath .\fox-evidence\B5-1-file.json
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string[]] $Path,
    [string] $OutPath
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

if (-not ('FoxForensics.Acp' -as [type])) {
    Add-Type -TypeDefinition @'
using System.Runtime.InteropServices;
namespace FoxForensics { public static class Acp { [DllImport("kernel32.dll")] public static extern uint GetACP(); } }
'@
}

function Test-FoxSameBytes([byte[]] $A, [byte[]] $B) {
    if ($A.Length -ne $B.Length) { return $false }
    for ($i = 0; $i -lt $A.Length; $i++) { if ($A[$i] -ne $B[$i]) { return $false } }
    return $true
}

$acp = -1
try { $acp = [int][FoxForensics.Acp]::GetACP() } catch { $acp = -1 }

$results = @()
foreach ($item in $Path) {
    $full = (Resolve-Path -LiteralPath $item).ProviderPath
    $bytes = [System.IO.File]::ReadAllBytes($full)
    $bom = 'none'; $offset = 0
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) { $bom = 'utf8'; $offset = 3 }
    elseif ($bytes.Length -ge 2 -and $bytes[0] -eq 0xFF -and $bytes[1] -eq 0xFE) { $bom = 'utf16le'; $offset = 2 }
    elseif ($bytes.Length -ge 2 -and $bytes[0] -eq 0xFE -and $bytes[1] -eq 0xFF) { $bom = 'utf16be'; $offset = 2 }

    $crlf = 0; $loneLf = 0; $loneCr = 0; $nonAscii = $false
    for ($i = 0; $i -lt $bytes.Length; $i++) {
        $b = $bytes[$i]
        if ($b -ge 0x80) { $nonAscii = $true }
        if ($b -eq 0x0D) {
            if ($i + 1 -lt $bytes.Length -and $bytes[$i + 1] -eq 0x0A) { $crlf++; $i++ } else { $loneCr++ }
        } elseif ($b -eq 0x0A) { $loneLf++ }
    }

    $body = New-Object byte[] ($bytes.Length - $offset)
    [System.Array]::Copy($bytes, $offset, $body, 0, $body.Length)
    $validUtf8 = $true
    try { [void](New-Object System.Text.UTF8Encoding($false, $true)).GetString($body) } catch { $validUtf8 = $false }
    $gbkRoundTrip = $false
    try {
        $gbk = [System.Text.Encoding]::GetEncoding(936)
        $gbkRoundTrip = Test-FoxSameBytes $body ($gbk.GetBytes($gbk.GetString($body)))
    } catch { $gbkRoundTrip = $false }

    $guess = 'unknown'
    if ($bom -like 'utf16*') { $guess = 'utf16' }
    elseif (-not $nonAscii) { $guess = 'ascii' }
    elseif ($bom -eq 'utf8') { $guess = 'utf8-bom' }
    elseif ($validUtf8) { $guess = 'utf8' }
    elseif ($gbkRoundTrip) { $guess = 'gbk' }

    $text = ''
    if ($guess -eq 'gbk') { $text = [System.Text.Encoding]::GetEncoding(936).GetString($body) }
    else { $text = (New-Object System.Text.UTF8Encoding($false)).GetString($body) }
    $firstLine = ($text -split "`r?`n")[0]
    $chcp = ($firstLine -match '^\s*chcp\s+65001')

    $batchOk = ($loneLf -eq 0 -and $loneCr -eq 0 -and (($guess -eq 'gbk') -or ($guess -eq 'ascii') -or (($guess -eq 'utf8') -and $chcp)))
    $results += [ordered]@{
        path = $full; bytes = $bytes.Length; bom = $bom
        crlf = $crlf; loneLf = $loneLf; loneCr = $loneCr
        hasNonAscii = $nonAscii; validUtf8 = $validUtf8; gbkRoundTrip = $gbkRoundTrip
        ansiCodePage = $acp
        guess = $guess; firstLine = $firstLine; startsWithChcp65001 = [bool]$chcp
        batchOk = [bool]$batchOk
        sha256 = (Get-FileHash -LiteralPath $full -Algorithm SHA256).Hash
    }
}

foreach ($r in $results) {
    Write-Host ("{0}: {1} bytes, guess={2}, bom={3}, CRLF={4}, loneLF={5}, loneCR={6}, ACP={7}, chcp65001={8}, batchOk={9}" -f $r.path, $r.bytes, $r.guess, $r.bom, $r.crlf, $r.loneLf, $r.loneCr, $r.ansiCodePage, $r.startsWithChcp65001, $r.batchOk)
}
if ($OutPath) {
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($OutPath, (ConvertTo-Json -InputObject @($results) -Depth 4), $utf8)
} else { $results | ForEach-Object { [pscustomobject]$_ } }
