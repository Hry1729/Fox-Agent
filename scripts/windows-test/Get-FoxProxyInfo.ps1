#Requires -Version 5.1
<#
.SYNOPSIS
    Proxy settings of this machine as Fox sees them (user:password in proxy URLs is redacted), and whether DNS answers with fake-ip
    addresses (B5-2 / B5-3) (Windows PowerShell 5.1 and PowerShell 7).

.DESCRIPTION
    Fox builds proxy variables for commands (auto / full-access only) from, in order:
      1. its own process environment (HTTP_PROXY / HTTPS_PROXY / ALL_PROXY / NO_PROXY, any case);
      2. the user environment in the registry (HKCU\Environment);
      3. the WinINET system proxy (HKCU\...\Internet Settings: ProxyEnable / ProxyServer /
         ProxyOverride). A PAC script (AutoConfigURL) alone is NOT used.
    web_read lets 198.18.0.0/15 through only when one of those sources has a proxy
    ("proxy in use"); that range is what Clash / mihomo / sing-box answer in fake-ip mode.

    This script reports all three sources, resolves -HostName with the system resolver and
    says whether every address is in 198.18.0.0/15 (fakeIp = true). Nothing is changed.

.PARAMETER HostName
    Names to resolve. Default: example.com, www.baidu.com.

.PARAMETER OutPath
    JSON file to write. Without it the result object is returned.

.EXAMPLE
    $R = (Resolve-Path ..\..\docs\windows-test\run-1339).Path
    .\Get-FoxProxyInfo.ps1 -OutPath "$R\fox-evidence\B5-3-proxy.json"
#>
[CmdletBinding()]
param(
    [string[]] $HostName = @('example.com', 'www.baidu.com'),
    [string] $OutPath
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$names = @('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY')

function Read-FoxReg([string] $Key, [string] $Name) {
    try { return (Get-ItemProperty -LiteralPath $Key -Name $Name -ErrorAction Stop).$Name } catch { return $null }
}

# Proxy URLs may carry user:password; only the scheme, host and port are kept.
function Hide-FoxSecret([string] $Value) {
    if (-not $Value) { return $Value }
    return ($Value -replace '(?i)([a-z][a-z0-9+.-]*://)[^/@\s]+@', '$1<redacted>@')
}

$process = [ordered]@{}
foreach ($n in $names) {
    foreach ($v in @([System.Environment]::GetEnvironmentVariables('Process').Keys)) {
        if ([string]$v -ieq $n) { $process[[string]$v] = Hide-FoxSecret ([System.Environment]::GetEnvironmentVariable([string]$v, 'Process')) }
    }
}
$user = [ordered]@{}
foreach ($n in $names) {
    $value = Read-FoxReg 'HKCU:\Environment' $n
    if ($value) { $user[$n] = Hide-FoxSecret $value }
}
$inet = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'
$enable = Read-FoxReg $inet 'ProxyEnable'
$wininet = [ordered]@{
    proxyEnable = $(if ($null -eq $enable) { $null } else { [int]$enable -ne 0 })
    proxyServer = (Hide-FoxSecret (Read-FoxReg $inet 'ProxyServer'))
    proxyOverride = (Read-FoxReg $inet 'ProxyOverride')
    autoConfigUrl = (Read-FoxReg $inet 'AutoConfigURL')
}
$systemProxyUsed = ($wininet.proxyEnable -eq $true -and [string]$wininet.proxyServer -ne '')
$proxyInUse = ($process.Count -gt 0 -or $user.Count -gt 0 -or $systemProxyUsed)
# NO_PROXY alone does not make a proxy.
$onlyNoProxy = $true
foreach ($k in @($process.Keys) + @($user.Keys)) { if ([string]$k -ine 'NO_PROXY') { $onlyNoProxy = $false } }
if (-not $systemProxyUsed -and $onlyNoProxy) { $proxyInUse = $false }

function Test-FoxFakeIp([string] $Address) {
    $ip = $null
    if (-not [System.Net.IPAddress]::TryParse($Address, [ref]$ip)) { return $false }
    if ($ip.IsIPv4MappedToIPv6) { $ip = $ip.MapToIPv4() }
    if ($ip.AddressFamily -ne [System.Net.Sockets.AddressFamily]::InterNetwork) { return $false }
    $b = $ip.GetAddressBytes()
    return ($b[0] -eq 198 -and ($b[1] -eq 18 -or $b[1] -eq 19))
}

$dns = @()
foreach ($h in $HostName) {
    $entry = [ordered]@{ host = $h; addresses = @(); fakeIp = $false; error = $null }
    try {
        $addresses = @([System.Net.Dns]::GetHostAddresses($h) | ForEach-Object { $_.ToString() })
        $entry.addresses = $addresses
        $v4 = @($addresses | Where-Object { $_ -match '^\d+\.\d+\.\d+\.\d+$' })
        $entry.fakeIp = ($v4.Count -gt 0 -and @($v4 | Where-Object { -not (Test-FoxFakeIp $_) }).Count -eq 0)
    } catch { $entry.error = $_.Exception.Message }
    $dns += $entry
}
$anyFake = @($dns | Where-Object { $_.fakeIp }).Count -gt 0

$verdict = if ($anyFake -and $proxyInUse) { 'fake-ip mode with a proxy Fox can see: web_read should be allowed and audited (proxy_fake_ip)' }
    elseif ($anyFake) { 'fake-ip DNS but no proxy Fox can see (no env vars, system proxy off or PAC only): web_read is expected to be blocked with the Chinese network-protection message' }
    elseif ($proxyInUse) { 'proxy in use, DNS returns real addresses (not fake-ip): B5-3 fake-ip step cannot be reproduced; record it as not reached' }
    else { 'no proxy and no fake-ip' }

$result = [ordered]@{
    kind = 'fox-proxy-info'
    takenAt = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss')
    processEnvironment = $process
    userEnvironment = $user
    wininet = $wininet
    systemProxyUsed = $systemProxyUsed
    proxyInUse = $proxyInUse
    dns = $dns
    fakeIp = $anyFake
    verdict = $verdict
}
Write-Host ("proxy in use: {0} (process env {1}, user env {2}, system proxy {3} {4}; PAC {5})" -f $proxyInUse, $process.Count, $user.Count, $wininet.proxyEnable, $wininet.proxyServer, $wininet.autoConfigUrl)
foreach ($d in $dns) { Write-Host ("dns {0} -> {1} fakeIp={2} {3}" -f $d.host, (@($d.addresses) -join ','), $d.fakeIp, $d.error) }
Write-Host ("verdict: {0}" -f $verdict)
if ($OutPath) {
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($OutPath, ($result | ConvertTo-Json -Depth 5), $utf8)
} else { return [pscustomobject]$result }
