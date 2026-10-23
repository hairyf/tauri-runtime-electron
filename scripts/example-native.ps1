# Native, repo-scoped keyframes and mouse input for the example CDP harness.
# Use a fresh pwsh process. stdout is one JSON object; failures exit with code 1.
# Coordinates, rectangles and PNG pixels are physical screen pixels (negative OK).
# List/Rect do not focus windows. Other actions never restore hidden/minimized ones.
# Capture requires an unobscured window and a new .png path in an existing directory.
# Drag keeps its entire pointer path inside the original window rectangle for safety.
# Examples:
#   pwsh -NoProfile -File scripts/example-native.ps1 -Action List -ProcessId 1234
#   pwsh -NoProfile -File scripts/example-native.ps1 -Action Capture -WindowHandle 0x12345 -OutputPath target/before.png -Activate
#   pwsh -NoProfile -File scripts/example-native.ps1 -Action Drag -WindowHandle 0x12345 -StartX 400 -StartY 300 -EndX 460 -EndY 340 -Expected Moved -Activate
#   pwsh -NoProfile -File scripts/example-native.ps1 -Action SelfTest
[CmdletBinding()]
param(
  [ValidateSet('List', 'Rect', 'Capture', 'Drag', 'Click', 'DoubleClick', 'Close', 'SelfTest')]
  [string] $Action = 'List',
  [ValidateRange(0, 2147483647)] [int] $ProcessId = 0,
  [string] $WindowHandle,
  [string] $Title,
  [string] $RepositoryPath = (Split-Path -Parent $PSScriptRoot),
  [string] $OutputPath,
  [int] $StartX, [int] $StartY, [int] $EndX, [int] $EndY,
  [ValidateSet('Any', 'Moved', 'Stationary')] [string] $Expected = 'Any',
  [ValidateRange(0, 20)] [int] $Tolerance = 2,
  [ValidateRange(50, 5000)] [int] $DurationMs = 400,
  [ValidateRange(2, 200)] [int] $Steps = 12,
  [switch] $Activate
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$activation = $null

function Test-RepoProcess($Record, $Table, [string] $Root) {
  $seen = @{}
  $prefix = $Root.TrimEnd('\') + '\'
  $mainArgument = '(?i)(?:^|[\s"])' + [regex]::Escape($prefix + 'electron\main.cjs') + '(?:"|\s|$)'
  while ($null -ne $Record -and -not $seen.ContainsKey([int] $Record.ProcessId)) {
    $seen[[int] $Record.ProcessId] = $true
    $image = ([string] $Record.ExecutablePath).Replace('/', '\')
    if ($image.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) -or
        ([string] $Record.CommandLine).Replace('/', '\') -match $mainArgument) { return $true }
    $Record = $Table[[int] $Record.ParentProcessId]
  }
  return $false
}

function Test-Movement($Before, $After, [int] $Dx, [int] $Dy, [string] $Want, [int] $Slack) {
  $actualX = $After.left - $Before.left
  $actualY = $After.top - $Before.top
  $sameSize = $After.width -eq $Before.width -and $After.height -eq $Before.height
  $moved = [Math]::Abs($actualX) -gt $Slack -or [Math]::Abs($actualY) -gt $Slack
  $passed = switch ($Want) {
    'Moved' { $sameSize -and $moved -and [Math]::Abs($actualX - $Dx) -le $Slack -and [Math]::Abs($actualY - $Dy) -le $Slack }
    'Stationary' { $sameSize -and -not $moved }
    default { $null }
  }
  return @{ expected = $Want; passed = $passed; moved = $moved; dx = $actualX; dy = $actualY; sameSize = $sameSize }
}

try {
  if ($Action -eq 'SelfTest') {
    $before = @{ left = -200; top = 50; width = 800; height = 600 }
    $after = @{ left = -140; top = 90; width = 800; height = 600 }
    if (-not (Test-Movement $before $after 60 40 'Moved' 2).passed) { throw 'movement check failed' }
    if ((Test-Movement $before $before 60 40 'Moved' 2).passed) { throw 'stationary false positive' }
    if ((Test-Movement $before $after 0 0 'Stationary' 2).passed) { throw 'movement false positive' }
    if (-not (Test-Movement $before $before 0 0 'Stationary' 2).passed) { throw 'stationary check failed' }
    $table = @{
      1 = @{ ProcessId = 1; ParentProcessId = 0; ExecutablePath = 'E:\repo\target\app.exe'; CommandLine = '' }
      2 = @{ ProcessId = 2; ParentProcessId = 1; ExecutablePath = 'C:\cache\electron.exe'; CommandLine = '' }
      3 = @{ ProcessId = 3; ParentProcessId = 0; ExecutablePath = 'E:\repo-other\app.exe'; CommandLine = '' }
      4 = @{ ProcessId = 4; ParentProcessId = 4; ExecutablePath = 'C:\other.exe'; CommandLine = '' }
    }
    if (-not (Test-RepoProcess $table[2] $table 'E:\repo')) { throw 'repo ancestor rejected' }
    if (Test-RepoProcess $table[3] $table 'E:\repo') { throw 'repo prefix false positive' }
    if (Test-RepoProcess $table[4] $table 'E:\repo') { throw 'ancestor cycle false positive' }
  }
  if (-not $IsWindows) { throw 'This helper requires Windows and an interactive desktop.' }
  $repo = (Resolve-Path -LiteralPath $RepositoryPath).Path.TrimEnd('\')
  if (-not (Test-Path -LiteralPath (Join-Path $repo 'electron/main.cjs')) -or
      -not (Select-String -LiteralPath (Join-Path $repo 'Cargo.toml') -Pattern '^name = "tauri-runtime-electron"$' -Quiet)) {
    throw 'RepositoryPath must identify the tauri-runtime-electron repository, not a broad parent directory.'
  }

  [Console]::Error.WriteLine('[native-phase] Add-Type start')
  Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
public static class ExampleNative {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; public POINT(int x, int y) { X = x; Y = y; } }
  [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT { public int dx, dy; public uint mouseData, flags, time; public UIntPtr extra; }
  [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public MOUSEINPUT mouse; }
  delegate bool EnumProc(IntPtr hwnd, IntPtr data);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr data);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr hwnd);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr hwnd, out RECT rect);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool ClientToScreen(IntPtr hwnd, ref POINT point);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int length);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder text, int length);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr hwnd, uint flags);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr hwnd, uint command);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll", SetLastError=true)] public static extern bool AttachThreadInput(uint from, uint to, bool attach);
  [DllImport("user32.dll", EntryPoint="PostMessageW", SetLastError=true)] public static extern bool PostMessage(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT point);
  [DllImport("user32.dll")] public static extern short GetAsyncKeyState(int key);
  [DllImport("user32.dll")] public static extern uint GetDoubleClickTime();
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
  [DllImport("user32.dll", SetLastError=true)] static extern uint SendInput(uint count, INPUT[] inputs, int size);
  [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr hwnd, int attribute, out int value, int size);
  [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr hwnd, int attribute, out RECT value, int size);
  public static RECT FrameBounds(IntPtr hwnd) { RECT rect; if (DwmGetWindowAttribute(hwnd, 9, out rect, Marshal.SizeOf(typeof(RECT))) != 0 && !GetWindowRect(hwnd, out rect)) throw new Win32Exception(); return rect; }
  public static bool Cloaked(IntPtr hwnd) { int value; return DwmGetWindowAttribute(hwnd, 14, out value, 4) == 0 && value != 0; }
  public static string Title(IntPtr hwnd) { var text = new StringBuilder(4096); GetWindowText(hwnd, text, text.Capacity); return text.ToString(); }
  public static string ClassName(IntPtr hwnd) { var text = new StringBuilder(256); GetClassName(hwnd, text, text.Capacity); return text.ToString(); }
  public static IntPtr[] Windows() { var result = new List<IntPtr>(); EnumWindows((hwnd, data) => { result.Add(hwnd); return true; }, IntPtr.Zero); return result.ToArray(); }
  public static void Mouse(uint flags, int x = 0, int y = 0) {
    var input = new INPUT { type = 0, mouse = new MOUSEINPUT { flags = flags, dx = x, dy = y } };
    if (SendInput(1, new[] { input }, Marshal.SizeOf(typeof(INPUT))) != 1) throw new Win32Exception(Marshal.GetLastWin32Error(), "SendInput failed (possibly UIPI/elevation mismatch)");
  }
  public static void Move(int x, int y) {
    int left = GetSystemMetrics(76), top = GetSystemMetrics(77), width = GetSystemMetrics(78), height = GetSystemMetrics(79);
    if (width <= 0 || height <= 0 || x < left || y < top || x >= left + width || y >= top + height) throw new ArgumentException("Pointer coordinate is outside the virtual desktop");
    int dx = Math.Min(65535, (int)(((long)(x - left) * 65536 + 32768) / width));
    int dy = Math.Min(65535, (int)(((long)(y - top) * 65536 + 32768) / height));
    Mouse(0xC001, dx, dy); // MOVE | ABSOLUTE | VIRTUALDESK
  }
}
'@
  [Console]::Error.WriteLine('[native-phase] Add-Type done')

  if ($Action -eq 'SelfTest') {
    @{ ok = $true; action = $Action; checks = 7; interopCompiled = $true; nativeInput = $false } | ConvertTo-Json -Compress
    exit 0
  }

  function Get-Rect([IntPtr] $Handle) {
    $rect = [ExampleNative+RECT]::new()
    if (-not [ExampleNative]::GetWindowRect($Handle, [ref] $rect)) { throw 'GetWindowRect failed.' }
    return @{ x = $rect.Left; y = $rect.Top; left = $rect.Left; top = $rect.Top; right = $rect.Right; bottom = $rect.Bottom; width = $rect.Right - $rect.Left; height = $rect.Bottom - $rect.Top }
  }
  function Get-FrameRect([IntPtr] $Handle) {
    $rect = [ExampleNative]::FrameBounds($Handle)
    return @{ x = $rect.Left; y = $rect.Top; left = $rect.Left; top = $rect.Top; right = $rect.Right; bottom = $rect.Bottom; width = $rect.Right - $rect.Left; height = $rect.Bottom - $rect.Top }
  }
  function Assert-Target {
    [uint32] $ownerId = 0
    [void] [ExampleNative]::GetWindowThreadProcessId($handle, [ref] $ownerId)
    if (-not [ExampleNative]::IsWindow($handle) -or $ownerId -ne $targetProcessId) { throw 'Target window disappeared or its owner changed.' }
  }
  function Get-WindowInfo([IntPtr] $Handle, [int] $OwnerId) {
    $client = [ExampleNative+RECT]::new()
    $origin = [ExampleNative+POINT]::new(0, 0)
    if (-not [ExampleNative]::GetClientRect($Handle, [ref] $client) -or
        -not [ExampleNative]::ClientToScreen($Handle, [ref] $origin)) { throw 'Cannot read client coordinates.' }
    $bounds = Get-Rect $Handle
    $clientBounds = @{ x = $origin.X; y = $origin.Y; left = $origin.X; top = $origin.Y; width = $client.Right; height = $client.Bottom; right = $origin.X + $client.Right; bottom = $origin.Y + $client.Bottom }
    return @{
      hwnd = $Handle.ToInt64().ToString(); pid = $OwnerId; processId = $OwnerId; title = [ExampleNative]::Title($Handle); className = [ExampleNative]::ClassName($Handle)
      processPath = $processes[$OwnerId].ExecutablePath; dpi = [ExampleNative]::GetDpiForWindow($Handle)
      visible = [ExampleNative]::IsWindowVisible($Handle); minimized = [ExampleNative]::IsIconic($Handle)
      maximized = [ExampleNative]::IsZoomed($Handle); rect = $bounds; bounds = $bounds; frameBounds = Get-FrameRect $Handle
      clientRect = $clientBounds; clientBounds = $clientBounds
    }
  }
  function Test-Point($Rect, [int] $X, [int] $Y) {
    return $X -ge $Rect.left -and $X -lt $Rect.right -and $Y -ge $Rect.top -and $Y -lt $Rect.bottom
  }
  function Get-ForegroundInfo {
    $foreground = [ExampleNative]::GetAncestor([ExampleNative]::GetForegroundWindow(), 2)
    [uint32] $foregroundOwner = 0
    [void] [ExampleNative]::GetWindowThreadProcessId($foreground, [ref] $foregroundOwner)
    return @{ hwnd = $foreground.ToInt64().ToString(); pid = $foregroundOwner }
  }
  function Assert-IdleInput {
    foreach ($key in @(1, 2, 4, 16, 17, 18, 91, 92)) {
      if ([ExampleNative]::GetAsyncKeyState($key) -lt 0) { throw 'A mouse button or modifier is already held; refusing to interfere.' }
    }
  }
  function Assert-Foreground {
    Assert-Target
    if ([ExampleNative]::GetAncestor([ExampleNative]::GetForegroundWindow(), 2) -ne $handle) {
      $foreground = Get-ForegroundInfo
      throw "Target is not foreground (target HWND=$handle PID=$targetProcessId; foreground HWND=$($foreground.hwnd) PID=$($foreground.pid); Activate=$Activate); use -Activate or focus only this target manually."
    }
  }
  function Assert-Point([int] $X, [int] $Y) {
    Assert-Foreground
    if ([ExampleNative]::GetAncestor([ExampleNative]::WindowFromPoint([ExampleNative+POINT]::new($X, $Y)), 2) -ne $handle) {
      throw "Pointer coordinate ($X, $Y) is covered by another window; refusing input."
    }
  }
  function Assert-Unobscured($Rect) {
    # Conservative: even a transparent overlapping top-level window causes a refusal.
    $above = [ExampleNative]::GetWindow($handle, 3) # GW_HWNDPREV
    $seen = @{}
    while ($above -ne [IntPtr]::Zero -and -not $seen.ContainsKey($above.ToInt64())) {
      $seen[$above.ToInt64()] = $true
      if ([ExampleNative]::IsWindowVisible($above) -and -not [ExampleNative]::IsIconic($above) -and -not [ExampleNative]::Cloaked($above)) {
        $other = Get-Rect $above
        if ($other.left -lt $Rect.right -and $other.right -gt $Rect.left -and $other.top -lt $Rect.bottom -and $other.bottom -gt $Rect.top) {
          throw 'Another top-level window overlaps the target; refusing screen capture/input.'
        }
      }
      $above = [ExampleNative]::GetWindow($above, 3)
    }
  }

  $oldDpi = [ExampleNative]::SetThreadDpiAwarenessContext([IntPtr]::new(-4)) # PER_MONITOR_AWARE_V2
  if ($oldDpi -eq [IntPtr]::Zero) { throw 'Cannot enable per-monitor physical coordinates (requires Windows 10+).' }
  try {
    $processes = @{}
    [Console]::Error.WriteLine('[native-phase] repository CIM start')
    Get-CimInstance Win32_Process | ForEach-Object { $processes[[int] $_.ProcessId] = $_ }
    [Console]::Error.WriteLine('[native-phase] repository CIM done')
    if ($ProcessId -ne 0 -and (-not $processes.ContainsKey($ProcessId) -or -not (Test-RepoProcess $processes[$ProcessId] $processes $repo))) {
      throw 'ProcessId is not a live process belonging to this repository.'
    }
    $requestedHandle = [IntPtr]::Zero
    if ($WindowHandle) {
      $value = if ($WindowHandle.StartsWith('0x', [StringComparison]::OrdinalIgnoreCase)) { [Convert]::ToInt64($WindowHandle.Substring(2), 16) } else { [long]::Parse($WindowHandle) }
      if ($value -le 0) { throw 'WindowHandle must be a positive decimal or hexadecimal HWND.' }
      $requestedHandle = [IntPtr]::new($value)
    }
    [Console]::Error.WriteLine('[native-phase] window enumeration start')
    $windows = @([ExampleNative]::Windows() | ForEach-Object {
      $candidate = $_
      [uint32] $ownerId = 0
      [void] [ExampleNative]::GetWindowThreadProcessId($candidate, [ref] $ownerId)
      $record = $processes[[int] $ownerId]
      if ($null -eq $record -or ($ProcessId -ne 0 -and $ownerId -ne $ProcessId) -or
          ($requestedHandle -ne [IntPtr]::Zero -and $requestedHandle -ne $candidate)) { return }
      $caption = [ExampleNative]::Title($candidate)
      if ($record.Name -match '^(dsh|dsh-tauri|deepseek-harness)(\.exe)?$' -or $caption -match '(?i)DeepSeek Harness|^DSH(?:\s|$)') { return }
      if (-not (Test-RepoProcess $record $processes $repo) -or ($Title -and $caption -cne $Title)) { return }
      Get-WindowInfo $candidate ([int] $ownerId)
    })
    [Console]::Error.WriteLine('[native-phase] window enumeration done')
    if ($Action -eq 'List') {
      @{ ok = $true; action = $Action; windows = $windows } | ConvertTo-Json -Depth 8 -Compress
      [Console]::Error.WriteLine('[native-phase] List done')
      exit 0
    }
    if ($windows.Count -ne 1) { throw "Expected one repo-scoped window, found $($windows.Count). Use List, then an exact WindowHandle (and optionally ProcessId/Title)." }
    $target = $windows[0]
    $handle = [IntPtr]::new([long] $target.hwnd)
    $targetProcessId = $target.processId
    Assert-Target
    if ($Action -eq 'Rect') {
      @{ ok = $true; action = $Action; window = $target } | ConvertTo-Json -Depth 8 -Compress
      exit 0
    }
    if ($Action -eq 'Close') {
      if (-not [ExampleNative]::PostMessage($handle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)) { throw 'PostMessageW(WM_CLOSE) failed.' }
      @{ ok = $true; action = $Action; window = $target; closeRequested = $true; note = 'WM_CLOSE posted; caller must await window/process exit.' } | ConvertTo-Json -Depth 8 -Compress
      exit 0
    }
    if (-not $target.visible -or $target.minimized -or [ExampleNative]::Cloaked($handle)) { throw 'Hidden, minimized or cloaked windows cannot be captured or receive input.' }
    if ($Activate) {
      Assert-IdleInput
      $activation = @{ requested = $true; before = Get-ForegroundInfo; direct = [ExampleNative]::SetForegroundWindow($handle); attachments = @() }
      $deadline = [datetime]::UtcNow.AddMilliseconds(200)
      while ([ExampleNative]::GetAncestor([ExampleNative]::GetForegroundWindow(), 2) -ne $handle -and [datetime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 20 }
      if ([ExampleNative]::GetAncestor([ExampleNative]::GetForegroundWindow(), 2) -ne $handle) {
        Assert-Target
        Assert-IdleInput # AttachThreadInput resets key state, so check before attaching.
        $foreground = [ExampleNative]::GetForegroundWindow()
        [uint32] $foregroundOwner = 0
        $foregroundThread = [ExampleNative]::GetWindowThreadProcessId($foreground, [ref] $foregroundOwner)
        [uint32] $targetOwner = 0
        $targetThread = [ExampleNative]::GetWindowThreadProcessId($handle, [ref] $targetOwner)
        $currentThread = [ExampleNative]::GetCurrentThreadId()
        $attached = @()
        try {
          foreach ($threadId in @($foregroundThread, $targetThread) | Select-Object -Unique) {
            if ($threadId -eq 0 -or $threadId -eq $currentThread) { continue }
            $joined = [ExampleNative]::AttachThreadInput($currentThread, $threadId, $true)
            $activation.attachments += @{ thread = $threadId; attached = $joined; error = $(if ($joined) { 0 } else { [Runtime.InteropServices.Marshal]::GetLastWin32Error() }) }
            if (-not $joined) { throw 'Cannot attach target/foreground input threads; refusing activation.' }
            $attached += $threadId
          }
          Assert-Target
          $activation.retry = [ExampleNative]::SetForegroundWindow($handle)
          $deadline = [datetime]::UtcNow.AddMilliseconds(500)
          while ([ExampleNative]::GetAncestor([ExampleNative]::GetForegroundWindow(), 2) -ne $handle -and [datetime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 20 }
        } finally {
          foreach ($threadId in $attached) { [void] [ExampleNative]::AttachThreadInput($currentThread, $threadId, $false) }
        }
      }
      $activation.after = Get-ForegroundInfo
      Assert-Foreground
    }
    $before = Get-Rect $handle
    $frame = Get-FrameRect $handle
    if ($before.width -le 0 -or $before.height -le 0 -or $frame.width -le 0 -or $frame.height -le 0) { throw 'Target has an empty window rectangle.' }
    Assert-Unobscured $frame
    $desktop = @{ left = [ExampleNative]::GetSystemMetrics(76); top = [ExampleNative]::GetSystemMetrics(77); right = [ExampleNative]::GetSystemMetrics(76) + [ExampleNative]::GetSystemMetrics(78); bottom = [ExampleNative]::GetSystemMetrics(77) + [ExampleNative]::GetSystemMetrics(79) }
    if (-not (Test-Point $desktop $frame.left $frame.top) -or -not (Test-Point $desktop ($frame.right - 1) ($frame.bottom - 1))) { throw 'Target is partly off-screen; refusing a cropped keyframe/input.' }

    if ($Action -eq 'Capture') {
      if (-not $OutputPath) { throw 'Capture requires OutputPath.' }
      $output = [IO.Path]::GetFullPath($OutputPath)
      if ([IO.Path]::GetExtension($output) -ine '.png' -or -not [IO.Directory]::Exists([IO.Path]::GetDirectoryName($output))) { throw 'OutputPath must be a .png in an existing directory.' }
      Add-Type -AssemblyName System.Drawing
      $bitmap = [Drawing.Bitmap]::new($frame.width, $frame.height)
      $graphics = $null
      $stream = $null
      try {
        $graphics = [Drawing.Graphics]::FromImage($bitmap)
        Assert-Target
        Assert-Unobscured $frame
        $graphics.CopyFromScreen($frame.left, $frame.top, 0, 0, $bitmap.Size, [Drawing.CopyPixelOperation]::SourceCopy)
        Assert-Target
        $currentFrame = Get-FrameRect $handle
        if ($currentFrame.left -ne $frame.left -or $currentFrame.top -ne $frame.top -or $currentFrame.width -ne $frame.width -or $currentFrame.height -ne $frame.height) { throw 'Target moved/resized during capture; retry after it settles.' }
        Assert-Unobscured $frame
        $stream = [IO.File]::Open($output, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
        $bitmap.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
      } finally {
        if ($null -ne $stream) { $stream.Dispose() }
        if ($null -ne $graphics) { $graphics.Dispose() }
        $bitmap.Dispose()
      }
      @{ ok = $true; action = $Action; window = Get-WindowInfo $handle $targetProcessId; outputPath = $output; capture = 'visible-screen-with-frame'; captureRect = $frame } | ConvertTo-Json -Depth 8 -Compress
      exit 0
    }

    if (-not $PSBoundParameters.ContainsKey('StartX') -or -not $PSBoundParameters.ContainsKey('StartY')) { throw 'Input requires physical StartX and StartY.' }
    if ($Action -eq 'Drag' -and (-not $PSBoundParameters.ContainsKey('EndX') -or -not $PSBoundParameters.ContainsKey('EndY'))) { throw 'Drag requires EndX and EndY.' }
    if ($Action -ne 'Drag') { $EndX = $StartX; $EndY = $StartY }
    if (-not (Test-Point $before $StartX $StartY) -or -not (Test-Point $before $EndX $EndY)) { throw 'The entire pointer path must stay within the original target rectangle.' }
    Assert-IdleInput
    Assert-Point $StartX $StartY
    [ExampleNative]::Move($StartX, $StartY)
    Start-Sleep -Milliseconds 40
    $pressed = $false
    try {
      $clicks = if ($Action -eq 'DoubleClick') { 2 } else { 1 }
      for ($click = 0; $click -lt $clicks; $click++) {
        Assert-Point $StartX $StartY
        $pressed = $true
        [ExampleNative]::Mouse(0x0002) # LEFTDOWN
        if ($Action -eq 'Drag') {
          Start-Sleep -Milliseconds 70
          for ($step = 1; $step -le $Steps; $step++) {
            $nextX = $StartX + [int] [Math]::Round(($EndX - $StartX) * $step / $Steps)
            $nextY = $StartY + [int] [Math]::Round(($EndY - $StartY) * $step / $Steps)
            Assert-Point $nextX $nextY
            [ExampleNative]::Move($nextX, $nextY)
            Start-Sleep -Milliseconds ([int] [Math]::Ceiling($DurationMs / $Steps))
          }
        } else { Start-Sleep -Milliseconds 40 }
        [ExampleNative]::Mouse(0x0004) # LEFTUP
        $pressed = $false
        if ($click + 1 -lt $clicks) { Start-Sleep -Milliseconds ([int] [Math]::Min(80, [ExampleNative]::GetDoubleClickTime() / 4)) }
      }
    } finally {
      if ($pressed) { [ExampleNative]::Mouse(0x0004) }
    }
    Start-Sleep -Milliseconds 150
    Assert-Target
    $after = Get-Rect $handle
    $assertion = Test-Movement $before $after ($EndX - $StartX) ($EndY - $StartY) $Expected $Tolerance
    $ok = $Expected -eq 'Any' -or $assertion.passed
    @{ ok = $ok; action = $Action; window = Get-WindowInfo $handle $targetProcessId; before = $before; after = $after; actualDx = $assertion.dx; actualDy = $assertion.dy; assertion = $assertion; startInClient = Test-Point $target.clientRect $StartX $StartY } | ConvertTo-Json -Depth 8 -Compress
    if (-not $ok) { exit 1 }
  } finally {
    [void] [ExampleNative]::SetThreadDpiAwarenessContext($oldDpi)
  }
} catch {
  @{ ok = $false; action = $Action; error = $_.Exception.Message; activation = $activation } | ConvertTo-Json -Depth 8 -Compress
  exit 1
}
