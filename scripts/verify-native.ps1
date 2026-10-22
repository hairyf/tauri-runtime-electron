param([int]$RootPid,[string]$Action='state',[double]$X=0,[double]$Y=0,[int]$Dx=60,[int]$Dy=35,[int]$Clicks=1,[string]$OutputPath='',[string]$Title='',[int]$Width=0,[int]$Height=0,[string]$ItemText='',[ValidateSet('window','popup','tray')][string]$Surface='window',[ValidateSet('left','right')][string]$Button='left')
$ErrorActionPreference = 'Stop'
# Non-interactive compile/state check: & ./scripts/verify-native.ps1 -Action state -RootPid $PID
# Title matches exactly; Clicks is reserved, and doubleclick always sends two clicks.
if ($RootPid -le 0) { throw 'RootPid must be a positive process ID.' }
if ($Action -notin @('state','raise','place','drag','doubleclick','capture','resize','popup','menu-click','dismiss','tray-state','tray-click','menubar','menubar-click','watch-splash')) { throw "Unknown action: $Action" }
if ($Action -in @('menu-click','tray-state','tray-click','menubar-click') -and !$ItemText) { throw "$Action requires exact ItemText." }
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
    static bool Own(IntPtr window, string className = "Chrome_WidgetWin_1") {
        uint process; GetWindowThreadProcessId(window, out process);
        if (!processes.Contains(process)) return false;
        var name = new StringBuilder(256); GetClassName(window, name, name.Capacity);
        return name.ToString() == className;
    }
    public static bool ScopedPopup(IntPtr window) { return Own(window, "#32768") && IsWindowVisible(window); }
    [DllImport("user32.dll", SetLastError=true)] static extern IntPtr SendMessageTimeoutW(IntPtr window, uint message, IntPtr wparam, IntPtr lparam, uint flags, uint timeout, out UIntPtr result);
    [DllImport("user32.dll")] static extern int GetMenuItemCount(IntPtr menu);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetMenuStringW(IntPtr menu, uint item, StringBuilder text, int length, uint flags);
    [DllImport("user32.dll")] static extern uint GetMenuState(IntPtr menu, uint item, uint flags);
    public static string MenuChecked(IntPtr window, string item) {
        if (!ScopedPopup(window)) throw new InvalidOperationException("Scoped popup is no longer open.");
        UIntPtr result;
        if (SendMessageTimeoutW(window, 0x01E1, IntPtr.Zero, IntPtr.Zero, 2, 1000, out result) == IntPtr.Zero) return null;
        var menu = new IntPtr(unchecked((long)result.ToUInt64()));
        for (uint i=0; i<GetMenuItemCount(menu); i++) {
            var name = new StringBuilder(1024); GetMenuStringW(menu, i, name, name.Capacity, 0x400);
            if (name.ToString().Split('\t')[0].Replace("&", "") != item) continue;
            uint state = GetMenuState(menu, i, 0x400);
            return state == 0xffffffff ? null : ((state & 8) != 0 ? "On" : "Off");
        }
        return null;
    }
    public static long[] Popups() {
        var handles = new List<long>();
        Check(EnumWindows((window, data) => { if (ScopedPopup(window)) handles.Add(window.ToInt64()); return true; }, IntPtr.Zero));
        return handles.ToArray();
    }
    public static void ClickAt(int x, int y, bool right) {
        Cursor(x, y); Thread.Sleep(40);
        mouse_event(right ? 8u : 2u, 0, 0, 0, UIntPtr.Zero);
        try { Thread.Sleep(25); } finally { mouse_event(right ? 16u : 4u, 0, 0, 0, UIntPtr.Zero); }
    }
    public static void PopupClick(IntPtr window, int x, int y) {
        if (!ScopedPopup(window) || GetAncestor(WindowFromPoint(new Point { x=x, y=y }), 2) != window)
            throw new InvalidOperationException("Scoped popup item is obscured or no longer open.");
        ClickAt(x, y, false);
    }
    [DllImport("user32.dll")] static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
    public static void DismissPopup(IntPtr window) {
        if (!ScopedPopup(window)) throw new InvalidOperationException("Scoped popup is no longer open.");
        uint process; GetWindowThreadProcessId(GetForegroundWindow(), out process);
        if (!processes.Contains(process)) throw new InvalidOperationException("Foreground window is outside the scoped process tree.");
        keybd_event(0x1B, 0, 0, UIntPtr.Zero);
        keybd_event(0x1B, 0, 2, UIntPtr.Zero);
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
function Read-Windows {
    $children = Get-CimInstance Win32_Process -Property ProcessId,ParentProcessId | Group-Object ParentProcessId -AsHashTable -AsString
    $processIds = [Collections.Generic.HashSet[uint32]]::new()
    $pending = [Collections.Generic.Queue[uint32]]::new()
    [void]$processIds.Add([uint32]$RootPid); $pending.Enqueue([uint32]$RootPid)
    while ($pending.Count) {
        foreach ($child in $children[[string]$pending.Dequeue()]) {
            if ($processIds.Add([uint32]$child.ProcessId)) { $pending.Enqueue([uint32]$child.ProcessId) }
        }
    }
    [NativeVerify]::Windows([uint32[]]@($processIds)) | Where-Object { !$Title -or $_.title -ceq $Title }
}
if ($Action -eq 'watch-splash') {
    [Console]::Out.WriteLine('READY'); [Console]::Out.Flush()
    $deadline = [DateTime]::UtcNow.AddSeconds(600)
    do {
        if (!(Get-Process -Id $RootPid -ErrorAction SilentlyContinue)) { throw 'Scoped root process exited before splash observation.' }
        $observed = @(Read-Windows)
        if ($observed.Count -eq 2 -and @($observed | Where-Object visible).Count -eq 1 -and @($observed | Where-Object { $_.clientWidth -gt 0 -and $_.clientHeight -gt 0 }).Count -eq 2) {
            ConvertTo-Json -InputObject $observed -Depth 4 -Compress; return
        }
        Start-Sleep -Milliseconds 150
    } while ([DateTime]::UtcNow -lt $deadline)
    throw 'Timed out observing one visible splash and one hidden main window with positive client sizes.'
}
$windows = @(Read-Windows)
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
function Save-Rectangle($rect) {
    Add-Type -AssemblyName System.Drawing
    [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($OutputPath))
    if ($rect.width -le 0 -or $rect.height -le 0) { throw 'Target has empty bounds.' }
    $bitmap = [Drawing.Bitmap]::new([int]$rect.width, [int]$rect.height); $graphics = $null
    try {
        $graphics = [Drawing.Graphics]::FromImage($bitmap)
        $graphics.CopyFromScreen([int]$rect.x, [int]$rect.y, 0, 0, $bitmap.Size, [Drawing.CopyPixelOperation]::SourceCopy)
        $bitmap.Save($OutputPath, [Drawing.Imaging.ImageFormat]::Png)
    } finally { if ($graphics) { $graphics.Dispose() }; $bitmap.Dispose() }
}
if ($Action -in @('popup','menu-click','dismiss','tray-state','tray-click','menubar','menubar-click') -or ($Action -eq 'capture' -and $Surface -ne 'window')) {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    function Bounds($rect) { @{ x=[int]$rect.X; y=[int]$rect.Y; width=[int]$rect.Width; height=[int]$rect.Height } }
    if ($Action -in @('menubar','menubar-click')) {
        $targets = @($windows | Where-Object visible)
        if ($targets.Count -ne 1) { throw 'Menubar requires one visible scoped app window (use exact Title when ambiguous).' }
        $handle = [IntPtr]$targets[0].handle
        if ($Action -eq 'menubar-click') { [NativeVerify]::Raise($handle) }
        $element = [System.Windows.Automation.AutomationElement]::FromHandle($handle)
        $condition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::MenuBar)
        $bars = @($element.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition))
        $items = @(foreach ($bar in $bars) {
            $condition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::MenuItem)
            foreach ($entry in $bar.FindAll([System.Windows.Automation.TreeScope]::Children, $condition)) {
                $info = $entry.Current
                @{ name=($info.Name -split "`t")[0]; enabled=$info.IsEnabled; offscreen=$info.IsOffscreen; bounds=(Bounds $info.BoundingRectangle) }
            }
        })
        $result = @{ handle=$handle.ToInt64(); title=$targets[0].title; items=$items }
        if ($Action -eq 'menubar-click') {
            $matching = @($items | Where-Object { $_.name -ceq $ItemText -and $_.enabled -and !$_.offscreen -and $_.bounds.width -gt 0 -and $_.bounds.height -gt 0 })
            if ($matching.Count -ne 1) { throw "Expected one visible enabled menubar item '$ItemText'; found $($matching.Count)." }
            $rect=$matching[0].bounds; $px=[int]($rect.x+$rect.width/2); $py=[int]($rect.y+$rect.height/2)
            [NativeVerify]::Aim($handle, $px, $py); [NativeVerify]::ClickAt($px, $py, $false)
        }
        ConvertTo-Json -InputObject $result -Depth 8 -Compress; return
    }
    function Get-Popups {
        foreach ($popupHandle in [NativeVerify]::Popups()) {
            $element = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$popupHandle)
            $menuCondition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::MenuItem)
            $items = @(foreach ($item in $element.FindAll([System.Windows.Automation.TreeScope]::Descendants, $menuCondition)) {
                $info = $item.Current; $pattern = $null; $checked = $null
                if ($item.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) { $checked = $pattern.Current.ToggleState.ToString() }
                else { $checked = [NativeVerify]::MenuChecked([IntPtr]$popupHandle, ($info.Name -split "`t")[0]) }
                @{ name=($info.Name -split "`t")[0]; enabled=$info.IsEnabled; offscreen=$info.IsOffscreen; checked=$checked; bounds=(Bounds $info.BoundingRectangle) }
            })
            @{ handle=$popupHandle; bounds=(Bounds $element.Current.BoundingRectangle); items=$items }
        }
    }
    if ($Action -in @('popup','menu-click','dismiss') -or ($Action -eq 'capture' -and $Surface -eq 'popup')) {
        $popups = @(Get-Popups)
        if ($Action -eq 'popup') { ConvertTo-Json -InputObject $popups -Depth 8 -Compress; return }
        $matching = @($popups | Where-Object { !$ItemText -or @($_.items | Where-Object name -CEQ $ItemText).Count })
        if ($matching.Count -ne 1) { throw "Expected one scoped popup matching '$ItemText'; found $($matching.Count)." }
        $popup = $matching[0]
        if ($Action -eq 'dismiss') { [NativeVerify]::DismissPopup([IntPtr]$popup.handle); $result = @{ dismissed=$true } }
        elseif ($Action -eq 'capture') { Save-Rectangle $popup.bounds; $result = $popup }
        else {
            $items = @($popup.items | Where-Object { $_.name -ceq $ItemText -and $_.enabled -and !$_.offscreen })
            if ($items.Count -ne 1) { throw 'Expected one visible enabled exact menu item.' }
            $rect = $items[0].bounds
            if ($rect.width -le 0 -or $rect.height -le 0) { throw 'Menu item has empty bounds.' }
            [NativeVerify]::PopupClick([IntPtr]$popup.handle, [int]($rect.x+$rect.width/2), [int]($rect.y+$rect.height/2))
            $result = $items[0]
        }
        ConvertTo-Json -InputObject $result -Depth 8 -Compress; return
    }
    # Notification icons belong to Explorer, so inspect only its known tray surfaces,
    # never toolbar process memory. Exact unique tooltips identify our created icon.
    if (!$windows.Count) { throw 'No app window remains in the scoped process tree.' }
    $rootElement = [System.Windows.Automation.AutomationElement]::RootElement
    $shellRoots = @($rootElement.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.ClassName -in @('Shell_TrayWnd','Shell_SecondaryTrayWnd','NotifyIconOverflowWindow','TopLevelWindowForOverflowXamlIsland') })
    function Find-TrayIcon {
        foreach ($shell in $shellRoots) {
            $condition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty, $ItemText)
            foreach ($element in $shell.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)) {
                if (!$element.Current.IsOffscreen -and $element.Current.BoundingRectangle.Width -gt 0 -and $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button) { $element }
            }
        }
    }
    $icons = @(Find-TrayIcon)
    if (!$icons.Count -and !@($shellRoots | Where-Object { $_.Current.ClassName -in @('NotifyIconOverflowWindow','TopLevelWindowForOverflowXamlIsland') -and !$_.Current.IsOffscreen }).Count) {
        $overflow = @($shellRoots | ForEach-Object { $_.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition) } | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and !$_.Current.IsOffscreen -and ($_.Current.AutomationId -eq '1502' -or $_.Current.Name -in @('Show hidden icons','Hidden icons menu')) })
        if ($overflow.Count -eq 1) {
            $pattern = $null
            if ($overflow[0].TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
                $pattern.Invoke()
                for ($attempt=0; $attempt -lt 10 -and !$icons.Count; $attempt++) {
                    Start-Sleep -Milliseconds 100
                    $shellRoots = @($rootElement.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.ClassName -in @('Shell_TrayWnd','Shell_SecondaryTrayWnd','NotifyIconOverflowWindow','TopLevelWindowForOverflowXamlIsland') })
                    $icons = @(Find-TrayIcon)
                }
            }
        }
    }
    $result = @{ available=($icons.Count -eq 1); name=$ItemText; reason=''; bounds=$null }
    if ($icons.Count -ne 1) { $result.reason = "Exact notification icon not uniquely exposed by shell UIAutomation (matches=$($icons.Count), tray surfaces=$($shellRoots.Count))." }
    else {
        $info = $icons[0].Current; $rect = Bounds $info.BoundingRectangle; $result.bounds = $rect
        if ($Action -eq 'tray-click') {
            $point = [System.Windows.Point]::new($rect.x+$rect.width/2, $rect.y+$rect.height/2)
            $hit = [System.Windows.Automation.AutomationElement]::FromPoint($point)
            if ($hit.Current.Name -cne $ItemText) { throw 'Exact tray input point is obscured.' }
            [NativeVerify]::ClickAt([int]$point.X, [int]$point.Y, $Button -eq 'right')
        } elseif ($Action -eq 'capture') { Save-Rectangle $rect }
    }
    if ($Action -eq 'capture' -and !$result.available) { throw $result.reason }
    ConvertTo-Json -InputObject $result -Depth 6 -Compress; return
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
    'capture' { Save-Rectangle $before; $before }
}
ConvertTo-Json -InputObject $result -Depth 4 -Compress
