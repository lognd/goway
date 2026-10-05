# goway remote side for Windows hosts. The counterpart of remote.sh: the
# same verbs, the same labelled directories, the same protocol and output
# formats, in PowerShell (Windows PowerShell 5.1 and PowerShell 7, so it
# also runs under pwsh on Linux for the contract tests).
#
#   powershell -NoProfile -File remote.ps1 VERB ARGS...
#
# Every directory goway owns carries a meta.json label and a lock file that
# is held (an open handle, FileShare.None) while in use; gc takes the same
# locks. All output is raw bytes on the standard streams, UTF-8, "\n" line
# ends, NUL separators where remote.sh uses them. Failures of the script
# itself exit 125, never a command-like code.
#
# Differences from remote.sh, all forced by Windows: symlinks in a tree are
# not created (reported on stderr, skipped); the executable bit of a file
# lives in a sidecar list of the seed; "load" is the CPU load scaled to the
# core count; a job's process tree is stopped with taskkill.
Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$script:IsWin = [Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT
$script:Utf8 = New-Object System.Text.UTF8Encoding($false)
$script:Stdout = [Console]::OpenStandardOutput()
$script:Stderr = [Console]::OpenStandardError()
$script:SelfPath = $MyInvocation.MyCommand.Path
$script:Held = @{}
$script:Ver = $PSVersionTable.PSVersion.Major

function Write-Out([string]$Text) {
  $b = $script:Utf8.GetBytes($Text)
  $script:Stdout.Write($b, 0, $b.Length)
  $script:Stdout.Flush()
}
function Write-OutBytes([byte[]]$Bytes) {
  $script:Stdout.Write($Bytes, 0, $Bytes.Length)
  $script:Stdout.Flush()
}
function Write-Err([string]$Text) {
  $b = $script:Utf8.GetBytes($Text)
  $script:Stderr.Write($b, 0, $b.Length)
  $script:Stderr.Flush()
}
# In a session (one process serving many verbs) a verb must not end the
# process: Exit-Verb throws a marker the session loop turns into the verb's
# exit code; as a one-shot it is a plain exit.
$script:InSession = $false
function Exit-Verb([int]$Code) {
  if ($script:InSession) { throw "goway-exit:$Code" }
  exit $Code
}
function Die([string]$Message) {
  Write-Err "goway-remote: $Message`n"
  Exit-Verb 125
}
trap {
  Write-Err ("goway-remote: failed at line {0}: {1}`n" -f $_.InvocationInfo.ScriptLineNumber, $_.Exception.Message)
  Exit-Verb 125
}

# ---- stdin ------------------------------------------------------------

# The verb's standard input: the request's input in a session.
$script:SessionIn = $null
function Get-Stdin {
  if ($script:InSession) { return $script:SessionIn }
  return [Console]::OpenStandardInput()
}

function Read-StdinBytes {
  $in = Get-Stdin
  $ms = New-Object System.IO.MemoryStream
  $in.CopyTo($ms)
  return ,$ms.ToArray()
}

# Split a NUL-terminated byte list into strings (empty items dropped).
function Split-Nul([byte[]]$Bytes) {
  $out = New-Object System.Collections.Generic.List[string]
  $start = 0
  for ($i = 0; $i -lt $Bytes.Length; $i++) {
    if ($Bytes[$i] -eq 0) {
      if ($i -gt $start) { $out.Add($script:Utf8.GetString($Bytes, $start, $i - $start)) }
      $start = $i + 1
    }
  }
  if ($start -lt $Bytes.Length) { $out.Add($script:Utf8.GetString($Bytes, $start, $Bytes.Length - $start)) }
  return ,$out
}

# Split NUL-terminated words, keeping empty ones (an empty argument is an argument).
function Split-Words([byte[]]$Bytes) {
  $out = New-Object System.Collections.Generic.List[string]
  $start = 0
  for ($i = 0; $i -lt $Bytes.Length; $i++) {
    if ($Bytes[$i] -eq 0) {
      $out.Add($script:Utf8.GetString($Bytes, $start, $i - $start))
      $start = $i + 1
    }
  }
  return ,$out
}

# ---- paths ------------------------------------------------------------

function Get-Home {
  $h = [Environment]::GetFolderPath('UserProfile')
  if (-not $h) { $h = $env:HOME }
  return $h
}

function Get-Root([string]$Arg) {
  if ([IO.Path]::IsPathRooted($Arg)) { return [IO.Path]::GetFullPath($Arg) }
  return [IO.Path]::GetFullPath([IO.Path]::Combine((Get-Home), $Arg))
}

function P([string]$Base, [string[]]$Parts) {
  $p = $Base
  foreach ($x in $Parts) { $p = [IO.Path]::Combine($p, $x) }
  return $p
}

function New-Dir([string]$Path) { [void][IO.Directory]::CreateDirectory($Path) }

function Read-TextOrEmpty([string]$Path) {
  try { return [IO.File]::ReadAllText($Path, $script:Utf8) } catch { return '' }
}
function Write-Text([string]$Path, [string]$Text) {
  [IO.File]::WriteAllText($Path, $Text, $script:Utf8)
}

function Unix-Secs { return [DateTimeOffset]::UtcNow.ToUnixTimeSeconds() }

# ---- locks ------------------------------------------------------------
# A lock is an open FileStream on a lock file: FileShare.None for exclusive,
# read access with FileShare.Read for shared (any number of holders). Held
# until Unlock or the end of the process; a run's locks end with it.

function Try-Lock([string]$Path, [bool]$Exclusive) {
  if (-not [IO.File]::Exists($Path)) {
    try { $c = [IO.File]::Open($Path, 'OpenOrCreate', 'ReadWrite', 'ReadWrite'); $c.Dispose() } catch [IO.IOException] { }
  }
  try {
    if ($Exclusive) {
      return [IO.File]::Open($Path, 'Open', 'ReadWrite', 'None')
    }
    return [IO.File]::Open($Path, 'Open', 'Read', 'Read')
  } catch [IO.IOException] {
    return $null
  } catch [UnauthorizedAccessException] {
    return $null
  }
}

# Take the lock at $Path (blocking up to $TimeoutMs, 0 = try once, -1 = forever).
function Get-Lock([string]$Path, [bool]$Exclusive, [int]$TimeoutMs) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($true) {
    $fs = Try-Lock $Path $Exclusive
    if ($fs) { return $fs }
    if ($TimeoutMs -eq 0) { return $null }
    if ($TimeoutMs -gt 0 -and $sw.ElapsedMilliseconds -ge $TimeoutMs) { return $null }
    Start-Sleep -Milliseconds 50
  }
}

# Hold DIR\lock (creating DIR if needed) under $Key until Unlock-Key. gc may
# remove DIR at any moment, so a vanished directory or lock file retries.
function Lock-Dir([string]$Key, [string]$Dir, [bool]$Exclusive) {
  Unlock-Key $Key
  $path = [IO.Path]::Combine($Dir, 'lock')
  for ($i = 0; $i -lt 200; $i++) {
    New-Dir $Dir
    try {
      $fs = Get-Lock $path $Exclusive 5000
    } catch [IO.DirectoryNotFoundException] { $fs = $null }
    if ($fs) {
      if ($env:GOWAY_TEST_HOOK -and $i -eq 0) {
        # Tests only: run once between the open and the check (see remote.sh).
        $hook = $env:GOWAY_TEST_HOOK; $env:GOWAY_TEST_HOOK = $null
        try { Invoke-Expression $hook } catch { }
      }
      if ([IO.File]::Exists($path)) {
        $script:Held[$Key] = $fs
        return
      }
      $fs.Dispose()
    }
    Start-Sleep -Milliseconds 50
  }
  Die "cannot lock $Dir (kept vanishing)"
}

function Unlock-Key([string]$Key) {
  if ($script:Held.ContainsKey($Key)) {
    $script:Held[$Key].Dispose()
    $script:Held.Remove($Key)
  }
}

# ---- state root -------------------------------------------------------

function Mark-Root([string]$Root) {
  $marker = P $Root @('.goway-root')
  if ([IO.File]::Exists($marker)) { return }
  New-Dir $Root
  foreach ($e in [IO.Directory]::EnumerateFileSystemEntries($Root)) {
    $n = [IO.Path]::GetFileName($e)
    if (@('work', 'seed', 'cache', 'gpu', 'gc.lock', 'evicted.log', '.goway-root') -notcontains $n) {
      Die "$Root exists, is not empty and is not goway state; pick a dedicated remote_root"
    }
  }
  Write-Text $marker "goway state; safe to delete with goway gc --all`n"
}

function New-Generation {
  $ns = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() * 1000000 + (Get-Random -Maximum 999999)
  return ('{0}-{1}-{2}' -f $ns, $PID, (Get-Random -Maximum 32768))
}

function Test-Id([string]$Id, [string]$What) {
  if (-not $Id -or $Id -notmatch '^[A-Za-z0-9-]+$') { Die "${What}: bad id" }
}

# ---- native helper (compiled on first use) ----------------------------

$script:NativeLoaded = $false
$script:NativeSource = @'
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class GowayNative {
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "CreateHardLinkW")]
  static extern bool CreateHardLinkW(string link, string existing, IntPtr security);
  [DllImport("kernel32.dll")]
  static extern bool IsProcessorFeaturePresent(uint feature);

  // Keeps the machine from sleeping on idle while a job runs: the request is
  // per thread and ends with ES_CONTINUOUS cleared or when the thread ends.
  [DllImport("kernel32.dll")]
  static extern uint SetThreadExecutionState(uint flags);
  public static void KeepAwake(bool on) {
    SetThreadExecutionState(on ? 0x80000001u : 0x80000000u); // ES_CONTINUOUS | ES_SYSTEM_REQUIRED
  }

  public static bool IsWindows { get { return Environment.OSVersion.Platform == PlatformID.Win32NT; } }

  // Memory, CPU load and the parent process without WMI (Get-CimInstance
  // costs a second or so per query under Windows PowerShell).
  [StructLayout(LayoutKind.Sequential)]
  struct MemStatus {
    public uint Length; public uint Load; public ulong Total; public ulong Avail;
    public ulong PageTotal; public ulong PageAvail; public ulong VirtTotal; public ulong VirtAvail; public ulong Ext;
  }
  [DllImport("kernel32.dll")] static extern bool GlobalMemoryStatusEx(ref MemStatus m);
  [DllImport("kernel32.dll")] static extern bool GetSystemTimes(out long idle, out long kernel, out long user);

  [StructLayout(LayoutKind.Sequential)]
  struct ProcBasic {
    public IntPtr Exit; public IntPtr PebBase; public IntPtr Affinity; public IntPtr Priority;
    public IntPtr UniquePid; public IntPtr ParentPid;
  }
  [DllImport("ntdll.dll")] static extern int NtQueryInformationProcess(IntPtr h, int cls, ref ProcBasic info, int len, out int ret);

  public static long[] Mem() {
    MemStatus m = new MemStatus();
    m.Length = (uint)Marshal.SizeOf(typeof(MemStatus));
    if (!GlobalMemoryStatusEx(ref m)) return null;
    return new long[] { (long)m.Total, (long)m.Avail };
  }

  // Percent of CPU time busy over a short sample, or -1.
  public static double Load(int ms) {
    long i1, k1, u1, i2, k2, u2;
    if (!GetSystemTimes(out i1, out k1, out u1)) return -1;
    Thread.Sleep(ms);
    if (!GetSystemTimes(out i2, out k2, out u2)) return -1;
    double total = (k2 - k1) + (u2 - u1);
    if (total <= 0) return -1;
    return 100.0 * (total - (i2 - i1)) / total;
  }

  // The parent process id of this process, or 0.
  public static int ParentPid() {
    ProcBasic pbi = new ProcBasic();
    int ret;
    if (NtQueryInformationProcess(Process.GetCurrentProcess().Handle, 0, ref pbi, Marshal.SizeOf(typeof(ProcBasic)), out ret) != 0) return 0;
    return (int)pbi.ParentPid.ToInt64();
  }

  // Hard-link every file of src into dst (directories created as needed).
  public static int LinkTree(string src, string dst) {
    int n = 0;
    Directory.CreateDirectory(dst);
    foreach (string d in Directory.EnumerateDirectories(src, "*", SearchOption.AllDirectories)) {
      Directory.CreateDirectory(Path.Combine(dst, d.Substring(src.Length).TrimStart(Path.DirectorySeparatorChar, '/')));
    }
    foreach (string f in Directory.EnumerateFiles(src, "*", SearchOption.AllDirectories)) {
      string rel = f.Substring(src.Length).TrimStart(Path.DirectorySeparatorChar, '/');
      string to = Path.Combine(dst, rel);
      if (!CreateHardLinkW(to, f, IntPtr.Zero)) {
        throw new IOException("cannot link " + f + " (error " + Marshal.GetLastWin32Error() + ")");
      }
      n++;
    }
    return n;
  }

  public static bool Feature(uint id) {
    try { return IsProcessorFeaturePresent(id); } catch { return false; }
  }

  // Which of the markers occur in the first cap bytes of the file.
  public static string[] FindMarkers(string path, string[] markers, long cap) {
    var found = new HashSet<string>();
    var pats = new List<byte[]>();
    foreach (string m in markers) pats.Add(Encoding.ASCII.GetBytes(m));
    int maxLen = 0;
    foreach (byte[] p in pats) if (p.Length > maxLen) maxLen = p.Length;
    byte[] buf = new byte[1 << 20];
    byte[] carry = new byte[0];
    long total = 0;
    using (var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete)) {
      int n;
      while (total < cap && (n = fs.Read(buf, 0, buf.Length)) > 0) {
        total += n;
        byte[] win = new byte[carry.Length + n];
        Buffer.BlockCopy(carry, 0, win, 0, carry.Length);
        Buffer.BlockCopy(buf, 0, win, carry.Length, n);
        for (int i = 0; i < pats.Count; i++) {
          if (!found.Contains(markers[i]) && IndexOf(win, pats[i]) >= 0) found.Add(markers[i]);
        }
        int keep = Math.Min(maxLen - 1, win.Length);
        carry = new byte[keep];
        Buffer.BlockCopy(win, win.Length - keep, carry, 0, keep);
      }
    }
    var res = new List<string>(found);
    return res.ToArray();
  }

  static int IndexOf(byte[] hay, byte[] needle) {
    if (needle.Length == 0) return 0;
    int last = hay.Length - needle.Length;
    for (int i = 0; i <= last; i++) {
      if (hay[i] != needle[0]) continue;
      int j = 1;
      while (j < needle.Length && hay[i + j] == needle[j]) j++;
      if (j == needle.Length) return i;
    }
    return -1;
  }

  // A Windows job object that kills everything in it when this process
  // ends, however it ends: the job's process tree never outlives the run.
  public static class JobTree {
    [StructLayout(LayoutKind.Sequential)]
    struct IoCounters { public ulong a, b, c, d, e, f; }
    [StructLayout(LayoutKind.Sequential)]
    struct Basic {
      public long PerProcessUserTimeLimit; public long PerJobUserTimeLimit; public uint LimitFlags;
      public UIntPtr MinimumWorkingSetSize; public UIntPtr MaximumWorkingSetSize; public uint ActiveProcessLimit;
      public UIntPtr Affinity; public uint PriorityClass; public uint SchedulingClass;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct Extended {
      public Basic BasicLimit; public IoCounters Io; public UIntPtr ProcessMemoryLimit;
      public UIntPtr JobMemoryLimit; public UIntPtr PeakProcessMemoryUsed; public UIntPtr PeakJobMemoryUsed;
    }
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    static extern IntPtr CreateJobObjectW(IntPtr attrs, string name);
    [DllImport("kernel32.dll")]
    static extern bool SetInformationJobObject(IntPtr job, int cls, IntPtr info, uint len);
    [DllImport("kernel32.dll")]
    static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    static IntPtr job = IntPtr.Zero;
    public static bool Attach(Process p) {
      if (Environment.OSVersion.Platform != PlatformID.Win32NT) return false;
      if (job == IntPtr.Zero) {
        job = CreateJobObjectW(IntPtr.Zero, null);
        if (job == IntPtr.Zero) return false;
        var info = new Extended();
        info.BasicLimit.LimitFlags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        int len = Marshal.SizeOf(typeof(Extended));
        IntPtr mem = Marshal.AllocHGlobal(len);
        try {
          Marshal.StructureToPtr(info, mem, false);
          if (!SetInformationJobObject(job, 9, mem, (uint)len)) return false;
        } finally { Marshal.FreeHGlobal(mem); }
      }
      return AssignProcessToJobObject(job, p.Handle);
    }
  }

  // Copy a stream to another until the end, keeping the first cap bytes.
  public sealed class Tee {
    public readonly MemoryStream Head = new MemoryStream();
    readonly Stream src, dst; readonly int cap; Thread t;
    public Tee(Stream src, Stream dst, int cap) { this.src = src; this.dst = dst; this.cap = cap; }
    public void Start() { t = new Thread(Run); t.IsBackground = true; t.Start(); }
    void Run() {
      byte[] b = new byte[16384]; int n;
      try {
        while ((n = src.Read(b, 0, b.Length)) > 0) {
          lock (Head) { if (Head.Length < cap) Head.Write(b, 0, (int)Math.Min(n, cap - Head.Length)); }
          dst.Write(b, 0, n); dst.Flush();
        }
      } catch (IOException) { }
    }
    public void Join() { if (t != null) t.Join(); }
    public byte[] Snapshot() { lock (Head) { return Head.ToArray(); } }
  }
}
'@

