param([string]$Exe, [string]$Out)
# Throwaway probe for #116 option A: from an elevated process, start the app with this
# process's own token lowered to Medium integrity, pass the WebView2 variables in an
# explicit environment block, and see whether the debugging port opens.
$ErrorActionPreference = 'Stop'
Start-Transcript -Path $Out -Force | Out-Null
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class Probe {
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool OpenProcessToken(IntPtr p, uint access, out IntPtr tok);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool DuplicateTokenEx(IntPtr t, uint access, IntPtr sa, int imp, int type, out IntPtr nt);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool ConvertStringSidToSid(string s, out IntPtr sid);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool SetTokenInformation(IntPtr t, int cls, ref TOKEN_MANDATORY_LABEL tml, int len);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool GetTokenInformation(IntPtr t, int cls, IntPtr buf, int len, out int ret);
  [DllImport("advapi32.dll")] static extern IntPtr GetSidSubAuthority(IntPtr sid, uint n);
  [DllImport("advapi32.dll")] static extern IntPtr GetSidSubAuthorityCount(IntPtr sid);
  [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
  static extern bool CreateProcessAsUserW(IntPtr t, string app, StringBuilder cmd, IntPtr pa, IntPtr ta, bool inherit, uint flags, IntPtr env, string dir, ref STARTUPINFO si, out PROCESS_INFORMATION pi);
  [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
  [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr OpenProcess(uint a, bool i, int pid);
  [StructLayout(LayoutKind.Sequential)] struct SID_AND_ATTRIBUTES { public IntPtr Sid; public uint Attributes; }
  [StructLayout(LayoutKind.Sequential)] struct TOKEN_MANDATORY_LABEL { public SID_AND_ATTRIBUTES Label; }
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] struct STARTUPINFO {
    public int cb; public string r; public string d; public string t; public int x, y, xs, ys, xc, yc, fa, flags; public short sw, r2; public IntPtr r3, i, o, e; }
  [StructLayout(LayoutKind.Sequential)] public struct PROCESS_INFORMATION { public IntPtr hp, ht; public int pid, tid; }

  public static string Describe(int pid) {
    IntPtr h = pid == 0 ? GetCurrentProcess() : OpenProcess(0x1000, false, pid);
    IntPtr tok; if (!OpenProcessToken(h, 0x8, out tok)) return "open token failed " + Marshal.GetLastWin32Error();
    int len; IntPtr buf = Marshal.AllocHGlobal(256);
    GetTokenInformation(tok, 25, buf, 256, out len); // TokenIntegrityLevel
    IntPtr sid = Marshal.ReadIntPtr(buf);
    int n = Marshal.ReadByte(GetSidSubAuthorityCount(sid));
    int il = Marshal.ReadInt32(GetSidSubAuthority(sid, (uint)(n - 1)));
    GetTokenInformation(tok, 20, buf, 256, out len); // TokenElevation
    int elev = Marshal.ReadInt32(buf);
    GetTokenInformation(tok, 18, buf, 256, out len); // TokenElevationType
    int etype = Marshal.ReadInt32(buf);
    return string.Format("IL 0x{0:X}, TokenElevation {1}, ElevationType {2}", il, elev, etype);
  }

  public static int Start(string exe, string envBlock) {
    IntPtr tok, dup, sid;
    if (!OpenProcessToken(GetCurrentProcess(), 0x1 | 0x2 | 0x8 | 0x80, out tok)) throw new Exception("OpenProcessToken " + Marshal.GetLastWin32Error());
    if (!DuplicateTokenEx(tok, 0x2000000, IntPtr.Zero, 2, 1, out dup)) throw new Exception("DuplicateTokenEx " + Marshal.GetLastWin32Error());
    ConvertStringSidToSid("S-1-16-8192", out sid);
    var tml = new TOKEN_MANDATORY_LABEL(); tml.Label.Sid = sid; tml.Label.Attributes = 0x20; // SE_GROUP_INTEGRITY
    if (!SetTokenInformation(dup, 25, ref tml, Marshal.SizeOf(tml) + 12)) throw new Exception("SetTokenInformation " + Marshal.GetLastWin32Error());
    var si = new STARTUPINFO(); si.cb = Marshal.SizeOf(si);
    PROCESS_INFORMATION pi;
    IntPtr env = Marshal.StringToHGlobalUni(envBlock);
    if (!CreateProcessAsUserW(dup, exe, new StringBuilder("\"" + exe + "\""), IntPtr.Zero, IntPtr.Zero, false, 0x400 /*UNICODE_ENVIRONMENT*/, env, null, ref si, out pi))
      throw new Exception("CreateProcessAsUser " + Marshal.GetLastWin32Error());
    return pi.pid;
  }
}
'@
"pokit side: $([Probe]::Describe(0))"
$tmp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }
$dir = Join-Path $tmp 'probe-medium'
Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
New-Item -ItemType Directory $dir | Out-Null
$vars = [Environment]::GetEnvironmentVariables()
$vars['WEBVIEW2_USER_DATA_FOLDER'] = $dir
$vars['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = '--remote-debugging-port=0'
$block = (($vars.Keys | Sort-Object | ForEach-Object { "$_=$($vars[$_])" }) -join "`0") + "`0`0"
$appPid = [Probe]::Start((Resolve-Path $Exe).Path, $block)
"app side:   $([Probe]::Describe($appPid))"
$f = Join-Path $dir 'EBWebView\DevToolsActivePort'
for ($i = 0; $i -lt 60 -and -not (Test-Path $f); $i++) { Start-Sleep -Milliseconds 500 }
"port file: $(Test-Path $f) $(if (Test-Path $f) { Get-Content $f -TotalCount 1 })"
if (Test-Path $f) {
  $port = Get-Content $f -TotalCount 1
  try { "json/version: $((Invoke-RestMethod "http://127.0.0.1:$port/json/version" -TimeoutSec 5).Browser)" } catch { "json/version failed: $_" }
}
Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object { $_.CommandLine -like '*probe-medium*' -and $_.CommandLine -notlike '*--type=*' } |
  ForEach-Object { "browser: $([Probe]::Describe($_.ProcessId)) :: $($_.CommandLine -replace '.*--disable-features','--disable-features')" }
Stop-Process -Id $appPid -Force -ErrorAction SilentlyContinue
Stop-Transcript | Out-Null
