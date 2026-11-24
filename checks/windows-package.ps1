# Run in an interactive desktop. Pass the compiler's shader file to simulate its absence.
param([Parameter(Mandatory=$true)][string]$Archive, [string]$ShaderSource, [switch]$OldPackage)
$ErrorActionPreference='Stop'
$dir=Join-Path $env:TEMP ("Bastion package check "+[guid]::NewGuid())
Expand-Archive -LiteralPath $Archive -DestinationPath $dir
$exe=Join-Path $dir 'BastionTerm.exe'
$env:BASTION_GPUI_HOME=Join-Path $dir 'isolated-user-data'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class WindowCheck {
 public delegate bool EnumProc(IntPtr h, IntPtr data);
 [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h,EnumProc fn,IntPtr data);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h,System.Text.StringBuilder text,int count);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageTimeout(IntPtr h,uint msg,IntPtr count,System.Text.StringBuilder text,uint flags,uint timeout,out IntPtr result);
 public static string DialogText(IntPtr h){
  var result=new System.Text.StringBuilder();
  EnumChildWindows(h, delegate(IntPtr child,IntPtr unused){
   var b=new System.Text.StringBuilder(2048);
   IntPtr len;
   SendMessageTimeout(child,13,(IntPtr)b.Capacity,b,2,300,out len);
   result.AppendLine(b.ToString());
   return true;
  }, IntPtr.Zero);
  return result.ToString();
 }
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern IntPtr GetMenu(IntPtr hwnd);
 [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
 [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr h, int how);
}
'@
try {
    # FileShare.None prevents either test process from reading the builder's shader source.
    if($ShaderSource){$denyRead=[IO.File]::Open($ShaderSource,'Open','Read','None')}
    if (!$OldPackage) {
        # Exercise a cold MCP client with all streams redirected, as a real agent does.
        $start=New-Object System.Diagnostics.ProcessStartInfo
        $start.FileName=$exe
        $start.Arguments='--mcp'
        $start.UseShellExecute=$false
        $start.CreateNoWindow=$true
        $start.RedirectStandardInput=$true
        $start.RedirectStandardOutput=$true
        $start.RedirectStandardError=$true
        $client=[Diagnostics.Process]::Start($start)
        try {
            $out=$client.StandardOutput.ReadToEndAsync()
            $err=$client.StandardError.ReadToEndAsync()
            $client.StandardInput.WriteLine('{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ssh_start_web","arguments":{}}}')
            $client.StandardInput.Close()
            if (!$client.WaitForExit(20000) -or $client.ExitCode -ne 0) {throw 'Cold MCP client failed to exit'}
            if (!$out.Wait(4000) -or !$err.Wait(4000)) {throw 'Service inherited a client pipe'}
            $reply=$out.Result | ConvertFrom-Json
            if (!$reply.result.content -or $reply.result.isError) {throw 'Cold MCP call failed'}
            if (!(Test-Path (Join-Path $env:BASTION_GPUI_HOME 'service.json'))) {throw 'Service exited with client'}
        } finally { $client.Dispose() }
    }
    $process=Start-Process -FilePath $exe -WorkingDirectory $dir -PassThru
    $until=(Get-Date).AddSeconds(25)
    do {
        Start-Sleep -Milliseconds 200
        $process.Refresh()
        if($process.HasExited){throw "Exited before opening terminal: $($process.ExitCode)"}
        $title=$process.MainWindowTitle
    } until ($title -like '*BastionTerm*' -or (Get-Date) -gt $until)
    if($OldPackage){
        if($title -like '*BastionTerm*'){throw 'Old binary unexpectedly opened terminal'}
        $detail=[WindowCheck]::DialogText($process.MainWindowHandle)
        if($detail -notmatch 'DirectWriteTextSystem'){throw "Unexpected old error: $title $detail"}
        Write-Output "Control reproduced DirectWriteTextSystem: $detail"
        return
    }
    if($title -ne ([char]0x5792+[string][char]0x6865+[char]0x7ec8+[char]0x7aef+' '+[char]0xb7+' BastionTerm')){throw "Expected terminal window; got '$title'"}
    $hwnd=$process.MainWindowHandle
    if ([WindowCheck]::GetMenu($hwnd) -ne [IntPtr]::Zero) {throw 'The main window reserves a system menu row at launch'}
    $shell=New-Object -ComObject WScript.Shell
    $null=$shell.AppActivate($process.Id)
    for($i=0;$i -lt 20 -and [WindowCheck]::GetForegroundWindow() -ne $hwnd;$i++){Start-Sleep -Milliseconds 100}
    if([WindowCheck]::GetForegroundWindow() -ne $hwnd){throw 'Cannot focus the isolated terminal for shortcut testing'}
    $shell.SendKeys('^+m')
    Start-Sleep -Milliseconds 400
    if([WindowCheck]::GetMenu($hwnd) -ne [IntPtr]::Zero){throw 'Ctrl+Shift+M opened an application menu'}
    Write-Output 'PASS: no application menu at startup or after Ctrl+Shift+M'
    if (!$process.CloseMainWindow()) {throw 'Could not close the native window'}
    Start-Sleep -Milliseconds 400
    if(![WindowCheck]::IsIconic($hwnd)){throw 'Could not minimize test window'}
    $second=Start-Process -FilePath $exe -PassThru
    if(!$second.WaitForExit(10000) -or $second.ExitCode -ne 0){throw 'Second launch failed'}
    for($i=0;$i -lt 30 -and [WindowCheck]::IsIconic($hwnd);$i++){Start-Sleep -Milliseconds 150}
    if([WindowCheck]::IsIconic($hwnd)){throw 'Second launch left terminal minimized'}
    $q='{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ssh_list_sessions","arguments":{}}}'
    $r=$q | & $exe --mcp | Out-String | ConvertFrom-Json
    if($r.result.content[0].text -ne '{"sessions":[]}'){throw 'Unexpected MCP response'}
    Write-Output "PASS: closing the window minimized it; a second launch restored the same terminal with MCP active"
} finally {
    if($denyRead){$denyRead.Dispose()}
    if($process -and !$process.HasExited){Stop-Process -Id $process.Id}
    if (!$OldPackage) { & $exe --stop | Out-Null }
}
