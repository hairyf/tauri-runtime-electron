param([int]$RootPid,[string]$Action='state',[double]$X=0,[double]$Y=0,[int]$Dx=60,[int]$Dy=35,[int]$Clicks=1,[string]$OutputPath='',[string]$Title='',[int]$Width=0,[int]$Height=0)
$ErrorActionPreference = 'Stop'
# Non-interactive compile/state check: & ./scripts/verify-native.ps1 -Action state -RootPid $PID
# Title matches exactly; Clicks is reserved, and doubleclick always sends two clicks.
if ($RootPid -le 0) { throw 'RootPid must be a positive process ID.' }
if ($Action -notin @('state','raise','place','drag','doubleclick','capture','resize')) { throw "Unknown action: $Action" }
if ($Action -eq 'resize' -and ($Width -le 0 -or $Height -le 0)) { throw 'Width and Height must be positive physical client pixel sizes.' }
if (-not ('NativeVerify' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
public sealed class NativeWindowState {
    public long handle; public string title;
    public int x, y, width, height, clientX, clientY, clientWidth, clientHeight;
    public bool visible;
}
public static class NativeVerify {
    [StructLayout(LayoutKind.Sequential)] struct Rect { public int left, top, right, bottom; }
    [StructLayout(LayoutKind.Sequential)] struct Point { public int x, y; }
    delegate bool EnumProc(IntPtr window, IntPtr data);
    static HashSet<uint> processes;
    [DllImport("user32.dll", SetLastError=true)] static extern bool EnumWindows(EnumProc callback, IntPtr data);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr window, StringBuilder text, int length);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowTextLength(IntPtr window);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr window, StringBuilder text, int length);
    [DllImport("user32.dll", SetLastError=true)] static extern bool GetWindowRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll", SetLastError=true)] static extern bool GetClientRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll", SetLastError=true)] static extern bool ClientToScreen(IntPtr window, ref Point point);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr window);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr window, int command);
    [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr window);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point point);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr window, uint flags);
    [DllImport("user32.dll", EntryPoint="GetWindowLongW")] static extern int GetWindowLong(IntPtr window, int index);
    [DllImport("user32.dll", SetLastError=true)] static extern bool SetWindowPos(IntPtr window, IntPtr order, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll", SetLastError=true)] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
    static void Check(bool ok) { if (!ok) throw new Win32Exception(Marshal.GetLastWin32Error()); }
    static bool Own(IntPtr window) {
        uint process; GetWindowThreadProcessId(window, out process);
        if (!processes.Contains(process)) return false;
        var name = new StringBuilder(256); GetClassName(window, name, name.Capacity);
        return name.ToString() == "Chrome_WidgetWin_1";
    }
    static void Require(IntPtr window) { if (!Own(window)) throw new InvalidOperationException("Window is no longer in the scoped process tree."); }
    public static NativeWindowState Read(IntPtr window) {
        Require(window); Rect rect, client; var origin = new Point();
        Check(GetWindowRect(window, out rect)); Check(GetClientRect(window, out client)); Check(ClientToScreen(window, ref origin));
        var title = new StringBuilder(GetWindowTextLength(window) + 1); GetWindowText(window, title, title.Capacity);
        return new NativeWindowState { handle=window.ToInt64(), title=title.ToString(), x=rect.left, y=rect.top,
            width=rect.right-rect.left, height=rect.bottom-rect.top, clientX=origin.x, clientY=origin.y,
            clientWidth=client.right-client.left, clientHeight=client.bottom-client.top, visible=IsWindowVisible(window) };
    }
    public static NativeWindowState[] Windows(uint[] pids) {
        processes = new HashSet<uint>(pids); var handles = new List<IntPtr>();
        Check(EnumWindows((window, data) => { if (Own(window)) handles.Add(window); return true; }, IntPtr.Zero));
        return handles.ConvertAll(Read).ToArray();
    }
    public static void Raise(IntPtr window) {
        Require(window); ShowWindow(window, IsIconic(window) ? 9 : 5); BringWindowToTop(window);
        if (!SetForegroundWindow(window)) {
            bool topmost = (GetWindowLong(window, -20) & 8) != 0;
            try {
                Check(SetWindowPos(window, new IntPtr(-1), 0, 0, 0, 0, 0x43)); // Show, no move/resize.
                Thread.Sleep(60); SetForegroundWindow(window);
            } finally {
                if (!topmost && Own(window)) Check(SetWindowPos(window, new IntPtr(-2), 0, 0, 0, 0, 0x13));
            }
        }
        if (GetForegroundWindow() != window) throw new InvalidOperationException("Windows denied foreground activation.");
    }
    public static void Place(IntPtr window) { Require(window); Check(SetWindowPos(window, IntPtr.Zero, 120, 80, 0, 0, 0x15)); }
    public static void Resize(IntPtr window, int width, int height) {
        var state = Read(window);
        Check(SetWindowPos(window, IntPtr.Zero, 0, 0, checked(width+state.width-state.clientWidth), checked(height+state.height-state.clientHeight), 0x16));
    }
    public static void Aim(IntPtr window, int x, int y) {
        Require(window);
        if (GetForegroundWindow() != window || GetAncestor(WindowFromPoint(new Point { x=x, y=y }), 2) != window)
            throw new InvalidOperationException("Input point is obscured or outside the foreground target window.");
        Cursor(x, y);
    }
    public static void Cursor(int x, int y) { Check(SetCursorPos(x, y)); }
}
'@
}
[void][NativeVerify]::SetProcessDPIAware()
$children = Get-CimInstance Win32_Process -Property ProcessId,ParentProcessId | Group-Object ParentProcessId -AsHashTable -AsString
$processIds = [Collections.Generic.HashSet[uint32]]::new()
$pending = [Collections.Generic.Queue[uint32]]::new()
[void]$processIds.Add([uint32]$RootPid); $pending.Enqueue([uint32]$RootPid)
while ($pending.Count) {
    foreach ($child in $children[[string]$pending.Dequeue()]) {
        if ($processIds.Add([uint32]$child.ProcessId)) { $pending.Enqueue([uint32]$child.ProcessId) }
    }
}
$windows = @([NativeVerify]::Windows([uint32[]]@($processIds)) | Where-Object { !$Title -or $_.title -ceq $Title })
if ($Action -eq 'state') { ConvertTo-Json -InputObject $windows -Depth 4 -Compress; return }
if ($Action -eq 'capture') {
    $worktree = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
    $allowed = [IO.Path]::GetFullPath((Join-Path $worktree '.cache/acceptance')) + [IO.Path]::DirectorySeparatorChar
    if (!$OutputPath) { throw 'capture requires OutputPath inside .cache/acceptance/.' }
    if (![IO.Path]::IsPathRooted($OutputPath)) { $OutputPath = Join-Path $worktree $OutputPath }
    $OutputPath = [IO.Path]::GetFullPath($OutputPath)
    if (!$OutputPath.StartsWith($allowed, [StringComparison]::OrdinalIgnoreCase) -or ![IO.Path]::GetFileName($OutputPath)) { throw 'OutputPath must be a file inside this worktree .cache/acceptance/.' }
    for ($part = $OutputPath; $part; $part = [IO.Path]::GetDirectoryName($part)) {
        $item = Get-Item -LiteralPath $part -Force -ErrorAction SilentlyContinue
        if ($item -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'OutputPath cannot traverse a symbolic link or junction.' }
    }
}
$targets = @($windows | Where-Object visible)
if ($targets.Count -ne 1) { throw "Expected one visible scoped Chrome_WidgetWin_1 window; found $($targets.Count). Specify an exact Title if ambiguous." }
$handle = [IntPtr]$targets[0].handle
if ($Action -in @('raise','drag','doubleclick','capture')) { [NativeVerify]::Raise($handle); Start-Sleep -Milliseconds 120 }
$before = [NativeVerify]::Read($handle)
$result = switch ($Action) {
    'raise' { $before }
    'place' { [NativeVerify]::Place($handle); Start-Sleep -Milliseconds 100; [NativeVerify]::Read($handle) }
    'resize' { [NativeVerify]::Resize($handle, $Width, $Height); Start-Sleep -Milliseconds 100; @{ before=$before; after=[NativeVerify]::Read($handle) } }
    { $_ -in @('drag','doubleclick') } {
        if ([double]::IsNaN($X) -or [double]::IsInfinity($X) -or [double]::IsNaN($Y) -or [double]::IsInfinity($Y)) { throw 'X and Y must be finite client pixel coordinates.' }
        $px = [int][Math]::Round($X); $py = [int][Math]::Round($Y)
        if ($px -lt 0 -or $py -lt 0 -or $px -ge $before.clientWidth -or $py -ge $before.clientHeight) { throw 'X and Y must lie inside the target client area.' }
        $sx = $before.clientX + $px; $sy = $before.clientY + $py
        [NativeVerify]::Aim($handle, $sx, $sy); Start-Sleep -Milliseconds 40
        if ($Action -eq 'drag') {
            [NativeVerify]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
            try {
                Start-Sleep -Milliseconds 40
                for ($step = 1; $step -le 10; $step++) {
                    [NativeVerify]::Cursor([int][Math]::Round($sx + $Dx * $step / 10.0), [int][Math]::Round($sy + $Dy * $step / 10.0))
                    Start-Sleep -Milliseconds 20
                }
            } finally { [NativeVerify]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero) }
        } else {
            for ($click = 0; $click -lt 2; $click++) {
                [NativeVerify]::Aim($handle, $sx, $sy)
                [NativeVerify]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
                try { Start-Sleep -Milliseconds 25 } finally { [NativeVerify]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero) }
                Start-Sleep -Milliseconds 40
            }
        }
        Start-Sleep -Milliseconds 250
        @{ before=$before; after=[NativeVerify]::Read($handle) }
    }
    'capture' {
        Add-Type -AssemblyName System.Drawing
        [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($OutputPath))
        if ($before.width -le 0 -or $before.height -le 0) { throw 'Target has empty window bounds.' }
        $bitmap = [Drawing.Bitmap]::new($before.width, $before.height); $graphics = $null
        try {
            $graphics = [Drawing.Graphics]::FromImage($bitmap)
            $graphics.CopyFromScreen($before.x, $before.y, 0, 0, $bitmap.Size, [Drawing.CopyPixelOperation]::SourceCopy)
            $bitmap.Save($OutputPath, [Drawing.Imaging.ImageFormat]::Png)
        } finally { if ($graphics) { $graphics.Dispose() }; $bitmap.Dispose() }
        $before
    }
}
ConvertTo-Json -InputObject $result -Depth 4 -Compress