# Compile the helper once per script version into a DLL next to the script
# and load that afterwards (Windows PowerShell only; about a tenth of a
# second against a quarter for compiling every time). Any trouble falls back
# to compiling in memory.
function Load-Native {
  if ($script:NativeLoaded) { return }
  if ($script:Ver -lt 6 -and $script:SelfPath) {
    # Named by a hash of its source, so a changed script never loads an older build.
    $sha = [Security.Cryptography.SHA256]::Create()
    $tag = ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($script:NativeSource)), 0, 4)).Replace('-', '').ToLowerInvariant()
    $sha.Dispose()
    $dll = [IO.Path]::Combine([IO.Path]::GetDirectoryName($script:SelfPath), [IO.Path]::GetFileNameWithoutExtension($script:SelfPath) + ".native.$tag.dll")
    if (-not [IO.File]::Exists($dll)) {
      $tmp = "$dll.$PID.tmp"
      try {
        Add-Type -TypeDefinition $script:NativeSource -OutputAssembly $tmp
        if ([IO.File]::Exists($dll)) { [IO.File]::Delete($tmp) } else { [IO.File]::Move($tmp, $dll) }
      } catch { try { [IO.File]::Delete($tmp) } catch { } }
    }
    if ([IO.File]::Exists($dll)) {
      try { Add-Type -Path $dll; $script:NativeLoaded = $true; return } catch { try { [IO.File]::Delete($dll) } catch { } }
    }
  }
  Add-Type -TypeDefinition $script:NativeSource
  $script:NativeLoaded = $true
}

# ---- trees ------------------------------------------------------------

$script:EpochTicks = (New-Object DateTime 1970, 1, 1, 0, 0, 0, ([DateTimeKind]::Utc)).Ticks

