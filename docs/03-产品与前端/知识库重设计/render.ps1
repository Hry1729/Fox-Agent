# Renders every mockup frame for each theme and mode into images/<theme>-<mode>/.
param(
  [string[]]$Themes = @('graphite', 'paper', 'mist'),
  [string[]]$Modes = @('light', 'dark'),
  [string[]]$Frames = @('home', 'files', 'kb-list', 'kb-home', 'kb-search', 'reader', 'kb-settings', 'jobs', 'models', 'kb-create')
)
$browser = @(
  'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe',
  'C:\Program Files\Microsoft\Edge\Application\msedge.exe',
  'C:\Program Files\Google\Chrome\Application\chrome.exe'
) | Where-Object { Test-Path $_ } | Select-Object -First 1
$page = ([System.Uri](Join-Path $PSScriptRoot 'index.html')).AbsoluteUri
$allFrames = @('home', 'files', 'kb-list', 'kb-home', 'kb-search', 'reader', 'kb-settings', 'jobs', 'models', 'kb-create')
foreach ($theme in $Themes) {
  foreach ($mode in $Modes) {
    $dir = Join-Path $PSScriptRoot "images\$theme-$mode"
    New-Item -ItemType Directory -Force $dir | Out-Null
    foreach ($frame in $Frames) {
      $index = [array]::IndexOf($allFrames, $frame) + 1
      $out = Join-Path $dir ('{0:D2}-{1}.png' -f $index, $frame)
      & $browser --headless=new --disable-gpu --hide-scrollbars --allow-file-access-from-files --force-device-scale-factor=2 --window-size=1280,800 --virtual-time-budget=3000 "--screenshot=$out" "$page`?frame=$frame&theme=$theme&mode=$mode" 2>$null | Out-Null
    }
  }
}
