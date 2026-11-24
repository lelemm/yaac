# Run with a staged Windows CEF directory. Only this test's helper receives commands.
param([Parameter(Mandatory=$true)][string]$Stage)
$ErrorActionPreference='Stop'
$env:YAAC_CEF_DIR=$Stage
$env:YAAC_PREVIEW_SESSION = Join-Path ([IO.Path]::GetTempPath()) ('yaac-preview-' + [Guid]::NewGuid().ToString())
New-Item -Path $env:YAAC_PREVIEW_SESSION -ItemType Directory | Out-Null
$start=New-Object System.Diagnostics.ProcessStartInfo
$start.FileName=Join-Path $Stage 'yaac_preview_native.exe'
$start.Arguments='--yaac-browser-host'
$start.UseShellExecute=$false
$start.RedirectStandardInput=$true
$start.RedirectStandardOutput=$true
$proc=[Diagnostics.Process]::Start($start)
try {
 $proc.StandardInput.WriteLine('{"tab":1,"action":{"Open":{"url":"data:text/html,<title>Windows probe</title><body style=background:red>","socks":null}}}')
 $timer=[Diagnostics.Stopwatch]::StartNew()
 $passed=$false
 while ($timer.Elapsed.TotalSeconds -lt 25) {
   $read=$proc.StandardOutput.ReadLineAsync()
   if (!$read.Wait(25000)){throw 'Timed out waiting for browser host'}
   $line=$read.Result
   if ($null -eq $line){throw "Host closed before paint. Exit $($proc.ExitCode)"}
   $notice=$line | ConvertFrom-Json
   if ($notice.event.Paint) {
     $file=Join-Path $env:YAAC_PREVIEW_SESSION '1-0.bgra'
     $stream=[IO.File]::Open($file,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
     try { $bytes=New-Object byte[] 4; [void]$stream.Read($bytes,0,4) } finally { $stream.Dispose() }
     $proc.StandardInput.WriteLine('{"tab":1,"action":{"Consumed":{"popup":false}}}')
     if($bytes[2] -eq 255 -and $bytes[1] -eq 0) {$passed=$true;break}
   }
 }
 if (!$passed){throw 'Missing page color'}
 Write-Output 'Sandbox bootstrap rendered Windows BGRA surface'
 $proc.StandardInput.WriteLine('{"tab":1,"action":{"Inspect":{"title":"YAAC preview test tools","at":null}}}')
 $timer.Restart()
 do {
     Start-Sleep -Milliseconds 100
     $proc.Refresh()
 } while ($proc.MainWindowTitle -ne 'YAAC preview test tools' -and $timer.Elapsed.TotalSeconds -lt 15)
 if ($proc.MainWindowTitle -ne 'YAAC preview test tools') { throw "Missing DevTools window. Title: $($proc.MainWindowTitle)" }
 Write-Output 'DevTools opened with supplied top-level caption'
 $proc.StandardInput.WriteLine('{"tab":0,"action":"Shutdown"}')
 $proc.StandardInput.Close()
 if (!$proc.WaitForExit(10000)){throw 'Host shutdown timed out'}
 if ($proc.ExitCode -ne 0){throw "Host exited $($proc.ExitCode)"}
} finally {
 if (!$proc.HasExited) { $proc.Kill(); $proc.WaitForExit() }
 $proc.Dispose()
 Remove-Item -LiteralPath $env:YAAC_PREVIEW_SESSION -Recurse -Force
}