function To-Rel([string]$Full, [int]$BaseLen) {
  $r = $Full.Substring($BaseLen)
  if ($script:IsWin) { $r = $r.Replace('\', '/') }
  return $r
}

# The regular files under $Dir: a list of @(relative path, size, mtime ticks).
function Get-Files([string]$Dir) {
  $out = New-Object System.Collections.Generic.List[object[]]
  if (-not [IO.Directory]::Exists($Dir)) { return ,$out }
  $base = $Dir.TrimEnd([IO.Path]::DirectorySeparatorChar, '/') + [IO.Path]::DirectorySeparatorChar
  $di = New-Object IO.DirectoryInfo $Dir
  foreach ($f in $di.EnumerateFiles('*', [IO.SearchOption]::AllDirectories)) {
    $out.Add(@((To-Rel $f.FullName $base.Length), $f.Length, $f.LastWriteTimeUtc.Ticks))
  }
  return ,$out
}

function Native-Path([string]$Rel) {
  if ($script:IsWin) { return $Rel.Replace('/', '\') }
  return $Rel
}

function Remove-EmptyDirs([string]$Dir, [bool]$Self) {
  foreach ($d in [IO.Directory]::EnumerateDirectories($Dir)) { Remove-EmptyDirs $d $true }
  if ($Self -and -not (@([IO.Directory]::EnumerateFileSystemEntries($Dir)).Count)) {
    try { [IO.Directory]::Delete($Dir) } catch { }
  }
}

# Best-effort removal of a finished run's work dir (as remote.sh remove_work): it
# never throws and never changes the command's exit code. A job's detached child, a
# virus scanner or a concurrent gc may still hold something there, so it retries a
# few times and otherwise leaves the directory for gc (any unlocked work dir past its
# orphan age).
function Remove-Work([string]$Dir) {
  for ($n = 1; $n -le 5; $n++) {
    try { Remove-Tree $Dir; return } catch { Start-Sleep -Milliseconds 200 }
  }
  Write-Err "goway: note: could not remove the work dir of this run; gc will collect it`n"
}

function Remove-Tree([string]$Path) {
  if ([IO.Directory]::Exists($Path)) {
    try { [IO.Directory]::Delete($Path, $true) } catch {
      # A read-only file blocks the delete on Windows: clear and retry.
      foreach ($f in [IO.Directory]::EnumerateFiles($Path, '*', [IO.SearchOption]::AllDirectories)) {
        try { [IO.File]::SetAttributes($f, [IO.FileAttributes]::Normal) } catch { }
      }
      [IO.Directory]::Delete($Path, $true)
    }
  } elseif ([IO.File]::Exists($Path)) {
    try { [IO.File]::SetAttributes($Path, [IO.FileAttributes]::Normal) } catch { }
    [IO.File]::Delete($Path)
  }
}

# Hard-link copy of a tree (no data is copied). The seed is only ever
# changed by unlink and recreate, so a snapshot made this way never changes.
function Link-Tree([string]$Src, [string]$Dst) {
  if ($script:IsWin) {
    Load-Native
    [void][GowayNative]::LinkTree($Src, $Dst)
  } else {
    & cp -al -- $Src $Dst
    if ($LASTEXITCODE -ne 0) { Die "cannot link $Src to $Dst" }
  }
}

# The sidecar list of executable files (NTFS has no execute bit).
function Load-Exec([string]$File) {
  $set = New-Object 'System.Collections.Generic.HashSet[string]'
  if ([IO.File]::Exists($File)) {
    foreach ($p in (Split-Nul ([IO.File]::ReadAllBytes($File)))) { [void]$set.Add($p) }
  }
  return ,$set
}
function Save-Exec([string]$File, $Set) {
  $ms = New-Object IO.MemoryStream
  foreach ($p in ($Set | Sort-Object)) {
    $b = $script:Utf8.GetBytes($p); $ms.Write($b, 0, $b.Length); $ms.WriteByte(0)
  }
  [IO.File]::WriteAllBytes($File, $ms.ToArray())
}

# ---- seeds ------------------------------------------------------------

# Start a new worktree's seed as a hard-link copy of the most recently used
# seed of the same repository, so its first sync only sends differences.
function Seed-FromSibling([string]$Seed) {
  $repoDir = [IO.Path]::GetDirectoryName($Seed)
  $sib = $null; $best = [DateTime]::MinValue
  if (-not [IO.Directory]::Exists($repoDir)) { return }
  foreach ($d in [IO.Directory]::EnumerateDirectories($repoDir)) {
    if ($d -eq $Seed) { continue }
    $meta = [IO.Path]::Combine($d, 'meta.json')
    if (-not ([IO.File]::Exists($meta) -and [IO.Directory]::Exists([IO.Path]::Combine($d, 'tree')))) { continue }
    $t = [IO.File]::GetLastWriteTimeUtc($meta)
    if ($t -gt $best) { $best = $t; $sib = $d }
  }
  if (-not $sib) { return }
  Lock-Dir 'sib' $sib $false
  try {
    $new = [IO.Path]::Combine($Seed, 'tree.new')
    Remove-Tree $new
    Link-Tree ([IO.Path]::Combine($sib, 'tree')) $new
    $modes = [IO.Path]::Combine($sib, 'modes')
    if ([IO.File]::Exists($modes)) { [IO.File]::Copy($modes, [IO.Path]::Combine($Seed, 'modes'), $true) }
    Write-Text ([IO.Path]::Combine($Seed, 'generation')) ((New-Generation) + "`n")
    [IO.Directory]::Move($new, [IO.Path]::Combine($Seed, 'tree'))
  } finally { Unlock-Key 'sib' }
}

function Get-Gen([string]$Seed) {
  $t = [IO.Path]::Combine($Seed, 'tree')
  if (-not [IO.Directory]::Exists($t)) { return '' }
  return (Read-TextOrEmpty ([IO.Path]::Combine($Seed, 'generation'))).Trim()
}

# manifest ROOT SEED: the seed tree as NUL-terminated records
#   type TAB size TAB mtime TAB mode TAB linktarget TAB path
function Verb-manifest([string[]]$A) {
  $root = Get-Root $A[0]; $seed = P $root @('seed', $A[1])
  Mark-Root $root
  $tree = P $seed @('tree')
  if (-not [IO.Directory]::Exists($tree)) {
    Lock-Dir 'seed' $seed $true
    if (-not [IO.Directory]::Exists($tree) -and -not [IO.File]::Exists((P $seed @('fresh')))) { Seed-FromSibling $seed }
  }
  Lock-Dir 'seed' $seed $false
  if (-not [IO.Directory]::Exists($tree)) { return }
  # The generation names this incarnation of the tree; receive refuses to
  # patch a tree whose generation changed since this manifest.
  $sb = New-Object Text.StringBuilder
  [void]$sb.Append("G`t0`t0`t0`t`t").Append((Read-TextOrEmpty (P $seed @('generation'))).Trim()).Append([char]0)
  $exec = Load-Exec (P $seed @('modes'))
  foreach ($f in (Get-Files $tree)) {
    $t = [long]$f[2] - $script:EpochTicks
    $secs = [math]::Floor($t / 10000000); $frac = $t % 10000000
    $mode = if ($exec.Contains($f[0])) { '755' } else { '644' }
    [void]$sb.Append('f').Append("`t").Append($f[1]).Append("`t").Append($secs).Append('.').Append(([long]$frac).ToString('D7')).Append('00').Append("`t").Append($mode).Append("`t`t").Append($f[0]).Append([char]0)
  }
  Write-Out $sb.ToString()
}

function Sha256-Hex([string]$Path) {
  $sha = [Security.Cryptography.SHA256]::Create()
  try {
    $fs = [IO.File]::Open($Path, 'Open', 'Read', 'ReadWrite, Delete')
    try { $h = $sha.ComputeHash($fs) } finally { $fs.Dispose() }
  } finally { $sha.Dispose() }
  return (($h | ForEach-Object { $_.ToString('x2') }) -join '')
}

# hashes ROOT SEED: NUL-separated paths on stdin; print "sha256  path\0".
function Verb-hashes([string[]]$A) {
  $root = Get-Root $A[0]; $seed = P $root @('seed', $A[1])
  Lock-Dir 'seed' $seed $false
  $tree = P $seed @('tree')
  $sb = New-Object Text.StringBuilder
  foreach ($p in (Split-Nul (Read-StdinBytes))) {
    $full = [IO.Path]::Combine($tree, (Native-Path $p))
    if ([IO.File]::Exists($full)) {
      [void]$sb.Append((Sha256-Hex $full)).Append('  ').Append($p).Append([char]0)
    }
  }
  Write-Out $sb.ToString()
}

# deletions ROOT SEED ATTEMPT: paths on stdin to delete at the receive of
# the same sync attempt (a list left by a failed attempt is never applied).
function Verb-deletions([string[]]$A) {
  $root = Get-Root $A[0]; $seed = P $root @('seed', $A[1])
  Test-Id $A[2] 'deletions'
  Mark-Root $root
  Lock-Dir 'seed' $seed $true
  [IO.File]::WriteAllBytes((P $seed @("deletions.$($A[2])")), (Read-StdinBytes))
}

# changes ROOT SEED ATTEMPT: paths on stdin that the receive of the same
# sync attempt will write; receive appends them to the seed's change log.
function Verb-changes([string[]]$A) {
  $root = Get-Root $A[0]; $seed = P $root @('seed', $A[1])
  Test-Id $A[2] 'changes'
  Mark-Root $root
  Lock-Dir 'seed' $seed $true
  [IO.File]::WriteAllBytes((P $seed @("changes.$($A[2])")), (Read-StdinBytes))
}

# ---- tar --------------------------------------------------------------

function Read-Exact([IO.Stream]$S, [byte[]]$Buf, [int]$Count) {
  $got = 0
  while ($got -lt $Count) {
    $n = $S.Read($Buf, $got, $Count - $got)
    if ($n -le 0) { break }
    $got += $n
  }
  return $got
}

function Tar-Field([byte[]]$B, [int]$Off, [int]$Len) {
  $end = $Off
  while ($end -lt ($Off + $Len) -and $B[$end] -ne 0) { $end++ }
  return $script:Utf8.GetString($B, $Off, $end - $Off)
}

function Tar-Number([byte[]]$B, [int]$Off, [int]$Len) {
  if ($B[$Off] -band 0x80) {
    [long]$v = $B[$Off] -band 0x7f
    for ($i = 1; $i -lt $Len; $i++) { $v = ($v -shl 8) -bor $B[$Off + $i] }
    return $v
  }
  $s = (Tar-Field $B $Off $Len).Trim()
  if (-not $s) { return [long]0 }
  return [Convert]::ToInt64($s, 8)
}

function Skip-Bytes([IO.Stream]$S, [long]$Count) {
  $buf = New-Object byte[] 65536
  while ($Count -gt 0) {
    $n = $S.Read($buf, 0, [int][math]::Min($Count, $buf.Length))
    if ($n -le 0) { Die 'the archive ended inside an entry' }
    $Count -= $n
  }
}

function Test-TarPath([string]$Name) {
  if (-not $Name -or $Name.StartsWith('/') -or $Name.Contains('\') -or $Name -match '^[A-Za-z]:') {
    Die "the archive names an unsafe path: $Name"
  }
  foreach ($c in $Name.Split('/')) {
    if ($c -eq '..') { Die "the archive names an unsafe path: $Name" }
  }
}

# Extract a tar stream into $Tree, replacing files by unlink and recreate.
# $Exec is updated from each file's execute bit. Returns the paths written.
function Expand-Tar([IO.Stream]$S, [string]$Tree, $Exec) {
  $hdr = New-Object byte[] 512
  $buf = New-Object byte[] 65536
  $longName = $null
  $written = New-Object System.Collections.Generic.List[string]
  while ($true) {
    if ((Read-Exact $S $hdr 512) -lt 512) { break }
    $zero = $true
    foreach ($b in $hdr) { if ($b -ne 0) { $zero = $false; break } }
    if ($zero) { break }
    $name = Tar-Field $hdr 0 100
    $type = [char]$hdr[156]
    $size = Tar-Number $hdr 124 12
    $mode = Tar-Number $hdr 100 8
    $mtime = Tar-Number $hdr 136 12
    if ((Tar-Field $hdr 257 6) -eq 'ustar') {
      $prefix = Tar-Field $hdr 345 155
      if ($prefix) { $name = "$prefix/$name" }
    }
    $pad = (512 - ($size % 512)) % 512
    if ($type -eq 'L') {
      $data = New-Object byte[] ([int]$size)
      [void](Read-Exact $S $data ([int]$size)); Skip-Bytes $S $pad
      $longName = $script:Utf8.GetString($data).TrimEnd([char]0)
      continue
    }
    if ($type -eq 'x' -or $type -eq 'g' -or $type -eq 'K') {
      $data = New-Object byte[] ([int]$size)
      [void](Read-Exact $S $data ([int]$size)); Skip-Bytes $S $pad
      if ($type -eq 'x') {
        $text = $script:Utf8.GetString($data)
        foreach ($line in $text.Split("`n")) {
          if ($line -match '^\d+ path=(.*)$') { $longName = $Matches[1] }
          if ($line -match '^\d+ mtime=(\d+)') { $mtime = [long]$Matches[1] }
        }
      }
      continue
    }
    if ($longName) { $name = $longName; $longName = $null }
    $name = $name.TrimEnd('/')
    if ($type -eq '5') {
      Test-TarPath $name
      New-Dir ([IO.Path]::Combine($Tree, (Native-Path $name)))
      continue
    }
    if ($type -eq '0' -or $type -eq [char]0 -or $type -eq '7') {
      Test-TarPath $name
      $dest = [IO.Path]::Combine($Tree, (Native-Path $name))
      $dir = [IO.Path]::GetDirectoryName($dest)
      # A file where a directory (or the reverse) was: replace it.
      $walk = $dir
      while ($walk.Length -gt $Tree.Length) {
        if ([IO.File]::Exists($walk)) { Remove-Tree $walk }
        $walk = [IO.Path]::GetDirectoryName($walk)
      }
      New-Dir $dir
      if ([IO.Directory]::Exists($dest)) { Remove-Tree $dest }
      if ([IO.File]::Exists($dest)) { Remove-Tree $dest }
      $fs = [IO.File]::Open($dest, 'CreateNew', 'Write', 'None')
      try {
        $left = $size
        while ($left -gt 0) {
          $n = $S.Read($buf, 0, [int][math]::Min($left, $buf.Length))
          if ($n -le 0) { Die 'the archive ended inside a file' }
          $fs.Write($buf, 0, $n); $left -= $n
        }
      } finally { $fs.Dispose() }
      Skip-Bytes $S $pad
      [IO.File]::SetLastWriteTimeUtc($dest, (New-Object DateTime ($script:EpochTicks + $mtime * 10000000), ([DateTimeKind]::Utc)))
      if ($mode -band 0x40) { [void]$Exec.Add($name) } else { [void]$Exec.Remove($name) }
      $written.Add($name)
      continue
    }
    if ($type -eq '2') {
      Write-Err "goway-remote: symlink $name not created (symlinks are not supported on this host)`n"
    }
    Skip-Bytes $S ($size + $pad)
  }
  # Drain what follows the end-of-archive blocks.
  $S.CopyTo([IO.Stream]::Null)
  return ,$written
}

# receive ROOT SEED META_B64 GENERATION RUN_ID WORK_META_B64 KEEP ATTEMPT:
# under the seed's exclusive lock, check the generation, apply pending
# deletions, extract the tar on stdin, and (with RUN_ID) snapshot the tree
# into the run's fresh work dir (hard links) in the same critical section.
# Exit 75 with "seed changed" when the generation differs.
function Verb-receive([string[]]$A) {
  $root = Get-Root $A[0]; $seed = P $root @('seed', $A[1])
  Mark-Root $root
  Lock-Dir 'seed' $seed $true
  $gen = Get-Gen $seed
  $attempt = if ($A.Length -gt 7) { $A[7] } else { '' }
  if ($attempt -and $attempt -notmatch '^[A-Za-z0-9-]+$') { Die 'receive: bad attempt id' }
  $stdin = Get-Stdin
  if ($gen -ne $A[3]) {
    foreach ($f in [IO.Directory]::EnumerateFiles($seed, 'deletions.*')) { [IO.File]::Delete($f) }
    foreach ($f in [IO.Directory]::EnumerateFiles($seed, 'changes.*')) { [IO.File]::Delete($f) }
    Write-Err ('goway-remote: seed changed (have "{0}", expected "{1}")' -f $gen, $A[3])
    Write-Err "`n"
    $stdin.CopyTo([IO.Stream]::Null)
    Exit-Verb 75
  }
  $tree = P $seed @('tree')
  if (-not [IO.Directory]::Exists($tree)) {
    New-Dir $tree
    Write-Text (P $seed @('generation')) ((New-Generation) + "`n")
  }
  [IO.File]::WriteAllBytes((P $seed @('meta.json')), [Convert]::FromBase64String($A[2]))
  $exec = Load-Exec (P $seed @('modes'))
  $delFile = if ($attempt) { P $seed @("deletions.$attempt") } else { '' }
  if ($delFile -and [IO.File]::Exists($delFile)) {
    foreach ($p in (Split-Nul ([IO.File]::ReadAllBytes($delFile)))) {
      $full = [IO.Path]::Combine($tree, (Native-Path $p))
      if ([IO.File]::Exists($full)) { Remove-Tree $full }
      [void]$exec.Remove($p)
    }
  }
  foreach ($f in [IO.Directory]::EnumerateFiles($seed, 'deletions.*')) { [IO.File]::Delete($f) }
  # The change log: one numbered file per sync that wrote files (the last
  # LogKeep are kept). A slot reconciled up to number n only needs the
  # entries after n to know which files to compare by content.
  $logDir = P $seed @('changes')
  New-Dir $logDir
  $seqFile = P $seed @('seq')
  $seq = 0
  $seqText = (Read-TextOrEmpty $seqFile).Trim()
  if ($seqText -match '^\d+$') { $seq = [int]$seqText }
  $chg = if ($attempt) { P $seed @("changes.$attempt") } else { '' }
  if ($chg -and [IO.File]::Exists($chg)) {
    $seq++
    [IO.File]::Move($chg, (P $logDir @([string]$seq)))
    Write-Text $seqFile ([string]$seq)
    $names = @([IO.Directory]::EnumerateFiles($logDir) | ForEach-Object { [int][IO.Path]::GetFileName($_) } | Sort-Object)
    if ($names.Count -gt 64) {
      foreach ($n in $names[0..($names.Count - 65)]) { [IO.File]::Delete((P $logDir @([string]$n))) }
    }
  }
  foreach ($f in [IO.Directory]::EnumerateFiles($seed, 'changes.*')) { [IO.File]::Delete($f) }
  $fresh = P $seed @('fresh')
  if ([IO.File]::Exists($fresh)) { [IO.File]::Delete($fresh) }
  [void](Expand-Tar $stdin $tree $exec)
  Remove-EmptyDirs $tree $false
  Save-Exec (P $seed @('modes')) $exec
  if ($A.Length -gt 4 -and $A[4]) {
    $work = P $root @('work', $A[4])
    New-Dir $work
    # What keeps gc from removing this dir before its run takes the lock, whatever
    # the wall clock does: this process while it lives, then the monotonic time
    # since boot (see Test-WorkYoung).
    Write-Text (P $work @('creator')) ("{0} {1}`n" -f $PID, (Proc-Start $PID))
    Write-Text (P $work @('born')) ("{0}`n" -f (Uptime-Secs))
    [IO.File]::WriteAllBytes((P $work @('meta.json')), [Convert]::FromBase64String($A[5]))
    if ($A.Length -gt 6 -and $A[6] -eq '1') { [IO.File]::WriteAllBytes((P $work @('keep')), @()) }
    Write-Text (P $work @('seed')) $A[1]
    Write-Text (P $work @('seqinfo')) ("{0} {1}" -f (Read-TextOrEmpty (P $seed @('generation'))).Trim(), $seq)
    # A hard-link snapshot: no data is copied, and nothing writes through it.
    Link-Tree $tree (P $work @('tree'))
    New-Dir (P $work @('changes'))
    foreach ($f in [IO.Directory]::EnumerateFiles($logDir)) {
      [IO.File]::Copy($f, (P $work @('changes', [IO.Path]::GetFileName($f))), $true)
    }
    if ([IO.File]::Exists((P $seed @('modes')))) { [IO.File]::Copy((P $seed @('modes')), (P $work @('modes')), $true) }
  }
}

# ---- running native programs -------------------------------------------

# Quote $Args as one Windows command line (the rules CommandLineToArgvW and
# .NET undo), so every argument arrives exactly as given.
function Join-WinArgs([string[]]$Words) {
  $parts = foreach ($w in $Words) {
    if ($w.Length -gt 0 -and $w -notmatch '[\s"]') { $w; continue }
    $sb = New-Object Text.StringBuilder
    [void]$sb.Append('"')
    $bs = 0
    foreach ($c in $w.ToCharArray()) {
      if ($c -eq '\') { $bs++; continue }
      if ($c -eq '"') { [void]$sb.Append('\', $bs * 2 + 1).Append('"'); $bs = 0; continue }
      if ($bs) { [void]$sb.Append('\', $bs); $bs = 0 }
      [void]$sb.Append($c)
    }
    if ($bs) { [void]$sb.Append('\', $bs * 2) }
    [void]$sb.Append('"')
    $sb.ToString()
  }
  return ($parts -join ' ')
}

# The file the shell would run for $Name (a path with a separator is taken
# as is), or $null.
function Resolve-Program([string]$Name) {
  if ($Name.Contains('/') -or $Name.Contains('\')) {
    if ([IO.File]::Exists($Name)) { return [IO.Path]::GetFullPath($Name) }
    return $null
  }
  $c = Get-Command -Name $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($c) { return $c.Source }
  return $null
}

# Run a program to completion with an empty standard input; returns
# @(exit code, stdout bytes). $Env adds environment variables. Standard
# error is discarded. There is deliberately no way to feed it input: under
# Windows PowerShell 5.1 the redirected stdin is a StreamWriter in the
# console code page, and with code page 65001 it sends a UTF-8 BOM first
# (what made git read "\uFEFFout" as a path on CI).
function Invoke-Native([string]$Exe, [string[]]$Words, [hashtable]$Env) {
  $psi = New-Object Diagnostics.ProcessStartInfo
  $psi.FileName = $Exe
  $psi.Arguments = Join-WinArgs $Words
  $psi.UseShellExecute = $false
  $psi.RedirectStandardInput = $true
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  if ($Env) { foreach ($k in $Env.Keys) { $psi.EnvironmentVariables[$k] = [string]$Env[$k] } }
  $p = [Diagnostics.Process]::Start($psi)
  $errTask = $p.StandardError.BaseStream.CopyToAsync([IO.Stream]::Null)
  $p.StandardInput.Close()
  $ms = New-Object IO.MemoryStream
  $p.StandardOutput.BaseStream.CopyTo($ms)
  $p.WaitForExit()
  $errTask.Wait()
  return @($p.ExitCode, $ms.ToArray())
}

# The first line of `$Exe $Words` (a tool's version), or '' on failure.
function Tool-Version([string]$Exe, [string[]]$Words) {
  try {
    $r = Invoke-Native $Exe $Words $null
    if ($r[0] -ne 0) { return '' }
    $t = $script:Utf8.GetString($r[1]).Trim()
    $nl = $t.IndexOf("`n")
    if ($nl -ge 0) { $t = $t.Substring(0, $nl) }
    return $t.Trim()
  } catch { return '' }
}

# ---- facts --------------------------------------------------------------

function Get-Arch {
  if ($script:IsWin) {
    switch ($env:PROCESSOR_ARCHITECTURE) {
      'AMD64' { return 'x86_64' }
      'ARM64' { return 'aarch64' }
      'x86' { return 'i686' }
      default { return [string]$env:PROCESSOR_ARCHITECTURE }
    }
  }
  $r = Invoke-Native (Resolve-Program 'uname') @('-m') $null
  return $script:Utf8.GetString($r[1]).Trim()
}

function Get-OsName { if ($script:IsWin) { return 'windows' } else { return 'linux' } }

function Get-Mem {
  # @(total bytes, available bytes) or $null
  if ($script:IsWin) {
    try {
      Load-Native
      $m = [GowayNative]::Mem()
      if ($m) { return @($m[0], $m[1]) }
    } catch { }
    $o = Get-CimInstance Win32_OperatingSystem
    return @(([long]$o.TotalVisibleMemorySize * 1024), ([long]$o.FreePhysicalMemory * 1024))
  }
  $t = 0; $a = 0
  foreach ($l in [IO.File]::ReadAllLines('/proc/meminfo')) {
    if ($l -match '^MemTotal:\s+(\d+)') { $t = [long]$Matches[1] * 1024 }
    if ($l -match '^MemAvailable:\s+(\d+)') { $a = [long]$Matches[1] * 1024 }
  }
  return @($t, $a)
}

function Get-Load {
  $cores = [Environment]::ProcessorCount
  if ($script:IsWin) {
    $pct = -1.0
    try { Load-Native; $pct = [GowayNative]::Load(60) } catch { }
    if ($pct -lt 0) {
      $pct = (Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average
      if ($null -eq $pct) { $pct = 0 }
    }
    $l = ($cores * $pct / 100.0).ToString('0.00', [Globalization.CultureInfo]::InvariantCulture)
    return @($l, $l, $l)
  }
  $f = (Read-TextOrEmpty '/proc/loadavg').Split(' ')
  return @($f[0], $f[1], $f[2])
}

function Cmd-Exists([string]$Name) { return [bool](Resolve-Program $Name) }

function GPU-Query([string]$Query) {
  $exe = Resolve-Program 'nvidia-smi'
  if (-not $exe) { return @() }
  try {
    $r = Invoke-Native $exe @("--query-gpu=$Query", '--format=csv,noheader,nounits') $null
    if ($r[0] -ne 0) { return @() }
    return @($script:Utf8.GetString($r[1]).Split("`n") | ForEach-Object { $_.Trim("`r") } | Where-Object { $_ })
  } catch { return @() }
}

function Static-Facts {
  $sb = New-Object Text.StringBuilder
  [void]$sb.Append("static=1`n")
  $n = 0
  $exe = Resolve-Program 'nvidia-smi'
  if ($exe) {
    $cuda = ''
    try {
      $r = Invoke-Native $exe @() $null
      if ($script:Utf8.GetString($r[1]) -match 'CUDA Version: ([0-9.]+)') { $cuda = $Matches[1] }
    } catch { }
    foreach ($line in (GPU-Query 'name,memory.total,driver_version')) {
      $f = $line.Split(',')
      if ($f.Length -lt 3) { continue }
      $name = $f[0].Replace('|', '').Trim()
      $mem = ($f[1] -replace '[^0-9]', '')
      $drv = $f[2].Replace(' ', '')
      [void]$sb.Append("gpu.$n=nvidia|$name|$mem|$drv|$cuda`n")
      $n++
    }
  }
  $flags = @()
  if ($script:IsWin) {
    Load-Native
    if ([GowayNative]::Feature(40)) { $flags += 'avx2' }
    if ([GowayNative]::Feature(41)) { $flags += 'avx512f' }
    if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { $flags += 'neon' }
  } elseif ([IO.File]::Exists('/proc/cpuinfo')) {
    $info = [IO.File]::ReadAllText('/proc/cpuinfo')
    foreach ($f in @('avx2', 'avx512f', 'neon')) {
      $pat = if ($f -eq 'neon') { '(neon|asimd)' } else { $f }
      if ($info -match "(?m)^(flags|Features)\s*:.*\s$pat(\s|$)") { $flags += $f }
    }
  }
  [void]$sb.Append("cpu_flags=$($flags -join ',')`n")
  $kvm = 0
  if (-not $script:IsWin -and [IO.File]::Exists('/dev/kvm')) {
    try { $h = [IO.File]::Open('/dev/kvm', 'Open', 'ReadWrite'); $h.Dispose(); $kvm = 1 } catch { }
  }
  [void]$sb.Append("kvm=$kvm`n")
  $docker = 0
  $d = Resolve-Program 'docker'
  if ($d) { try { if ((Invoke-Native $d @('info') $null)[0] -eq 0) { $docker = 1 } } catch { } }
  [void]$sb.Append("docker=$docker`n")
  $win = ''
  if ($script:IsWin) {
    try { $win = ((Get-CimInstance Win32_VideoController).Name -join ';') } catch { }
  }
  [void]$sb.Append("wsl=0`nwinvideo=$win`n")
  return $sb.ToString()
}

function Dir-Bytes([string]$Dir) {
  if (-not [IO.Directory]::Exists($Dir)) { return 0 }
  [long]$sum = 0
  $di = New-Object IO.DirectoryInfo $Dir
  foreach ($f in $di.EnumerateFiles('*', [IO.SearchOption]::AllDirectories)) { $sum += $f.Length }
  return $sum
}

function Free-Bytes([string]$Path) {
  try {
    $full = [IO.Path]::GetFullPath($Path)
    $best = $null
    foreach ($d in [IO.DriveInfo]::GetDrives()) {
      if ($d.IsReady -and $full.StartsWith($d.Name, [StringComparison]::OrdinalIgnoreCase)) {
        if (-not $best -or $d.Name.Length -gt $best.Name.Length) { $best = $d }
      }
    }
    if ($best) { return [long]$best.AvailableFreeSpace }
  } catch { }
  return 0
}

# The size in bytes of the drive holding $Path (0 when unknown).
function Total-Bytes([string]$Path) {
  try {
    $full = [IO.Path]::GetFullPath($Path)
    $best = $null
    foreach ($d in [IO.DriveInfo]::GetDrives()) {
      if ($d.IsReady -and $full.StartsWith($d.Name, [StringComparison]::OrdinalIgnoreCase)) {
        if (-not $best -or $d.Name.Length -gt $best.Name.Length) { $best = $d }
      }
    }
    if ($best) { return [long]$best.TotalSize }
  } catch { }
  return 0
}

# The disk budget in bytes: $Max when set (> 0), else the smaller of 20% of
# the drive holding $Path and 50 GiB (the same rule as remote.sh).
function Budget-Max([string]$Path, [long]$Max) {
  if ($Max -gt 0) { return $Max }
  $total = Total-Bytes $Path
  return [math]::Min([long][math]::Floor($total / 5), [long]53687091200)
}

# Binary units, one decimal ("3.1 GiB").
function Human-Bytes([double]$B) {
  $u = @('B', 'KiB', 'MiB', 'GiB', 'TiB'); $i = 0
  while ($B -ge 1024 -and $i -lt 4) { $B /= 1024; $i++ }
  if ($i -eq 0) { return ('{0} B' -f [long]$B) }
  return ('{0:F1} {1}' -f $B, $u[$i])
}

# want.TOOL=<first version line> for each safe tool name, `want.TOOL=` when missing
# (names with anything outside [A-Za-z0-9._+-] are skipped; the Unix side is want_facts).
function Want-Facts([string[]]$Names) {
  $sb = New-Object Text.StringBuilder
  foreach ($t in $Names) {
    if (-not $t -or $t -notmatch '^[A-Za-z0-9._+-]+$') { continue }
    $exe = Resolve-Program $t
    $v = ''
    if ($exe) {
      $v = if ($t -eq 'go') { Tool-Version $exe @('version') } elseif ($t -eq 'java') { Tool-Version $exe @('-version') } else { Tool-Version $exe @('--version') }
      if ($null -eq $v) { $v = '' }
    }
    [void]$sb.Append("want.$t=$v`n")
  }
  return $sb.ToString()
}

# probe ROOT [disk] [budget:MAX:MIN_FREE] [static] [tools:A,B]: key=value facts for scheduling and status.
function Verb-probe([string[]]$A) {
  $root = Get-Root $A[0]
  $wantDisk = $false; $wantStatic = $false; $budget = $null; $tools = @()
  foreach ($x in $A[1..([math]::Max($A.Length - 1, 1))]) {
    if ($x -eq 'disk') { $wantDisk = $true }
    if ($x -eq 'static') { $wantStatic = $true }
    if ($x -match '^budget:(\d+):(\d+)$') { $budget = @([long]$Matches[1], [long]$Matches[2]) }
    if ($x -like 'tools:*') { $tools = @($x.Substring(6).Split(',')) }
  }
  $sb = New-Object Text.StringBuilder
  $m = Get-Mem
  if ($m[0]) { [void]$sb.Append("mem_total=$($m[0])`n") }
  if ($m[1]) { [void]$sb.Append("mem_avail=$($m[1])`n") }
  if ($wantStatic) { [void]$sb.Append((Static-Facts)) }
  [void]$sb.Append("arch=$(Get-Arch)`nhostname=$([Environment]::MachineName)`ncores=$([Environment]::ProcessorCount)`n")
  [void]$sb.Append("os=$(Get-OsName)`n")
  [void]$sb.Append("epoch=$(Unix-Secs)`n")
  $l = Get-Load
  [void]$sb.Append("load1=$($l[0])`nload5=$($l[1])`nload15=$($l[2])`n")
  $jobs = 0
  $workDir = P $root @('work')
  if ([IO.Directory]::Exists($workDir)) {
    foreach ($d in [IO.Directory]::EnumerateDirectories($workDir)) {
      $lock = [IO.Path]::Combine($d, 'lock')
      if (-not [IO.File]::Exists($lock)) { continue }
      $fs = Get-Lock $lock $true 0
      if ($fs) { $fs.Dispose() } else { $jobs++ }
    }
  }
  [void]$sb.Append("jobs=$jobs`n")
  if ($wantDisk) {
    [void]$sb.Append("disk_used=$(Dir-Bytes $root)`ndisk_free=$(Free-Bytes (Get-Home))`n")
    if ($budget) { [void]$sb.Append("disk_max=$(Budget-Max (Get-Home) $budget[0])`ndisk_min_free=$($budget[1])`n") }
  }
  if ($tools.Count) { [void]$sb.Append((Want-Facts $tools)) }
  Write-Out $sb.ToString()
}

# doctor ROOT: key=value facts about the toolchain and host.
function Verb-doctor([string[]]$A) {
  $sb = New-Object Text.StringBuilder
  [void]$sb.Append("cargo_env=no`n")
  $tools = @('bash', 'git', 'tar', 'flock', 'setsid', 'cc', 'curl', 'rustup', 'cargo', 'cargo-nextest', 'sccache', 'winget', 'apt-get', 'dnf', 'pacman')
  foreach ($t in $tools) {
    $exe = Resolve-Program $t
    if (-not $exe -and $t -eq 'cc') {
      foreach ($c in @('cl', 'gcc', 'clang')) { $exe = Resolve-Program $c; if ($exe) { break } }
    }
    if ($exe) {
      if ($t -eq 'cargo-nextest') { $v = Tool-Version $exe @('nextest', '--version') }
      elseif ($t -in @('cc', 'cl', 'winget')) { $v = '' }
      else { $v = Tool-Version $exe @('--version') }
      if (-not $v) { $v = 'present' }
      [void]$sb.Append("tool.$t=$v`n")
    } else {
      [void]$sb.Append("tool.$t=`n")
    }
  }
  [void]$sb.Append("epoch=$(Unix-Secs)`n")
  if ($A.Length -gt 1) { [void]$sb.Append((Want-Facts ([string[]]$A[1..($A.Length - 1)]))) }
  $os = 'unknown'
  if ($script:IsWin) { try { $os = (Get-CimInstance Win32_OperatingSystem).Caption } catch { } }
  else { $os = Get-OsName }
  [void]$sb.Append("os=$os`narch=$(Get-Arch)`ndisk_free=$(Free-Bytes (Get-Home))`n")
  $pa = 'default-yes'
  $cfg = if ($script:IsWin) { [IO.Path]::Combine($env:ProgramData, 'ssh', 'sshd_config') } else { '/etc/ssh/sshd_config' }
  if ([IO.File]::Exists($cfg)) {
    foreach ($l in [IO.File]::ReadAllLines($cfg)) {
      if ($l -match '^\s*PasswordAuthentication\s+(\S+)') { $pa = $Matches[1].ToLowerInvariant(); break }
    }
  }
  [void]$sb.Append("password_auth=$pa`nhome=$(Get-Home)`n")
  [void]$sb.Append((Static-Facts))
  $ch = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { [IO.Path]::Combine((Get-Home), '.cargo') }
  $has = if ([IO.Directory]::Exists($ch) -or [IO.File]::Exists($ch)) { 1 } else { 0 }
  [void]$sb.Append("cargo_home=$has`n")
  Write-Out $sb.ToString()
}

# How long (milliseconds) lifeline waits for the client's next heartbeat byte
# (GOWAY_LIFELINE_TIMEOUT, in seconds, overrides it for tests). Long on purpose: an
# overloaded laptop sends its beats late, and silence alone is never proof that the
# client is gone (end of input is, and stops the job at once).
$script:LifelineTimeoutMs = 120000
if ($env:GOWAY_LIFELINE_TIMEOUT -match '^\d+$') { $script:LifelineTimeoutMs = 1000 * [int]$env:GOWAY_LIFELINE_TIMEOUT }

# Lost-Note WORK: say why the helper stopped this run, when its lifeline did.
function Lost-Note([string]$Work) {
  $why = (Read-TextOrEmpty (P $Work @('lost'))).Trim()
  if (-not $why) { return }
  Write-Err "goway: this run was stopped by the helper: the client was considered gone because $why`n"
}

# lifeline ROOT RUN_ID: the run's lifeline. The client keeps this call open and
# writes a byte to its stdin every few seconds. When stdin ends (the client
# died) the run is stopped at once; when no byte arrives for the timeout (laptop
# asleep, network gone) it is stopped too. The reason goes into the lost marker. The
# run's job process tree is stopped when the job started, else the run's own
# process. A run that already finished (its work dir is gone or marked done) is
# left alone. The same contract as remote.sh.
function Verb-lifeline([string[]]$A) {
  $root = Get-Root $A[0]; Test-Id $A[1] 'lifeline'
  $work = P $root @('work', $A[1])
  $live = { [IO.Directory]::Exists($work) -and -not [IO.File]::Exists((P $work @('done'))) }
  $in = [Console]::OpenStandardInput()
  $buf = New-Object byte[] 1
  $silent = $false
  while ($true) {
    $t = $in.ReadAsync($buf, 0, 1)
    if (-not $t.Wait($script:LifelineTimeoutMs)) { $silent = $true; break }
    if ($t.Result -le 0) { break }
    if (-not (& $live)) { return }
  }
  if (-not (& $live)) { return }
  if ($silent) { $why = "its heartbeat was silent for $([int]($script:LifelineTimeoutMs / 1000))s (laptop asleep or the network gone)" }
  else { $why = 'its lifeline connection closed (the client exited or lost its network)' }
  try { Write-Text (P $work @('lost')) "$why`n" } catch { return }
  $pidText = (Read-TextOrEmpty (P $work @('pid'))).Trim()
  if ($pidText -match '^\d+$') {
    $p = Get-Process -Id ([int]$pidText) -ErrorAction SilentlyContinue
    if ($p) {
      Write-Err "goway-remote: client of run $($A[1]) is gone ($why); stopping its job`n"
      Stop-Tree $p
      return
    }
  }
  $runner = (Read-TextOrEmpty (P $work @('runner'))).Trim()
  if ($runner -notmatch '^\d+$') { return }
  $r = Get-Process -Id ([int]$runner) -ErrorAction SilentlyContinue
  if ($r) { try { $r.Kill() } catch { } }
}

# Whether $Path is a reparse point (a symlink, or the Windows Store's zero-byte app
# execution alias): refused by its attributes, not by its name.
function Test-Reparse([string]$Path) {
  try { return ([IO.File]::GetAttributes($Path) -band [IO.FileAttributes]::ReparsePoint) -ne 0 } catch { return $true }
}

# The absolute path of the file $Name on a PATH directory, else $null. Only absolute
# directories outside $Root (goway's own state, where the synced work tree lives) are
# searched: never the current directory, never a relative entry, so a repository cannot
# ship a look-alike. WindowsApps (the Store alias directory) and reparse points never count.
function Find-OnPath([string]$Name, [string]$Root) {
  $rootFull = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
  $cmp = if ($script:IsWin) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
  foreach ($d in ([string]$env:PATH).Split([IO.Path]::PathSeparator)) {
    if (-not $d -or -not [IO.Path]::IsPathRooted($d)) { continue }
    try { $dir = [IO.Path]::GetFullPath($d).TrimEnd('\', '/') } catch { continue }
    if (($dir + [IO.Path]::DirectorySeparatorChar).StartsWith($rootFull, $cmp)) { continue }
    if ($script:IsWin -and ($dir -split '[\\/]') -contains 'WindowsApps') { continue }
    $f = [IO.Path]::Combine($dir, $Name)
    if (-not [IO.File]::Exists($f)) { continue }
    if (Test-Reparse $f) { continue }
    return $f
  }
  return $null
}

# Run $Exe $Words with no input, output discarded; true when it exits 0 within $Secs.
function Test-Check([string]$Exe, [string[]]$Words, [int]$Secs) {
  try {
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    $psi.Arguments = Join-WinArgs $Words
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $p = [Diagnostics.Process]::Start($psi)
    $o = $p.StandardOutput.BaseStream.CopyToAsync([IO.Stream]::Null)
    $e = $p.StandardError.BaseStream.CopyToAsync([IO.Stream]::Null)
    $p.StandardInput.Close()
    if (-not $p.WaitForExit($Secs * 1000)) { try { Stop-Tree $p } catch { }; return $false }
    return ($p.ExitCode -eq 0)
  } catch { return $false }
}

# discard ROOT RUN_ID: remove the synced work dir of a run that will not start (as
# remote.sh discard). Best effort: it never fails; gc collects what it cannot remove.
function Verb-discard([string[]]$A) {
  $root = Get-Root $A[0]; Test-Id $A[1] 'discard'
  Remove-Work (P $root @('work', $A[1]))
}

# resolve ROOT RUN_ID: for portable command translation (the same contract as remote.sh).
# stdin holds candidate lines `tier;kind;name;args;check`. Prints one line: `same`,
# `ok;PROGRAM;ARGS` (a bare name resolves to an absolute PATH file; a work-tree file keeps
# its path relative to the tree, as `.\x\y.exe`) or `none;WHY`. Any trouble is `none`.
function Verb-resolve([string[]]$A) {
  $root = Get-Root $A[0]; Test-Id $A[1] 'resolve'
  $tree = P $root @('work', $A[1], 'tree')
  $lines = @(([Text.Encoding]::UTF8.GetString((Read-StdinBytes)) -split "`n") | Where-Object { $_.Trim() } | Select-Object -First 16)
  $cands = @()
  foreach ($l in $lines) {
    $f = $l.TrimEnd("`r").Split(';')
    if ($f.Length -ne 5 -or $f[0] -notmatch '^\d+$') { continue }
    $cands += [pscustomobject]@{ Tier = [int]$f[0]; Kind = $f[1]; Name = $f[2]; Args = $f[3]; Check = $f[4] }
  }
  foreach ($t in @($cands | ForEach-Object { $_.Tier } | Sort-Object -Unique)) {
    $hits = @()
    foreach ($c in @($cands | Where-Object { $_.Tier -eq $t })) {
      $prog = $null
      if ($c.Kind -eq 'tree') {
        $rel = $c.Name
        if ($rel -notmatch '^[A-Za-z0-9._+ /-]+$' -or ($rel -split '/') -contains '..' -or ($rel -split '/') -contains '.' -or $rel.StartsWith('/')) { continue }
        $full = [IO.Path]::Combine($tree, (Native-Path $rel))
        if (-not [IO.File]::Exists($full) -or (Test-Reparse $full)) { continue }
        $prog = '.\' + $rel.Replace('/', '\')
      } elseif ($c.Kind -eq 'bare' -or $c.Kind -eq 'same') {
        if ($c.Name -notmatch '^[A-Za-z0-9._+-]+$') { continue }
        $prog = Find-OnPath $c.Name $root
        if (-not $prog) { continue }
        if ($c.Check -and -not (Test-Check $prog ([string[]]$c.Check.Split(',')) 15)) { continue }
      } else { continue }
      $hits += [pscustomobject]@{ Kind = $c.Kind; Prog = $prog; Args = $c.Args }
    }
    if ($hits.Count -gt 1) { Write-Out "none;several candidates match`n"; return }
    if ($hits.Count -eq 1) {
      $h = $hits[0]
      if ($h.Kind -eq 'same') { Write-Out "same`n"; return }
      if ($h.Prog.Contains(';') -or $h.Prog.Contains(',')) { Write-Out "none;unsafe path`n"; return }
      Write-Out ("ok;{0};{1}`n" -f $h.Prog, $h.Args)
      return
    }
  }
  Write-Out "none;nothing found`n"
}

# envfile ROOT RUN_ID: store the run's --env values (NUL-separated on stdin)
# in its work dir.
function Verb-envfile([string[]]$A) {
  $root = Get-Root $A[0]; $work = P $root @('work', $A[1])
  Test-Id $A[1] 'envfile: run'
  if (-not [IO.Directory]::Exists($work)) { Die "envfile: no work dir $work" }
  [IO.File]::WriteAllBytes((P $work @('env')), (Read-StdinBytes))
}

# argsfile ROOT RUN_ID: store the run's command words (NUL-separated on
# stdin) in its work dir; `run ... -- @args` reads them from there (a
# command line to Windows OpenSSH is limited to a few thousand characters).
function Verb-argsfile([string[]]$A) {
  $root = Get-Root $A[0]; $work = P $root @('work', $A[1])
  Test-Id $A[1] 'argsfile: run'
  if (-not [IO.Directory]::Exists($work)) { Die "argsfile: no work dir $work" }
  [IO.File]::WriteAllBytes((P $work @('args')), (Read-StdinBytes))
}

# ---- gc ------------------------------------------------------------------

# A work dir with no lock file yet and younger than this (seconds) is never removed by gc.
$script:WorkGrace = 120

# Seconds since boot by the monotonic tick counter, which a wall-clock jump
# cannot move (0 when unknown).
function Uptime-Secs { try { return [long]([Environment]::TickCount64 / 1000) } catch { return [long]([Environment]::TickCount / 1000) } }

# "pid start-ticks" of this process, so a recycled pid is not mistaken for it.
function Proc-Start([int]$ProcId) {
  try { return [string](Get-Process -Id $ProcId -ErrorAction Stop).StartTime.ToUniversalTime().Ticks } catch { return '' }
}

# Whether the run that owns work dir $Dir is still starting: its creator process
# is alive, or the dir is younger than WorkGrace by the monotonic clock. Never
# judged by wall-clock age, which a clock jump can make huge. A dir without the
# markers is not young by this test.
function Test-WorkYoung([string]$Dir) {
  $c = (Read-TextOrEmpty ([IO.Path]::Combine($Dir, 'creator'))).Trim().Split(' ')
  if ($c.Length -ge 1 -and $c[0] -match '^\d+$') {
    $start = if ($c.Length -ge 2) { $c[1] } else { '' }
    $now = Proc-Start ([int]$c[0])
    if ($now -and (-not $start -or $now -eq $start)) { return $true }
  }
  $born = (Read-TextOrEmpty ([IO.Path]::Combine($Dir, 'born'))).Trim()
  if ($born -notmatch '^\d+$') { return $false }
  $up = Uptime-Secs
  return ($up -ge [long]$born -and ($up - [long]$born) -lt $script:WorkGrace)
}

# What the last Gc-Entry/Evict-Slot decided (action and bytes), the bytes gc
# has removed or would remove so far, and the paths a dry run lists as gone,
# so a dry-run eviction never counts them twice.
$script:GcAction = ''
$script:GcBytes = [long]0
$script:GcFreed = [long]0
$script:GcGone = New-Object 'System.Collections.Generic.HashSet[string]'

function Repo-Of([string]$Dir) {
  $meta = Read-TextOrEmpty ([IO.Path]::Combine($Dir, 'meta.json'))
  $name = '-'; $id = '-'
  if ($meta -match '"repo":"([^"]*)"') { $name = $Matches[1] }
  if ($meta -match '"repo_id":"([^"]*)"') { $id = $Matches[1] }
  return @($name, $id)
}

function Age-Of([string]$Dir, [long]$Now) {
  $meta = [IO.Path]::Combine($Dir, 'meta.json')
  try {
    if ([IO.File]::Exists($meta)) { $t = [IO.File]::GetLastWriteTimeUtc($meta) }
    else { $t = [IO.Directory]::GetLastWriteTimeUtc($Dir) }
    return $Now - [long]([DateTimeOffset]$t).ToUnixTimeSeconds()
  } catch { return 0 }
}

# Test-WorkAlive DIR: whether the run that owns work dir DIR still has a live runner or
# job process (its recorded pids): liveness by process, never by age, as a second guard
# behind the dir's lock (as remote.sh work_alive).
function Test-WorkAlive([string]$Dir) {
  foreach ($f in @('runner', 'pid')) {
    $t = (Read-TextOrEmpty ([IO.Path]::Combine($Dir, $f))).Trim()
    if ($t -notmatch '^\d+$') { continue }
    if (Get-Process -Id ([int]$t) -ErrorAction SilentlyContinue) { return $true }
  }
  return $false
}

# Decide one entry: print "action TAB kind TAB age TAB bytes TAB repo TAB id
# TAB path" and remove it when the action is "remove" and $Mode is apply. The
# entry's locks are taken exclusively (non-blocking) while it is judged.
function Gc-Entry([string]$Kind, [string]$Dir, [long]$Ttl, [long]$Now, [string]$Mode, [string]$RepoFilter, [string]$Verb = 'remove') {
  $script:GcAction = 'skip'; $script:GcBytes = 0
  if (-not [IO.Directory]::Exists($Dir)) { return }
  $repo = Repo-Of $Dir
  if ($RepoFilter -and $repo[0] -ne $RepoFilter -and $repo[1] -ne $RepoFilter) { return }
  $locks = @()
  if ($Kind -eq 'cache') {
    $locks = @([IO.Directory]::EnumerateFiles($Dir, 'target-*.lock'))
  } else {
    $locks = @([IO.Path]::Combine($Dir, 'lock'))
  }
  # Only entries goway labelled as this kind are ever removed.
  if ((Read-TextOrEmpty ([IO.Path]::Combine($Dir, 'meta.json'))) -notmatch ('"kind":"{0}"' -f $Kind)) {
    Write-Out ("unlabelled`t$Kind`t0`t0`t-`t-`t$Dir`n")
    return
  }
  $action = 'keep'
  $held = @()
  foreach ($l in $locks) {
    if (-not [IO.File]::Exists($l)) { continue }
    $fs = Get-Lock $l $true 0
    if ($fs) { $held += $fs } else { $action = 'busy' }
  }
  $age = Age-Of $Dir $Now
  if ($action -eq 'keep' -and $age -ge $Ttl) { $action = $Verb }
  # A run creates its work dir in one call and its lock in the next: never
  # remove such a young dir, not even with --all.
  if ($Kind -eq 'work' -and $action -eq $Verb -and -not [IO.File]::Exists([IO.Path]::Combine($Dir, 'lock')) -and $age -lt $script:WorkGrace) { $action = 'keep' }
  # A run that is starting is protected by liveness, not by wall-clock age: if the
  # host's clock jumps forward every age is huge, and this dir must still survive.
  if ($Kind -eq 'work' -and $action -eq $Verb -and (Test-WorkYoung $Dir)) { $action = 'keep' }
  # A work dir whose run is alive is never taken, whatever its lock or age say.
  if ($Kind -eq 'work' -and $action -eq $Verb -and (Test-WorkAlive $Dir)) { $action = 'busy' }
  $bytes = Dir-Bytes $Dir
  Write-Out ("$action`t$Kind`t$age`t$bytes`t$($repo[0])`t$($repo[1])`t$Dir`n")
  $script:GcAction = $action; $script:GcBytes = $bytes
  if ($action -eq $Verb) { $script:GcFreed += $bytes; [void]$script:GcGone.Add($Dir) }
  # A handle blocks deletion on Windows: let go of the locks first.
  foreach ($fs in $held) { $fs.Dispose() }
  if ($action -eq $Verb -and $Mode -eq 'apply') {
    try { Remove-Tree $Dir } catch { Write-Err "goway-remote: cannot remove ${Dir}: $($_.Exception.Message)`n" }
  }
}

# Evict-Slot CACHE_DIR K NOW MODE REPO: evict one build slot (its tree-K and
# target-K) of a per-repository cache when its lock is free. The slot's age
# is its lock file's mtime, read after the lock is held.
function Evict-Slot([string]$Dir, [int]$K, [long]$Now, [string]$Mode, [string]$RepoFilter) {
  $script:GcAction = 'skip'; $script:GcBytes = 0
  $lock = [IO.Path]::Combine($Dir, "target-$K.lock")
  $tree = [IO.Path]::Combine($Dir, "tree-$K"); $target = [IO.Path]::Combine($Dir, "target-$K")
  if (-not [IO.File]::Exists($lock)) { return }
  if (-not ([IO.Directory]::Exists($tree) -or [IO.Directory]::Exists($target))) { return }
  if ((Read-TextOrEmpty ([IO.Path]::Combine($Dir, 'meta.json'))) -notmatch '"kind":"cache"') { return }
  $repo = Repo-Of $Dir
  if ($RepoFilter -and $repo[0] -ne $RepoFilter -and $repo[1] -ne $RepoFilter) { return }
  $fs = Get-Lock $lock $true 0
  if (-not $fs) {
    Write-Out ("busy`tslot`t0`t0`t$($repo[0])`t$($repo[1])`t$tree`n")
    $script:GcAction = 'busy'
    return
  }
  try {
    $age = $Now - [long]([DateTimeOffset][IO.File]::GetLastWriteTimeUtc($lock)).ToUnixTimeSeconds()
    $bytes = (Dir-Bytes $tree) + (Dir-Bytes $target)
    Write-Out ("evict`tslot`t$age`t$bytes`t$($repo[0])`t$($repo[1])`t$tree`n")
    $script:GcAction = 'evict'; $script:GcBytes = $bytes
    $script:GcFreed += $bytes
    if ($Mode -eq 'apply') {
      # The lock file is outside both trees, so the held handle does not block this.
      try { Remove-Tree $tree; Remove-Tree $target } catch { Write-Err "goway-remote: cannot evict slot ${K}: $($_.Exception.Message)`n" }
    }
  } finally { $fs.Dispose() }
}

# Evict-Budget ROOT NOW MODE REPO MAX_DISK MIN_FREE [log]: when goway's root is
# over its budget (MAX_DISK bytes, 0 = auto) or the disk has less than MIN_FREE
# bytes free, evict unlocked entries, least recently used first (build slots,
# then work dirs, seeds, whole repository caches at the same age), until both
# hold. Entries in use are skipped. With "log" a summary is left for the next
# run to print. The same rules as remote.sh (evict).
function Evict-Budget([string]$Root, [long]$Now, [string]$Mode, [string]$Repo, [long]$MaxDisk, [long]$MinFree, [string]$Log) {
  $before = $script:GcFreed
  $max = Budget-Max $Root $MaxDisk
  $used = Dir-Bytes $Root
  $free = Free-Bytes $Root
  # A dry run has not removed what gc listed before this; pretend it did.
  if ($Mode -ne 'apply') {
    $used = [math]::Max($used - $before, 0); $free += $before
  }
  # On a small disk a fixed MIN_FREE could never be met and would empty
  # goway's root after every run: cap it at a quarter of the disk.
  $total = Total-Bytes $Root
  $minFree = [math]::Min($MinFree, [long][math]::Floor($total / 4))
  $need = $used - $max
  if (($minFree - $free) -gt $need) { $need = $minFree - $free }
  if ($need -le 0) { return }
  $list = New-Object System.Collections.Generic.List[object]
  $add = { param($d, $rank, $kind, $k)
    $meta = [IO.Path]::Combine($d, 'meta.json')
    $t = try { if ([IO.File]::Exists($meta)) { [IO.File]::GetLastWriteTimeUtc($meta) } else { [IO.Directory]::GetLastWriteTimeUtc($d) } } catch { [DateTime]::UtcNow }
    $list.Add([pscustomobject]@{ M = [long]([DateTimeOffset]$t).ToUnixTimeSeconds(); Rank = $rank; Kind = $kind; Path = $d; K = $k })
  }
  $work = P $Root @('work')
  if ([IO.Directory]::Exists($work)) { foreach ($d in [IO.Directory]::EnumerateDirectories($work)) { & $add $d 1 'work' 0 } }
  $seed = P $Root @('seed')
  if ([IO.Directory]::Exists($seed)) {
    foreach ($r in [IO.Directory]::EnumerateDirectories($seed)) { foreach ($d in [IO.Directory]::EnumerateDirectories($r)) { & $add $d 1 'seed' 0 } }
  }
  $cache = P $Root @('cache')
  if ([IO.Directory]::Exists($cache)) {
    foreach ($d in [IO.Directory]::EnumerateDirectories($cache)) {
      & $add $d 2 'cache' 0
      foreach ($l in [IO.Directory]::EnumerateFiles($d, 'target-*.lock')) {
        $k = [IO.Path]::GetFileNameWithoutExtension($l).Substring(7)
        if ($k -notmatch '^\d+$') { continue }
        $m = try { [long]([DateTimeOffset][IO.File]::GetLastWriteTimeUtc($l)).ToUnixTimeSeconds() } catch { $Now }
        $list.Add([pscustomobject]@{ M = $m; Rank = 0; Kind = 'slot'; Path = $d; K = [int]$k })
      }
    }
  }
  $freed = [long]0; $count = 0
  $slotLog = @{}
  foreach ($e in @($list | Sort-Object M, Rank)) {
    if ($freed -ge $need) { break }
    if ($script:GcGone.Contains($e.Path)) { continue }
    if ($e.Kind -eq 'slot') { Evict-Slot $e.Path $e.K $Now $Mode $Repo }
    else { Gc-Entry $e.Kind $e.Path 0 $Now $Mode $Repo 'evict' }
    if ($script:GcAction -eq 'evict') {
      $sub = [long]0
      # A dry run has not removed the slots it listed; do not count them twice.
      if ($e.Kind -eq 'cache' -and $Mode -ne 'apply' -and $slotLog.ContainsKey($e.Path)) { $sub = $slotLog[$e.Path] }
      if ($e.Kind -eq 'slot') { $slotLog[$e.Path] = [long]($slotLog[$e.Path]) + $script:GcBytes }
      $freed += $script:GcBytes - $sub
      $count++
    }
  }
  $script:GcFreed = $before + $freed
  if ($Mode -eq 'apply' -and $count -gt 0 -and $Log -eq 'log') {
    $line = 'goway: disk budget: evicted {0} entries, freed {1} (goway used {2} of {3}, {4} free)' -f $count, (Human-Bytes $freed), (Human-Bytes $used), (Human-Bytes $max), (Human-Bytes $free)
    try { [IO.File]::AppendAllText((P $Root @('evicted.log')), $line + "`n", $script:Utf8) } catch { }
  }
}

# gc ROOT NOW CACHE_TTL ORPHAN_TTL KEPT_TTL MODE REPO OLDER_THAN [MAX_DISK MIN_FREE [log]]
function Verb-gc([string[]]$A) {
  $root = Get-Root $A[0]; $now = [long]$A[1]
  $cacheTtl = [long]$A[2]; $orphanTtl = [long]$A[3]; $keptTtl = [long]$A[4]
  $mode = $A[5]
  $repo = if ($A.Length -gt 6) { $A[6] } else { '' }
  $older = if ($A.Length -gt 7) { $A[7] } else { '' }
  if (-not [IO.Directory]::Exists($root)) { return }
  if (-not [IO.File]::Exists((P $root @('.goway-root')))) {
    Write-Err "goway-remote: $root is not marked as goway state; gc removes nothing there`n"
    return
  }
  $work = P $root @('work')
  if ([IO.Directory]::Exists($work)) {
    foreach ($d in @([IO.Directory]::EnumerateDirectories($work))) {
      $ttl = if ([IO.File]::Exists([IO.Path]::Combine($d, 'keep'))) { $keptTtl } else { $orphanTtl }
      if ($older) { $ttl = [long]$older }
      Gc-Entry 'work' $d $ttl $now $mode $repo
    }
  }
  $seed = P $root @('seed')
  if ([IO.Directory]::Exists($seed)) {
    foreach ($r in @([IO.Directory]::EnumerateDirectories($seed))) {
      foreach ($d in @([IO.Directory]::EnumerateDirectories($r))) {
        $ttl = if ($older) { [long]$older } else { $cacheTtl }
        Gc-Entry 'seed' $d $ttl $now $mode $repo
      }
    }
  }
  $cache = P $root @('cache')
  if ([IO.Directory]::Exists($cache)) {
    foreach ($d in @([IO.Directory]::EnumerateDirectories($cache))) {
      $ttl = if ($older) { [long]$older } else { $cacheTtl }
      Gc-Entry 'cache' $d $ttl $now $mode $repo
    }
  }
  if ($mode -eq 'apply' -and [IO.Directory]::Exists($seed)) {
    foreach ($r in @([IO.Directory]::EnumerateDirectories($seed))) {
      if (-not (@([IO.Directory]::EnumerateFileSystemEntries($r)).Count)) { try { [IO.Directory]::Delete($r) } catch { } }
    }
  }
  $minFree = if ($A.Length -gt 9) { $A[9] } else { '' }
  if ($minFree -match '^\d+$') {
    $maxDisk = if ($A.Length -gt 8 -and $A[8] -match '^\d+$') { [long]$A[8] } else { 0 }
    Evict-Budget $root $now $mode $repo $maxDisk ([long]$minFree) $(if ($A.Length -gt 10) { $A[10] } else { '' })
  }
}

# auto-gc ROOT NOW CACHE_TTL ORPHAN_TTL KEPT_TTL MODE REPO OLDER_THAN: the
# automatic gc a run starts after it ends. At most one runs per root (gc.lock),
# and it only ever removes files and never starts a goway run.
function Verb-auto-gc([string[]]$A) {
  $root = Get-Root $A[0]
  if (-not [IO.Directory]::Exists($root)) { return }
  $fs = Get-Lock (P $root @('gc.lock')) $true 0
  if (-not $fs) { return }
  try { Verb-gc $A } finally { $fs.Dispose() }
}

# purge ROOT: remove all of goway's state on this host (goway uninstall).
# Refuses an unmarked root and a root with a run in progress.
function Verb-purge([string[]]$A) {
  $root = Get-Root $A[0]
  if (-not [IO.Directory]::Exists($root)) { Write-Out "absent`n"; return }
  if (-not [IO.File]::Exists((P $root @('.goway-root')))) { Die "$root is not marked as goway state; not removing it" }
  $locks = @()
  $work = P $root @('work')
  if ([IO.Directory]::Exists($work)) {
    foreach ($d in [IO.Directory]::EnumerateDirectories($work)) {
      $l = [IO.Path]::Combine($d, 'lock'); if ([IO.File]::Exists($l)) { $locks += $l }
    }
  }
  $cache = P $root @('cache')
  if ([IO.Directory]::Exists($cache)) {
    foreach ($d in [IO.Directory]::EnumerateDirectories($cache)) {
      $locks += @([IO.Directory]::EnumerateFiles($d, 'target-*.lock'))
    }
  }
  foreach ($l in $locks) {
    # gc and the end of a run hold locks for moments; a real run for longer.
    $fs = Get-Lock $l $true 10000
    if (-not $fs) { Die 'a goway run is in progress on this host; try again when it ends' }
    $fs.Dispose()
  }
  # Only goway's own entries: a root that also holds foreign files keeps them.
  foreach ($n in @('work', 'seed', 'cache', 'gpu', 'gc.lock')) { Remove-Tree (P $root @($n)) }
  $marker = P $root @('.goway-root')
  if ([IO.File]::Exists($marker)) { [IO.File]::Delete($marker) }
  try { [IO.Directory]::Delete($root) } catch { }
  Write-Out "removed`n"
}

# ---- slot trees ------------------------------------------------------------
#
# A slot is a persistent build tree (cache\<repo>\tree-K) plus its cargo
# target dir (target-K), held by target-K.lock. Sync-Slot updates the tree in
# place from the run's snapshot; see remote.sh (sync_slot) for the design.

function New-WildcardSet {
  return New-Object 'System.Collections.Generic.List[System.Management.Automation.WildcardPattern]'
}

function New-Pattern([string]$Glob) {
  $opt = if ($script:IsWin) { [Management.Automation.WildcardOptions]::IgnoreCase } else { [Management.Automation.WildcardOptions]::None }
  return New-Object Management.Automation.WildcardPattern($Glob, $opt)
}

# The keep set: Names match an entry's name at any depth, Paths match its
# path relative to the tree root.
function Get-KeepSpec([string]$Farm, [string]$KeepB64) {
  $names = New-WildcardSet; $paths = New-WildcardSet
  $entries = New-Object System.Collections.Generic.HashSet[string]
  $detect = {
    param($dir, $name, $rel)
  }
  $stack = New-Object System.Collections.Generic.Stack[string]
  $stack.Push($Farm)
  $baseLen = $Farm.TrimEnd([IO.Path]::DirectorySeparatorChar, '/').Length + 1
  while ($stack.Count) {
    $d = $stack.Pop()
    foreach ($sub in [IO.Directory]::EnumerateDirectories($d)) {
      $n = [IO.Path]::GetFileName($sub)
      if ($n -ne 'node_modules' -and $n -ne '.git') { $stack.Push($sub) }
    }
    $relDir = if ($d.Length -ge $baseLen) { To-Rel $d $baseLen } else { '' }
    $pre = if ($relDir) { '/' + [Management.Automation.WildcardPattern]::Escape($relDir) + '/' } else { '/' }
    foreach ($f in [IO.Directory]::EnumerateFiles($d)) {
      $n = [IO.Path]::GetFileName($f)
      switch -Wildcard ($n) {
        'package.json' { [void]$entries.Add('node_modules'); [void]$entries.Add("$pre.next"); [void]$entries.Add("$pre.nuxt") }
        'pyproject.toml' { foreach ($x in @('__pycache__', '.venv', 'venv', '.tox', '.nox', '.pytest_cache', '.mypy_cache', '.ruff_cache')) { if ($x -eq '__pycache__') { [void]$entries.Add($x) } else { [void]$entries.Add("$pre$x") } } }
        'requirements*.txt' { foreach ($x in @('__pycache__', '.venv', 'venv', '.tox', '.nox', '.pytest_cache', '.mypy_cache', '.ruff_cache')) { if ($x -eq '__pycache__') { [void]$entries.Add($x) } else { [void]$entries.Add("$pre$x") } } }
        'CMakeLists.txt' { [void]$entries.Add("${pre}build"); [void]$entries.Add("${pre}cmake-build-*") }
        'pom.xml' { [void]$entries.Add("${pre}target") }
        'build.gradle' { [void]$entries.Add("${pre}.gradle"); [void]$entries.Add("${pre}build") }
        'build.gradle.kts' { [void]$entries.Add("${pre}.gradle"); [void]$entries.Add("${pre}build") }
      }
    }
  }
  if ($KeepB64) {
    $text = $script:Utf8.GetString([Convert]::FromBase64String($KeepB64))
    foreach ($e in $text.Split("`n")) {
      $e = $e.Trim()
      if (-not $e) { continue }
      if ($e.StartsWith('./')) { $e = $e.Substring(2) }
      if ($e.Contains('/')) { [void]$entries.Add('/' + $e) } else { [void]$entries.Add($e) }
    }
  }
  foreach ($e in $entries) {
    if ($e.StartsWith('/')) { $paths.Add((New-Pattern $e.Substring(1))) } else { $names.Add((New-Pattern $e)) }
  }
  return @{ Names = $names; Paths = $paths }
}

function Test-Kept($Spec, [string]$Name, [string]$Rel) {
  foreach ($p in $Spec.Names) { if ($p.IsMatch($Name)) { return $true } }
  foreach ($p in $Spec.Paths) { if ($p.IsMatch($Rel)) { return $true } }
  return $false
}

# The slot's files outside the keep set: path -> "size:ticks".
function Get-SlotFiles([string]$Slot, $Spec) {
  $out = New-Object 'System.Collections.Generic.Dictionary[string,string]'
  $baseLen = $Slot.TrimEnd([IO.Path]::DirectorySeparatorChar, '/').Length + 1
  $stack = New-Object System.Collections.Generic.Stack[string]
  $stack.Push($Slot)
  while ($stack.Count) {
    $d = $stack.Pop()
    $di = New-Object IO.DirectoryInfo $d
    foreach ($e in $di.EnumerateFileSystemInfos()) {
      $rel = To-Rel $e.FullName $baseLen
      if (Test-Kept $Spec $e.Name $rel) { continue }
      if ($e -is [IO.DirectoryInfo]) { $stack.Push($e.FullName) }
      else { $out[$rel] = '{0}:{1}' -f $e.Length, $e.LastWriteTimeUtc.Ticks }
    }
  }
  return $out
}

function Remove-EmptyUnkept([string]$Dir, [string]$Slot, $Spec, [int]$BaseLen) {
  foreach ($d in @([IO.Directory]::EnumerateDirectories($Dir))) {
    $rel = To-Rel $d $BaseLen
    if (Test-Kept $Spec ([IO.Path]::GetFileName($d)) $rel) { continue }
    Remove-EmptyUnkept $d $Slot $Spec $BaseLen
    if (-not (@([IO.Directory]::EnumerateFileSystemEntries($d)).Count)) { try { [IO.Directory]::Delete($d) } catch { } }
  }
}

function Read-Recs([string]$File) {
  $d = New-Object 'System.Collections.Generic.Dictionary[string,string]'
  if ([IO.File]::Exists($File)) {
    foreach ($r in (Split-Nul ([IO.File]::ReadAllBytes($File)))) {
      $i = $r.IndexOf([char]1)
      if ($i -gt 0) { $d[$r.Substring($i + 1)] = $r.Substring(0, $i) }
    }
  }
  return ,$d
}

function Write-Recs([string]$File, $Dict) {
  $ms = New-Object IO.MemoryStream
  foreach ($k in $Dict.Keys) {
    $b = $script:Utf8.GetBytes($Dict[$k] + [string][char]1 + $k)
    $ms.Write($b, 0, $b.Length); $ms.WriteByte(0)
  }
  [IO.File]::WriteAllBytes($File, $ms.ToArray())
}

function Files-Equal([string]$A, [string]$B) {
  $fa = New-Object IO.FileInfo $A; $fb = New-Object IO.FileInfo $B
  if (-not $fa.Exists -or -not $fb.Exists -or $fa.Length -ne $fb.Length) { return $false }
  return (Sha256-Hex $A) -eq (Sha256-Hex $B)
}

# Paths the .gitignore rules of the snapshot ignore, of $Paths.
function Get-GitIgnored([string]$Farm, [string[]]$Paths, [string]$Scratch) {
  $git = Resolve-Program 'git'
  if (-not $git -or $Paths.Count -eq 0) { return @() }
  $gitdir = [IO.Path]::Combine($Scratch, 'git')
  $null = Invoke-Native $git @('init', '-q', '--bare', $gitdir) $null
  $null_ = if ($script:IsWin) { 'NUL' } else { '/dev/null' }
  $out = New-Object System.Collections.Generic.List[string]
  # The paths go as arguments, in batches under the command line limit.
  $i = 0
  while ($i -lt $Paths.Count) {
    $batch = New-Object System.Collections.Generic.List[string]
    $len = 0
    while ($i -lt $Paths.Count -and ($batch.Count -eq 0 -or $len + $Paths[$i].Length + 3 -lt 16000)) {
      $batch.Add($Paths[$i]); $len += $Paths[$i].Length + 3; $i++
    }
    $words = @('-c', "core.excludesFile=$null_", '-c', 'core.quotepath=false', 'check-ignore', '--no-index', '--') + $batch.ToArray()
    $r = Invoke-Native $git $words @{ GIT_DIR = $gitdir; GIT_WORK_TREE = $Farm }
    foreach ($line in $script:Utf8.GetString($r[1]).Split("`n")) {
      $line = $line.TrimEnd("`r")
      if ($line) { $out.Add($line) }
    }
  }
  return [string[]]$out.ToArray()
}

# Update $Slot in place so it holds exactly the files of $Farm (the run's
# snapshot) plus what the keep set preserves. Returns @(written, removed).
function Sync-Slot([string]$Farm, [string]$Slot, [string]$Work, [bool]$KeepIgnored, [string]$KeepB64, [string]$SeedKey, [string]$Target) {
  New-Dir $Slot
  $scratch = P $Work @('reconcile')
  Remove-Tree $scratch; New-Dir $scratch
  $spec = Get-KeepSpec $Farm $KeepB64
  $snap = New-Object 'System.Collections.Generic.Dictionary[string,string]'
  foreach ($f in (Get-Files $Farm)) { $snap[$f[0]] = '{0}:{1}' -f $f[1], $f[2] }
  $cur = Get-SlotFiles $Slot $spec
  $farmLast = Read-Recs "$Slot.farm"
  $slotLast = Read-Recs "$Slot.slot"
  $cand = New-Object 'System.Collections.Generic.HashSet[string]'
  # A: new or changed in the snapshot since the last reconcile
  foreach ($p in $snap.Keys) { $v = $null; if (-not $farmLast.TryGetValue($p, [ref]$v) -or $v -ne $snap[$p]) { [void]$cand.Add($p) } }
  # B: changed or removed in the slot since the last reconcile (a job wrote there)
  foreach ($p in $slotLast.Keys) {
    if (-not $snap.ContainsKey($p)) { continue }
    $v = $null
    if (-not $cur.TryGetValue($p, [ref]$v) -or $v -ne $slotLast[$p]) { [void]$cand.Add($p) }
  }
  # C: missing from the slot listing (new, or under a kept dir)
  foreach ($p in $snap.Keys) { if (-not $cur.ContainsKey($p)) { [void]$cand.Add($p) } }
  # D: files the seed's change log names since the slot's last reconcile; all
  #    files when the last reconcile was another seed's
  $full = $true
  $stateFile = "$Slot.state"
  $have = (Read-TextOrEmpty $stateFile).Trim().Split(' ')
  $curInfo = (Read-TextOrEmpty (P $Work @('seqinfo'))).Trim().Split(' ')
  if ($SeedKey -and $have.Length -ge 3 -and $curInfo.Length -ge 2 -and $have[0] -eq $SeedKey -and $have[1] -eq $curInfo[0] -and $have[2] -match '^\d+$' -and $curInfo[1] -match '^\d+$') {
    $full = $false
    for ($n = [int]$have[2] + 1; $n -le [int]$curInfo[1]; $n++) {
      $cf = P $Work @('changes', [string]$n)
      if ([IO.File]::Exists($cf)) { foreach ($p in (Split-Nul ([IO.File]::ReadAllBytes($cf)))) { [void]$cand.Add($p) } }
      else { $full = $true; break }
    }
  }
  if ($full) { foreach ($p in $snap.Keys) { [void]$cand.Add($p) } }
  # A candidate already holding the snapshot's content is not written.
  $todo = New-Object System.Collections.Generic.List[string]
  foreach ($p in ($cand | Sort-Object)) {
    if (-not $snap.ContainsKey($p)) { continue }
    $sp = [IO.Path]::Combine($Slot, (Native-Path $p))
    $fp = [IO.Path]::Combine($Farm, (Native-Path $p))
    if ([IO.File]::Exists($sp) -and (Files-Equal $sp $fp)) { continue }
    $todo.Add($p)
  }
  # Slot paths the snapshot does not have at all, minus the ignored ones.
  $stale = New-Object System.Collections.Generic.List[string]
  foreach ($p in $cur.Keys) { if (-not $snap.ContainsKey($p)) { $stale.Add($p) } }
  if ($KeepIgnored -and $stale.Count) {
    $ignored = New-Object 'System.Collections.Generic.HashSet[string]'
    foreach ($p in (Get-GitIgnored $Farm $stale.ToArray() $scratch)) { [void]$ignored.Add($p) }
    $kept = New-Object System.Collections.Generic.List[string]
    foreach ($p in $stale) { if (-not $ignored.Contains($p)) { $kept.Add($p) } }
    $stale = $kept
  }
  foreach ($p in $stale) {
    try { Remove-Tree ([IO.Path]::Combine($Slot, (Native-Path $p))) } catch { }
    [void]$cur.Remove($p)
  }
  Remove-EmptyUnkept $Slot $Slot $spec ($Slot.TrimEnd([IO.Path]::DirectorySeparatorChar, '/').Length + 1)
  if ($todo.Count) {
    # Written files get the current time as mtime, as git checkout does: make,
    # ninja and cargo rebuild only when a source is newer than its output. If
    # anything in the slot or its target dir is dated in the future, they are
    # stamped one second after the newest such file instead.
    $now = [DateTime]::UtcNow
    $newest = $null
    foreach ($root in @($Slot, $Target)) {
      if (-not $root -or -not [IO.Directory]::Exists($root)) { continue }
      $di = New-Object IO.DirectoryInfo $root
      foreach ($f in $di.EnumerateFiles('*', [IO.SearchOption]::AllDirectories)) {
        $t = $f.LastWriteTimeUtc
        if ($t -gt $now -and (-not $newest -or $t -gt $newest)) { $newest = $t }
      }
    }
    $stamp = $now
    if ($newest) { $stamp = $newest.AddSeconds(1) }
    $execs = New-Object System.Collections.Generic.List[string]
    $execSet = Load-Exec (P $Work @('modes'))
    foreach ($p in $todo) {
      $sp = [IO.Path]::Combine($Slot, (Native-Path $p))
      $fp = [IO.Path]::Combine($Farm, (Native-Path $p))
      $walk = [IO.Path]::GetDirectoryName($sp)
      while ($walk.Length -gt $Slot.Length) {
        if ([IO.File]::Exists($walk)) { Remove-Tree $walk }
        $walk = [IO.Path]::GetDirectoryName($walk)
      }
      New-Dir ([IO.Path]::GetDirectoryName($sp))
      Remove-Tree $sp
      [IO.File]::Copy($fp, $sp, $false)
      [IO.File]::SetAttributes($sp, [IO.FileAttributes]::Normal)
      [IO.File]::SetLastWriteTimeUtc($sp, $stamp)
      $fi = New-Object IO.FileInfo $sp
      $cur[$p] = '{0}:{1}' -f $fi.Length, $fi.LastWriteTimeUtc.Ticks
      if ($execSet.Contains($p)) { $execs.Add($sp) }
    }
    if (-not $script:IsWin -and $execs.Count) {
      for ($i = 0; $i -lt $execs.Count; $i += 200) {
        $chunk = $execs.GetRange($i, [math]::Min(200, $execs.Count - $i)).ToArray()
        & chmod u+x -- @chunk
      }
    }
  }
  # What the copy-integrity check looks at: what this sync wrote, and all of
  # the snapshot's files.
  $script:Written = @($todo)
  $script:AllReg = @($snap.Keys | Sort-Object)
  # What the next reconcile compares against.
  Write-Recs "$Slot.farm" $snap
  Write-Recs "$Slot.slot" $cur
  Write-Text $stateFile ("{0} {1}" -f $SeedKey, (Read-TextOrEmpty (P $Work @('seqinfo'))).Trim())
  Write-Text "$Slot.stats" ("written={0} removed={1}`n" -f $todo.Count, $stale.Count)
  Remove-Tree $scratch
  return @($todo.Count, $stale.Count)
}

function Copy-Dir([string]$Src, [string]$Dst) {
  New-Dir $Dst
  foreach ($f in [IO.Directory]::EnumerateFiles($Src)) { [IO.File]::Copy($f, [IO.Path]::Combine($Dst, [IO.Path]::GetFileName($f)), $true) }
  foreach ($d in [IO.Directory]::EnumerateDirectories($Src)) { Copy-Dir $d ([IO.Path]::Combine($Dst, [IO.Path]::GetFileName($d))) }
}

# Throw away a slot's tree, state and cargo target dir; with $SeedKey also
# the seed this run was given (marked so it is rebuilt from the laptop, never
# from a sibling seed), so the next attempt copies everything again.
function Wipe-Slot([int]$Slot, [string]$Cache, [string]$SeedKey, [string]$Root) {
  Remove-Tree (P $Cache @("tree-$Slot"))
  Remove-Tree (P $Cache @("target-$Slot"))
  foreach ($x in @('farm', 'slot', 'state', 'stats')) {
    $f = P $Cache @("tree-$Slot.$x")
    if ([IO.File]::Exists($f)) { [IO.File]::Delete($f) }
  }
  if (-not $SeedKey -or $SeedKey -notmatch '^[A-Za-z0-9._/-]+$' -or $SeedKey.Contains('..')) { return }
  $seed = P $Root @('seed', $SeedKey)
  if ([IO.Directory]::Exists($seed)) {
    foreach ($n in @('tree', 'changes')) { Remove-Tree (P $seed @($n)) }
    foreach ($n in @('generation')) { $f = P $seed @($n); if ([IO.File]::Exists($f)) { [IO.File]::Delete($f) } }
    [IO.File]::WriteAllBytes((P $seed @('fresh')), @())
  }
}

# ---- copy integrity -------------------------------------------------------

# "f SOH sha256 SOH path" claims about the named files of $Dir. GOWAY_TEST_CORRUPT
# (tests only) falsifies the first hash: 1 always, first only attempt 1's,
# after only the post-failure check of attempt 1.
function Get-Claims([string]$Dir, [string[]]$Paths, [int]$Attempt, [int]$Phase) {
  $corrupt = $false
  switch ($env:GOWAY_TEST_CORRUPT) {
    '1' { $corrupt = $true }
    'first' { $corrupt = ($Attempt -eq 1) }
    'after' { $corrupt = ($Attempt -eq 1 -and $Phase -eq 2) }
  }
  $ms = New-Object IO.MemoryStream
  $first = $true
  foreach ($p in ($Paths | Sort-Object)) {
    $full = [IO.Path]::Combine($Dir, (Native-Path $p))
    if (-not [IO.File]::Exists($full)) { continue }
    $h = Sha256-Hex $full
    if ($corrupt -and $first) { $h = '0' * 64 }
    $first = $false
    $b = $script:Utf8.GetBytes("f$([char]1)$h$([char]1)$p")
    $ms.Write($b, 0, $b.Length); $ms.WriteByte(0)
  }
  return ,$ms.ToArray()
}

# Publish the claims about $Paths (verify.PHASE) and block until goway
# answers with a verdict (verdict.PHASE: ok or bad). True for ok; false for
# bad, or when no answer comes (goway went away or never answered).
function Verify-Gate([string]$Work, [int]$Phase, [string]$Dir, [string[]]$Paths, [int]$Attempt) {
  $claims = Get-Claims $Dir $Paths $Attempt $Phase
  $head = $script:Utf8.GetBytes('goway-verify1'); 
  $ms = New-Object IO.MemoryStream
  $ms.Write($head, 0, $head.Length); $ms.WriteByte(0); $ms.Write($claims, 0, $claims.Length)
  $tmp = P $Work @("verify.$Phase.tmp")
  [IO.File]::WriteAllBytes($tmp, $ms.ToArray())
  [IO.File]::Move($tmp, (P $Work @("verify.$Phase")))
  $verdict = P $Work @("verdict.$Phase")
  for ($i = 0; $i -lt 4800; $i++) {
    if ([IO.File]::Exists($verdict)) {
      return ((Read-TextOrEmpty $verdict).Trim() -eq 'ok')
    }
    if (($i % 8) -eq 7 -and -not (Test-ParentAlive)) { return $false }
    Start-Sleep -Milliseconds 25
  }
  return $false
}

# verify-wait ROOT RUN_ID PHASE [SECONDS]: for goway's control call. Prints
# "ready" and the claims once the run published them, "ended" once the run is
# over, or "pending" after SECONDS (default 10, at most 55: ask again).
function Verb-verify-wait([string[]]$A) {
  $root = Get-Root $A[0]; $work = P $root @('work', $A[1])
  if (("$($A[1])$($A[2])") -notmatch '^[A-Za-z0-9-]+$') { Die 'verify-wait: bad argument' }
  $window = 10
  if ($A.Length -gt 3) {
    if ($A[3] -notmatch '^\d+$') { Die 'verify-wait: bad window' }
    $window = [math]::Min([int]$A[3], 55)
  }
  $seen = 0
  for ($i = 0; $i -lt $window * 40; $i++) {
    $v = P $work @("verify.$($A[2])")
    if ([IO.File]::Exists($v)) {
      Write-Out "ready`n"
      Write-OutBytes ([IO.File]::ReadAllBytes($v))
      return
    }
    $lock = P $work @('lock')
    if ([IO.File]::Exists($lock)) {
      $fs = Get-Lock $lock $true 0
      if ($fs) {
        $fs.Dispose(); $seen++
        if ($seen -ge 12) { Write-Out "ended`n"; return }
      } else { $seen = 0 }
    } elseif (-not [IO.Directory]::Exists($work)) {
      Write-Out "ended`n"; return
    }
    Start-Sleep -Milliseconds 25
  }
  Write-Out "pending`n"
}

# verify-verdict ROOT RUN_ID PHASE ok|bad: goway's answer to Verify-Gate.
function Verb-verify-verdict([string[]]$A) {
  $root = Get-Root $A[0]; $work = P $root @('work', $A[1])
  if (("$($A[1])$($A[2])") -notmatch '^[A-Za-z0-9-]+$') { Die 'verify-verdict: bad argument' }
  if ($A[3] -ne 'ok' -and $A[3] -ne 'bad') { Die 'verify-verdict: bad verdict' }
  if (-not [IO.Directory]::Exists($work)) { Die 'verify-verdict: no work dir' }
  $tmp = P $work @("verdict.$($A[2]).tmp")
  [IO.File]::WriteAllText($tmp, $A[3])
  [IO.File]::Move($tmp, (P $work @("verdict.$($A[2])")))
}

# ---- jobs ------------------------------------------------------------------

$script:ParentPid = $null
$script:ParentStart = $null

function Get-ParentPid {
  if ($script:Ver -ge 7) {
    try { return [int](Get-Process -Id $PID).Parent.Id } catch { }
  }
  if ($script:IsWin) {
    try { Load-Native; $pp = [GowayNative]::ParentPid(); if ($pp -gt 0) { return $pp } } catch { }
    try { return [int](Get-CimInstance Win32_Process -Filter "ProcessId=$PID").ParentProcessId } catch { return 0 }
  }
  $ps = Resolve-Program 'ps'
  if ($ps) {
    $r = Invoke-Native $ps @('-o', 'ppid=', '-p', "$PID") $null
    $t = $script:Utf8.GetString($r[1]).Trim()
    if ($t -match '^\d+$') { return [int]$t }
  }
  return 0
}

# Whether the process that started this script (the ssh session's shell) is
# still there. When it is gone the connection dropped and the job must stop.
function Test-ParentAlive {
  if ($null -eq $script:ParentPid) {
    $script:ParentPid = Get-ParentPid
    $script:ParentStart = $null
    if ($script:ParentPid -gt 0) {
      try { $script:ParentStart = (Get-Process -Id $script:ParentPid).StartTime } catch { }
    }
  }
  if ($script:ParentPid -le 0) { return $true }
  $p = Get-Process -Id $script:ParentPid -ErrorAction SilentlyContinue
  if (-not $p) { return $false }
  try { if ($script:ParentStart -and $p.StartTime -ne $script:ParentStart) { return $false } } catch { }
  return $true
}

# Stop a job's whole process tree.
function Stop-Tree([Diagnostics.Process]$P) {
  if ($script:IsWin) {
    & taskkill /PID $P.Id /T /F 2>&1 | Out-Null
  } else {
    & kill -TERM -- "-$($P.Id)" 2>$null
    if (-not $P.WaitForExit(5000)) { & kill -KILL -- "-$($P.Id)" 2>$null }
  }
}

# Start the job as run does: own process tree, standard handles inherited
# (live, byte-exact output), a polite priority. With $RedirectErr its
# standard error is a pipe for the caller to copy.
function Start-JobProcess([string[]]$Words, [string]$Cwd, [string]$Priority, [string]$PidFile, [bool]$RedirectErr) {
  $psi = New-Object Diagnostics.ProcessStartInfo
  $psi.UseShellExecute = $false
  $psi.WorkingDirectory = $Cwd
  $psi.RedirectStandardError = $RedirectErr
  if ($script:IsWin) {
    $exe = Resolve-Program $Words[0]
    if (-not $exe) {
      Write-Err "goway-remote: run: $($Words[0]): command not found`n"
      return $null
    }
    $psi.FileName = $exe
    $psi.Arguments = Join-WinArgs ([string[]]($Words | Select-Object -Skip 1))
    $p = [Diagnostics.Process]::Start($psi)
    Load-Native
    [void][GowayNative+JobTree]::Attach($p)
    [IO.File]::WriteAllText($PidFile, [string]$p.Id)
    if ($Priority -eq 'low') { try { $p.PriorityClass = [Diagnostics.ProcessPriorityClass]::BelowNormal } catch { } }
    return $p
  }
  $pre = @()
  if ($Priority -eq 'low' -and (Resolve-Program 'nice')) { $pre = @('nice', '-n', '10') }
  $inner = @('sh', '-c', 'echo $$ >"$0"; exec "$@"', $PidFile) + $pre + $Words
  $setsid = Resolve-Program 'setsid'
  if ($setsid) { $psi.FileName = $setsid; $psi.Arguments = Join-WinArgs $inner }
  else { $psi.FileName = (Resolve-Program 'sh'); $psi.Arguments = Join-WinArgs ($inner | Select-Object -Skip 1) }
  return [Diagnostics.Process]::Start($psi)
}

# Wait for the job; stop it when the session goes away. Returns its exit
# code (143 when it was stopped for that).
function Wait-Job([Diagnostics.Process]$P) {
  while (-not $P.WaitForExit(500)) {
    if (-not (Test-ParentAlive)) {
      Stop-Tree $P
      [void]$P.WaitForExit(5000)
      return 143
    }
  }
  $rc = $P.ExitCode
  if ($rc -lt 0 -or $rc -gt 255) { $rc = 128 + ($rc -band 0x7f) }
  return $rc
}

function Run-Job([string[]]$Words, [string]$Cwd, [string]$Priority, [string]$PidFile) {
  $p = Start-JobProcess $Words $Cwd $Priority $PidFile $false
  if (-not $p) { return 127 }
  return (Wait-Job $p)
}

# ---- GPU slots ---------------------------------------------------------------

function Get-GpuList {
  $out = @()
  $exe = Resolve-Program 'nvidia-smi'
  if ($exe) {
    foreach ($i in (GPU-Query 'index')) { if ($i -match '^\d+$') { $out += "nvidia $i" } }
  }
  return $out
}

# Take one free GPU slot (waiting, with a note, while all are busy), then
# export CUDA_VISIBLE_DEVICES for it unless the user already set it.
function Acquire-Gpu([string]$Root, [int]$Per) {
  $dir = P $Root @('gpu')
  $gpus = @(Get-GpuList)
  if ($gpus.Count -eq 0) {
    Write-Err "goway: warning: this run needs a GPU but no GPU tool lists one here; running without a GPU slot`n"
    return
  }
  New-Dir $dir
  $noted = $false
  while ($true) {
    for ($k = 0; $k -lt $Per; $k++) {
      foreach ($line in $gpus) {
        $vendor, $idx = $line.Split(' ')
        $f = [IO.Path]::Combine($dir, "$vendor-$idx.$k.lock")
        $fs = Get-Lock $f $true 0
        if ($fs) {
          $script:Held['gpu'] = $fs
          if ($null -eq [Environment]::GetEnvironmentVariable('CUDA_VISIBLE_DEVICES')) {
            [Environment]::SetEnvironmentVariable('CUDA_VISIBLE_DEVICES', $idx)
          }
          Write-Err "goway: using GPU $idx ($vendor), slot $k`n"
          return
        }
      }
    }
    if (-not $noted) {
      Write-Err "goway: all $($gpus.Count) GPU(s) are busy (up to $Per run(s) each); waiting for one`n"
      $noted = $true
    }
    Start-Sleep -Seconds 1
  }
}

# ---- shard framework detection (goway run --shard) ---------------------------
#
# The program a shard's command names may be a GoogleTest or Catch2 v3 test
# binary. The file is only ever READ, never executed: it must be a regular ELF
# or PE executable (magic bytes) and contain EVERY marker string of a
# framework. A binary with both frameworks' markers counts as neither.

$script:SniffCap = 256MB
$script:GtestMarkers = @('GTEST_SHARD_INDEX', 'GTEST_TOTAL_SHARDS', '--gtest_list_tests', '--gtest_filter')
$script:Catch2Markers = @('--shard-count', '--shard-index', '--list-tests', 'catch2-version')

function Sniff-Binary([string]$File) {
  if (-not $File -or -not [IO.File]::Exists($File)) { return 'none' }
  try {
    $fs = [IO.File]::Open($File, 'Open', 'Read', 'ReadWrite, Delete')
    try { $magic = New-Object byte[] 4; $n = $fs.Read($magic, 0, 4) } finally { $fs.Dispose() }
  } catch { return 'none' }
  $isElf = ($n -ge 4 -and $magic[0] -eq 0x7f -and $magic[1] -eq 0x45 -and $magic[2] -eq 0x4c -and $magic[3] -eq 0x46)
  $isPe = ($n -ge 2 -and $magic[0] -eq 0x4d -and $magic[1] -eq 0x5a)
  if (-not ($isElf -or $isPe)) { return 'none' }
  Load-Native
  $hits = [GowayNative]::FindMarkers($File, [string[]]($script:GtestMarkers + $script:Catch2Markers), $script:SniffCap)
  $g = $true; foreach ($m in $script:GtestMarkers) { if ($hits -notcontains $m) { $g = $false } }
  $c = $true; foreach ($m in $script:Catch2Markers) { if ($hits -notcontains $m) { $c = $false } }
  if ($g -and -not $c) { return 'gtest' }
  if ($c -and -not $g) { return 'catch2' }
  return 'none'
}

function Test-Catch2Rejected([string]$Err, [int]$Rc) {
  if ($Rc -eq 0) { return $false }
  $first = ($Err -split "`n" | Where-Object { $_.Trim() } | Select-Object -First 1)
  if (-not $first -or $first.Trim() -ne 'Error(s) in input:') { return $false }
  return ($Err -match 'Unrecognised token: --shard-(count|index)')
}

# Run CMD once. Attempt 1 adds Catch2's shard flags, attempt 2 (the one
# rerun) adds none. Stderr still reaches the caller live; a copy of its first
# 64 KiB goes to the returned text. Returns @(exit code, rejected?).
function Catch2-Attempt([int]$Attempt, [int]$Idx, [int]$Cnt, [string[]]$Words, [string]$Cwd, [string]$Priority, [string]$PidFile) {
  $w = $Words
  if ($Attempt -eq 1) { $w = $Words + @('--shard-count', "$Cnt", '--shard-index', [string]($Idx - 1)) }
  Load-Native
  $p = Start-JobProcess $w $Cwd $Priority $PidFile $true
  if (-not $p) { return @(127, $false, '') }
  $tee = New-Object 'GowayNative+Tee' $p.StandardError.BaseStream, $script:Stderr, 65536
  $tee.Start()
  $rc = Wait-Job $p
  $tee.Join()
  $err = $script:Utf8.GetString($tee.Snapshot())
  return @($rc, (Test-Catch2Rejected $err $rc), $err)
}

# Run a shard whose framework goway may detect. $Spec is "index:count:nonce".
# Prints goway's notes on stderr and, last, one result line the client strips
# from the stream and records. The rerun after a certain Catch2 rejection
# happens at most once, by construction: attempt 1 and 2 are two explicit calls.
function Shard-Run([string]$Spec, [string]$Work, [string[]]$Words, [string]$Cwd, [string]$Priority, [string]$PidFile) {
  $idx, $cnt, $nonce = $Spec.Split(':')
  $idx = [int]$idx; $cnt = [int]$cnt
  $prog = $Words[0]
  $file = Resolve-Program $prog
  $kind = Sniff-Binary $file
  foreach ($a in $Words) {
    if ($a -in @('--shard-count', '--shard-index') -or $a -like '--shard-count=*' -or $a -like '--shard-index=*') { $kind = 'none' }
  }
  if ($kind -eq 'gtest' -and ($env:GTEST_TOTAL_SHARDS -or $env:GTEST_SHARD_INDEX)) { $kind = 'none' }
  $rc = 0; $attempts = ''; $rerun = 0; $rejected = 0; $flagged = 0; $dup = 0
  $status = P $Work @('gtest-shard-status')
  switch ($kind) {
    'gtest' {
      Write-Err "goway: shard ${idx}/${cnt}: $prog is a GoogleTest binary (detected by reading it); sharding with GTEST_TOTAL_SHARDS and GTEST_SHARD_INDEX`n"
      if ([IO.File]::Exists($status)) { [IO.File]::Delete($status) }
      [Environment]::SetEnvironmentVariable('GTEST_TOTAL_SHARDS', "$cnt")
      [Environment]::SetEnvironmentVariable('GTEST_SHARD_INDEX', [string]($idx - 1))
      [Environment]::SetEnvironmentVariable('GTEST_SHARD_STATUS_FILE', $status)
      $rc = Run-Job $Words $Cwd $Priority $PidFile
      $attempts = "$rc"
      $judged = $true
      foreach ($a in $Words) { if ($a -in @('--gtest_list_tests', '--help', '-h', '--gtest_help')) { $judged = $false } }
      if ($judged -and $cnt -gt 1 -and -not [IO.File]::Exists($status)) {
        $dup = 1
        Write-Err "goway: warning: ${prog}: GoogleTest did not apply sharding (no status file), so this shard ran the whole suite; results stand but work was duplicated`n"
      }
    }
    'catch2' {
      Write-Err "goway: shard ${idx}/${cnt}: $prog is a Catch2 v3 binary (detected by reading it); sharding with --shard-count and --shard-index`n"
      $r = Catch2-Attempt 1 $idx $cnt $Words $Cwd $Priority $PidFile
      $rc = $r[0]; $attempts = "$rc"
      if ($r[1]) {
        Write-Err "goway: warning: $prog rejected the shard flags before running any test; rerunning this shard once without them (it runs the whole suite)`n"
        $rerun = 1
        $r2 = Catch2-Attempt 2 $idx $cnt $Words $Cwd $Priority $PidFile
        $rc = $r2[0]; $attempts = "$attempts,$rc"
        if ($r2[1]) {
          $rejected = 1
          Write-Err "goway: warning: $prog was rejected again after the rerun; not trying again`n"
        }
      } elseif ($rc -ne 0 -and ($r[2] -match 'Unrecognised token|Error\(s\) in input')) {
        $flagged = 1
        Write-Err "goway: warning: $prog failed with a command-line error that may be about the shard flags; not certain, so it is not rerun`n"
      }
    }
    default {
      $rc = Run-Job $Words $Cwd $Priority $PidFile
      $attempts = "$rc"
    }
  }
  Write-Err ("{0}goway-shard-result:{1} detected={2} attempts={3} rerun={4} rejected={5} flagged={6} duplicated={7}`n" -f [char]1, $nonce, $kind, $attempts, $rerun, $rejected, $flagged, $dup)
  return $rc
}

# ---- run ---------------------------------------------------------------------

function Get-Stamps([string]$Tree) {
  $d = New-Object 'System.Collections.Generic.Dictionary[string,string]'
  foreach ($f in (Get-Files $Tree)) { $d[$f[0]] = '{0}:{1}' -f $f[1], $f[2] }
  return ,$d
}

# Whether any work, seed or cache entry is old enough (by the shortest TTL)
# for gc to want it; a run starts the detached gc only then, instead of a
# PowerShell per run that finds nothing.
function Test-GcDue([string]$Root, [string]$Ttls) {
  $t = $Ttls.Split(':')
  if ($t.Length -lt 3) { return $false }
  $min = [long]([Linq.Enumerable]::Min([long[]]@([long]$t[0], [long]$t[1], [long]$t[2])))
  $now = Unix-Secs
  $dirs = New-Object System.Collections.Generic.List[string]
  foreach ($kind in @('work', 'cache')) {
    $d = P (Get-Root $Root) @($kind)
    if ([IO.Directory]::Exists($d)) { foreach ($e in [IO.Directory]::EnumerateDirectories($d)) { $dirs.Add($e) } }
  }
  $seed = P (Get-Root $Root) @('seed')
  if ([IO.Directory]::Exists($seed)) {
    foreach ($r in [IO.Directory]::EnumerateDirectories($seed)) { foreach ($e in [IO.Directory]::EnumerateDirectories($r)) { $dirs.Add($e) } }
  }
  foreach ($e in $dirs) { if ((Age-Of $e $now) -ge $min) { return $true } }
  return $false
}

# A cheap automatic gc of expired entries, detached: it never delays the
# exit and never holds the ssh session's pipes open.
function Start-AutoGc([string]$RootArg, [string]$Ttls) {
  $t = $Ttls.Split(':')
  if ($t.Length -lt 3 -or -not $script:SelfPath) { return }
  # TTLS is "cache:orphan:kept[:max_disk:min_free:cache_size]"; with the budget fields the
  # detached gc also evicts to the disk budget and leaves a note for the next run.
  $budget = if ($t.Length -ge 5) { @($t[3], $t[4], 'log') } else { @() }
  $source = ps-call-source (@($script:SelfPath, 'auto-gc', $RootArg, [string](Unix-Secs), $t[0], $t[1], $t[2], 'apply', '', '') + $budget)
  $enc = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($source))
  $exe = (Get-Process -Id $PID).Path
  $scratch = [IO.Path]::GetTempPath()
  $o = [IO.Path]::Combine($scratch, 'goway-gc.out'); $e = [IO.Path]::Combine($scratch, 'goway-gc.err')
  try {
    $sp = @{ FilePath = $exe; ArgumentList = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand $enc"; RedirectStandardOutput = $o; RedirectStandardError = $e }
    if ($script:IsWin) { $sp['WindowStyle'] = 'Hidden' }
    Start-Process @sp | Out-Null
  } catch { Write-Err "goway-remote: cannot start the background gc: $($_.Exception.Message)`n" }
}

function ps-call-source([string[]]$Words) {
  return '& ' + (($Words | ForEach-Object { "'" + $_.Replace("'", "''") + "'" }) -join ' ')
}

# Git for Windows' usr\bin directory (it holds sh.exe and the Unix tools
# tests spawn), or $null: the install root is found from git.exe's location,
# then the standard install paths; only an existing directory counts.
function Find-GitUsrBin {
  $roots = New-Object System.Collections.Generic.List[string]
  $git = Resolve-Program 'git'
  if ($git) {
    $d = [IO.Path]::GetDirectoryName($git)
    for ($n = 0; $d -and $n -lt 3; $n++) { $roots.Add($d); $d = [IO.Path]::GetDirectoryName($d) }
  }
  foreach ($v in @($env:ProgramFiles, ${env:ProgramFiles(x86)}, $env:ProgramW6432)) {
    if ($v) { $roots.Add([IO.Path]::Combine($v, 'Git')) }
  }
  if ($env:LOCALAPPDATA) { $roots.Add([IO.Path]::Combine($env:LOCALAPPDATA, 'Programs', 'Git')) }
  foreach ($r in $roots) {
    $u = [IO.Path]::Combine($r, 'usr', 'bin')
    if ([IO.File]::Exists([IO.Path]::Combine($u, 'sh.exe'))) { return $u }
  }
  return $null
}

# Where sh is missing, append Git for Windows' usr\bin to PATH (never
# prepend: the host's own tools keep winning) and say so on stderr.
function Add-GitUsrBin {
  if (Resolve-Program 'sh') { return }
  $u = Find-GitUsrBin
  if (-not $u) { return }
  $env:PATH = $env:PATH + [IO.Path]::PathSeparator + $u
  Write-Err "goway: sh was not on PATH; appended $u (Git for Windows)`n"
}

# Slots-Busy-Note CACHE SLOTS: how long the oldest holder of a build slot has run (a slot
# lock's mtime is stamped when a run takes it), as text such as `oldest holder has run 12m 3s`.
function Slots-Busy-Note([string]$Cache, [int]$Slots) {
  $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
  $oldest = 0
  for ($k = 0; $k -lt $Slots; $k++) {
    $l = P $Cache @("target-$k.lock")
    if (-not [IO.File]::Exists($l)) { continue }
    $age = $now - [DateTimeOffset]([IO.File]::GetLastWriteTimeUtc($l)).ToUnixTimeSeconds()
    if ($age -gt $oldest) { $oldest = $age }
  }
  return ('oldest holder has run {0}m {1}s' -f [math]::Floor($oldest / 60), ($oldest % 60))
}

# run ROOT RUN_ID REPO_ID KEEP SLOTS CACHE_META_B64 TTLS PRIORITY KEEP_IGNORED
#     KEEP_B64 [WORD...] -- CMD...
# The work dir was created by receive (a hard-link snapshot of the seed).
# Take a free slot (preferring the one this worktree used last), update the
# slot's persistent tree in place from the snapshot, run CMD there with stdio
# passed through, clean up, and exit with CMD's status. With KEEP=1 the
# finished tree is copied to work\<run-id>\tree for inspection. CMD is
# `@args` to read the words from the work dir's args file (argsfile verb).
function Verb-run([string[]]$A) {
  if ($A.Length -lt 11) { Die 'run: too few arguments' }
  $rootArg = $A[0]
  $root = Get-Root $A[0]; $runId = $A[1]; $repoId = $A[2]; $keep = $A[3]
  $slots = [int]$A[4]; $cacheMeta = $A[5]; $ttls = $A[6]; $priority = $A[7]
  $keepIgnored = ($A[8] -eq '1'); $keepB64 = $A[9]
  Test-Id $runId 'run'
  $work = P $root @('work', $runId); $cache = P $root @('cache', $repoId)
  $detect = ''; $gpuPer = 0; $slotWait = 300; $verify = $false; $fresh = $false; $attempt = 1; $level = 'changed'
  $i = 10
  while ($i -lt $A.Length -and $A[$i] -ne '--') {
    $w = $A[$i]
    if ($w -match '^shard-detect:\d+:\d+:[A-Za-z0-9]+$') { $detect = $w.Substring(13) }
    elseif ($w -match '^gpu-slots:(\d+)$') { $gpuPer = [int]$Matches[1] }
    elseif ($w -match '^slot-wait:(\d+)$') { $slotWait = [int]$Matches[1] }
    elseif ($w -match '^limits:(\d+|auto):\d*:\d*$') { }  # per-job caps: systemd scopes only, nothing to do here
    elseif ($w -match '^verify:([12]):(changed|all)(:fresh)?$') {
      # The attempt number is goway's explicit argument, never read from the
      # environment or from anything the helper reports.
      $verify = $true; $attempt = [int]$Matches[1]; $level = $Matches[2]; $fresh = [bool]$Matches[3]
    } else { Die "run: unknown option $w" }
    $i++
  }
  [string[]]$cmd = @()
  if ($i -lt $A.Length) { $cmd = @($A[($i + 1)..($A.Length - 1)]) }
  if ($cmd.Count -eq 1 -and $cmd[0] -eq '@args') {
    $cmd = [string[]](Split-Nul ([IO.File]::ReadAllBytes((P $work @('args')))))
  }
  if ($cmd.Count -eq 0) { Die 'run: no command' }
  if (-not [IO.Directory]::Exists((P $work @('tree')))) { Die "run: no work dir at $work (was it synced?)" }

  Mark-Root $root
  # What the last automatic disk-budget eviction freed (it ran detached).
  $evictedLog = P $root @('evicted.log')
  if ([IO.File]::Exists($evictedLog)) {
    $note = Read-TextOrEmpty $evictedLog
    if ($note) { Write-Err $note }
    try { [IO.File]::Delete($evictedLog) } catch { }
  }
  New-Dir $cache
  [void](Test-ParentAlive)
  Lock-Dir 'work' $work $true
  # The lock protects the dir from here on; the starting-run markers are done.
  foreach ($m in @('born', 'creator')) { $f = P $work @($m); if ([IO.File]::Exists($f)) { try { [IO.File]::Delete($f) } catch { } } }
  Write-Text (P $work @('runner')) ("{0}`n" -f $PID)
  # The client's lifeline gave up on this run while it was starting: clean up and go.
  if ([IO.File]::Exists((P $work @('lost')))) { Lost-Note $work; Unlock-Key 'work'; Remove-Work $work; exit 143 }
  $meta = P $cache @('meta.json')
  if (-not [IO.File]::Exists($meta)) { [IO.File]::WriteAllBytes($meta, [Convert]::FromBase64String($cacheMeta)) }
  [IO.File]::SetLastWriteTimeUtc($meta, [DateTime]::UtcNow)

  # Settings that already exist win over goway's defaults: first the remote
  # environment, then the user's --env values; goway only fills in what is
  # still unset.
  $cargoBin = [IO.Path]::Combine((Get-Home), '.cargo', 'bin')
  if ([IO.Directory]::Exists($cargoBin) -and ($env:PATH -notlike "*$cargoBin*")) {
    $env:PATH = $cargoBin + [IO.Path]::PathSeparator + $env:PATH
  }
  if ($script:IsWin) { Add-GitUsrBin }
  $envFile = P $work @('env')
  if ([IO.File]::Exists($envFile)) {
    foreach ($kv in (Split-Nul ([IO.File]::ReadAllBytes($envFile)))) {
      $eq = $kv.IndexOf('=')
      if ($eq -gt 0) { [Environment]::SetEnvironmentVariable($kv.Substring(0, $eq), $kv.Substring($eq + 1)) }
    }
    [IO.File]::Delete($envFile)
  }

  # A GPU run holds one GPU slot until this process ends. It waits for the GPU
  # before taking a build slot, so a queue for GPUs never pins the build slots.
  if ($gpuPer -ge 1) { Acquire-Gpu $root $gpuPer }

  # Take a slot, preferring the one this worktree used last.
  $seedKeyFull = (Read-TextOrEmpty (P $work @('seed'))).Trim()
  $seedTail = $seedKeyFull.Substring($seedKeyFull.LastIndexOf('/') + 1)
  $aff = $null
  if ($seedTail -match '^[A-Za-z0-9._-]+$') { $aff = P $cache @("affinity-$seedTail") }
  $order = New-Object System.Collections.Generic.List[int]
  if ($aff) {
    $k = (Read-TextOrEmpty $aff).Trim()
    if ($k -match '^\d+$' -and [int]$k -lt $slots) { $order.Add([int]$k) }
  }
  for ($k = 0; $k -lt $slots; $k++) { if (-not $order.Contains($k)) { $order.Add($k) } }
  $slot = -1
  foreach ($k in $order) {
    # gc may have removed an expired cache dir just now: re-create it.
    New-Dir $cache
    $fs = Get-Lock (P $cache @("target-$k.lock")) $true 0
    if ($fs) { $script:Held['slot'] = $fs; $slot = $k; break }
  }
  if ($slot -lt 0) {
    # Every slot is busy: wait for the first to free, at most the run's --wait.
    $note = Slots-Busy-Note $cache $slots
    Write-Err "goway: all $slots build slots busy ($note); waiting up to $($slotWait)s for one`n"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($slot -lt 0) {
      foreach ($k in $order) {
        New-Dir $cache
        $fs = Get-Lock (P $cache @("target-$k.lock")) $true 0
        if ($fs) { $script:Held['slot'] = $fs; $slot = $k; break }
      }
      if ($slot -ge 0) { break }
      if ($sw.Elapsed.TotalSeconds -ge $slotWait) {
        $note = Slots-Busy-Note $cache $slots
        Write-Err "goway: no build slot freed within $($slotWait)s: all $slots build slots busy ($note); raise --wait or try another host`n"
        Unlock-Key 'work'
        Remove-Work $work
        Exit-Verb 125
      }
      Start-Sleep -Seconds 1
    }
  }
  if (-not [IO.File]::Exists($meta)) { [IO.File]::WriteAllBytes($meta, [Convert]::FromBase64String($cacheMeta)) }
  [IO.File]::SetLastWriteTimeUtc($meta, [DateTime]::UtcNow)
  # The slot's last use, for least-recently-used eviction (Evict-Slot).
  try { [IO.File]::SetLastWriteTimeUtc((P $cache @("target-$slot.lock")), [DateTime]::UtcNow) } catch { }
  if ($aff) { Write-Text $aff ([string]$slot) }
  # Builds bake absolute source paths into binaries, so a slot's binaries
  # always run against a tree at the same path: tree-<slot>.
  $rundir = P $cache @("tree-$slot")
  $target = P $cache @("target-$slot")
  if ($fresh -or $attempt -eq 2) {
    Wipe-Slot $slot $cache '' $root
    Write-Err "goway: building slot $slot from scratch (attempt $attempt)`n"
  }
  [void](Sync-Slot (P $work @('tree')) $rundir $work $keepIgnored $keepB64 $seedKeyFull $target)
  # The snapshot has done its job; its links hold no data of their own.
  try { Remove-Tree (P $work @('tree')) } catch { }
  # Copy integrity, before the command may start.
  $failed = $false
  if ($verify) {
    $paths = if ($level -eq 'all') { $script:AllReg } else { $script:Written }
    if (-not (Verify-Gate $work 1 $rundir ([string[]]$paths) $attempt)) { $failed = $true }
  }
  if ($failed) { Fail-Verify 1 $slot $cache $seedKeyFull $root $work }
  $stampsBefore = Get-Stamps $rundir
  if ($null -eq [Environment]::GetEnvironmentVariable('CARGO_TARGET_DIR')) {
    [Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR', $target)
  }
  if ($null -eq [Environment]::GetEnvironmentVariable('RUSTC_WRAPPER') -and (Resolve-Program 'sccache')) {
    [Environment]::SetEnvironmentVariable('RUSTC_WRAPPER', 'sccache')
    if (-not $env:SCCACHE_DIR) { [Environment]::SetEnvironmentVariable('SCCACHE_DIR', (P $cache @('sccache'))) }
    if (-not $env:SCCACHE_IDLE_TIMEOUT) { [Environment]::SetEnvironmentVariable('SCCACHE_IDLE_TIMEOUT', '300') }
  }
  # Compiler caches stay under a size cap unless the user chose one (MB suffix, as remote.sh).
  $csize = [long]0
  $tf = $ttls.Split(':')
  if ($tf.Length -ge 6 -and $tf[5] -match '^\d+$') { $csize = [long]$tf[5] }
  if ($csize -gt 0) {
    $mb = [string][long][math]::Floor($csize / 1048576) + 'M'
    if (-not $env:SCCACHE_CACHE_SIZE) { [Environment]::SetEnvironmentVariable('SCCACHE_CACHE_SIZE', $mb) }
    if (-not $env:CCACHE_MAXSIZE) { [Environment]::SetEnvironmentVariable('CCACHE_MAXSIZE', $mb) }
  }
  [Environment]::SetEnvironmentVariable('GOWAY', '1')
  [Environment]::SetEnvironmentVariable('GOWAY_RUN_ID', $runId)
  [Environment]::SetEnvironmentVariable('GOWAY_HOST', [Environment]::MachineName)

  if ([IO.File]::Exists((P $work @('lost')))) { Lost-Note $work; Unlock-Key 'slot'; Unlock-Key 'work'; Remove-Work $work; exit 143 }
  $pidFile = P $work @('pid')
  [IO.File]::WriteAllBytes($pidFile, @())
  $rc = 0
  # Hold a keep-awake request exactly while the job runs (this thread's own).
  $awake = $false
  if ($script:IsWin) { try { Load-Native; [GowayNative]::KeepAwake($true); $awake = $true } catch { Write-Err "goway-remote: keep-awake unavailable: $($_.Exception.Message)`n" } }
  try {
    if ($detect) { $rc = Shard-Run $detect $work $cmd $rundir $priority $pidFile }
    else { $rc = Run-Job $cmd $rundir $priority $pidFile }
  } finally {
    if ($awake) { try { [GowayNative]::KeepAwake($false) } catch { } }
  }
  Write-Text (P $work @('done')) ''
  Lost-Note $work

  # A failed command: before blaming the code, goway compares every synced
  # file the command did not itself change with the laptop's.
  if ($verify -and $rc -ne 0 -and -not [IO.File]::Exists((P $work @('lost'))) -and (Test-ParentAlive)) {
    $after = Get-Stamps $rundir
    $untouched = @()
    foreach ($p in $script:AllReg) {
      $b = $null; $c = $null
      if ($stampsBefore.TryGetValue($p, [ref]$b) -and $after.TryGetValue($p, [ref]$c) -and $b -eq $c) { $untouched += $p }
    }
    if (-not (Verify-Gate $work 2 $rundir ([string[]]$untouched) $attempt)) { Fail-Verify 2 $slot $cache $seedKeyFull $root $work }
  }
  if ($keep -eq '1') { Copy-Dir $rundir (P $work @('tree')) }
  Unlock-Key 'work'
  if ($keep -ne '1') { Remove-Work $work }
  # Cheap automatic gc of expired entries, detached.
  # With a disk budget it always starts (usage is checked there, not here).
  if ($ttls -and (($ttls.Split(':').Length -ge 5) -or (Test-GcDue $rootArg $ttls))) { Start-AutoGc $rootArg $ttls }
  exit $rc
}

# goway judged the copy bad (or never answered): wipe the slot and the seed,
# say so, and stop.
function Fail-Verify([int]$Phase, [int]$Slot, [string]$Cache, [string]$SeedKey, [string]$Root, [string]$Work) {
  Write-Err "goway-remote: the copy of the tree on this host did not verify (phase $Phase); slot $Slot is discarded`n"
  Unlock-Key 'slot'
  Wipe-Slot $Slot $Cache $SeedKey $Root
  Unlock-Key 'work'
  Remove-Work $Work
  exit 125
}

# ---- session ---------------------------------------------------------------

# Run one verb (not run, which owns this process's standard handles).
function Invoke-Verb([string]$Verb, [string[]]$Rest) {
  switch ($Verb) {
    'ping' { Write-Out "goway-remote ok`n" }
    'manifest' { Verb-manifest $Rest }
    'hashes' { Verb-hashes $Rest }
    'deletions' { Verb-deletions $Rest }
    'changes' { Verb-changes $Rest }
    'receive' { Verb-receive $Rest }
    'envfile' { Verb-envfile $Rest }
    'argsfile' { Verb-argsfile $Rest }
    'probe' { Verb-probe $Rest }
    'gc' { Verb-gc $Rest }
    'auto-gc' { Verb-auto-gc $Rest }
    'doctor' { Verb-doctor $Rest }
    'purge' { Verb-purge $Rest }
    'lifeline' { Verb-lifeline $Rest }
    'resolve' { Verb-resolve $Rest }
    'discard' { Verb-discard $Rest }
    'verify-wait' { Verb-verify-wait $Rest }
    'verify-verdict' { Verb-verify-verdict $Rest }
    default { Die "unknown verb: $Verb" }
  }
}

function Read-Frame([IO.Stream]$S, [int]$Count) {
  $buf = New-Object byte[] $Count
  $got = 0
  while ($got -lt $Count) {
    $n = $S.Read($buf, $got, $Count - $got)
    if ($n -le 0) { return $null }
    $got += $n
  }
  return ,$buf
}

function Write-Be32([IO.Stream]$S, [int]$V) {
  $b = [BitConverter]::GetBytes([int][Net.IPAddress]::HostToNetworkOrder($V))
  $S.Write($b, 0, 4)
}

# session: serve many verbs in this one process, saving a PowerShell start
# per call. Frames on stdin: 4-byte big-endian ARGS length and INPUT length,
# the NUL-terminated ARGS (verb first), the INPUT bytes. Answer: 4-byte
# exit code, OUT length, ERR length, OUT, ERR. The first line printed is
# "goway-session1". Ends at end of input.
function Verb-session([string[]]$A) {
  $script:InSession = $true
  $in = [Console]::OpenStandardInput()
  $realOut = $script:Stdout; $realErr = $script:Stderr
  Write-Out "goway-session1`n"
  while ($true) {
    $head = Read-Frame $in 8
    if ($null -eq $head) { break }
    $argsLen = [Net.IPAddress]::NetworkToHostOrder([BitConverter]::ToInt32($head, 0))
    $inLen = [Net.IPAddress]::NetworkToHostOrder([BitConverter]::ToInt32($head, 4))
    $argBytes = if ($argsLen -gt 0) { Read-Frame $in $argsLen } else { ,(New-Object byte[] 0) }
    $inBytes = if ($inLen -gt 0) { Read-Frame $in $inLen } else { ,(New-Object byte[] 0) }
    if ($null -eq $argBytes -or $null -eq $inBytes) { break }
    [string[]]$words = [string[]](Split-Words $argBytes)
    $out = New-Object IO.MemoryStream; $err = New-Object IO.MemoryStream
    $script:Stdout = $out; $script:Stderr = $err
    $script:SessionIn = New-Object IO.MemoryStream (, $inBytes)
    $code = 0
    try {
      if ($words.Count -eq 0 -or $words[0] -eq 'session' -or $words[0] -eq 'run' -or $words[0] -eq 'lifeline') { Die 'session: verb not allowed' }
      [string[]]$rest = @()
      if ($words.Count -gt 1) { $rest = [string[]]@($words[1..($words.Count - 1)]) }
      Invoke-Verb $words[0] $rest
    } catch {
      $m = [string]$_
      if ($m -match '^goway-exit:(\d+)$') { $code = [int]$Matches[1] }
      else {
        Write-Err ("goway-remote: failed: {0}`n" -f $_.Exception.Message)
        $code = 125
      }
    } finally {
      foreach ($k in @($script:Held.Keys)) { Unlock-Key $k }
    }
    $script:Stdout = $realOut; $script:Stderr = $realErr
    $ob = $out.ToArray(); $eb = $err.ToArray()
    Write-Be32 $realOut $code; Write-Be32 $realOut $ob.Length; Write-Be32 $realOut $eb.Length
    $realOut.Write($ob, 0, $ob.Length); $realOut.Write($eb, 0, $eb.Length)
    $realOut.Flush()
  }
}

# ---- main ----------------------------------------------------------------------

if ($args.Count -lt 1) { Die 'no verb' }
$verb = [string]$args[0]
[string[]]$rest = @()
if ($args.Count -gt 1) { $rest = @($args[1..($args.Count - 1)] | ForEach-Object { [string]$_ }) }
switch ($verb) {
  'run' { Verb-run $rest }
  'session' { Verb-session $rest }
  default { Invoke-Verb $verb $rest }
}
exit 0
