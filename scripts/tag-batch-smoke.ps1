# Tag filter and batch tag edit smoke test.
# Uses isolated portable data, seeds three tagged snippets, then drives the
# management window: filter by tag, Ctrl-select two rows, add/remove a tag.
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts\tag-batch-smoke.ps1 [-SkipBuild]

param([switch]$SkipBuild)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "smoke-common.ps1")
Assert-QPTestIdle
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root "target\release\quickphrase.exe"

if (-not $SkipBuild) {
    Push-Location $root
    try {
        & cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    } finally { Pop-Location }
}
if (-not (Test-Path $exe)) { throw "missing $exe" }

$qpRun = New-QPTestRun $exe
try {
# First launch seeds data/settings, then the test replaces the tags.
$dataPath = Join-Path $qpRun.Data 'data.json'
$seed = Start-QPTest -Arguments '--bench' -Wait
if ($seed.ExitCode -ne 0) { throw "Could not seed the test library: $($seed.ExitCode)" }
$data = [IO.File]::ReadAllText($dataPath, [Text.Encoding]::UTF8) | ConvertFrom-Json
$data.snippets[0].tags = @("support", "refund")
$data.snippets[1].tags = @("support")
$data.snippets[2].tags = @("dev")
[IO.File]::WriteAllText($dataPath, ($data | ConvertTo-Json -Depth 8), (New-Object Text.UTF8Encoding($false)))

Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class QPTagBatchSmoke {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", EntryPoint="SendMessageW", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageS(IntPtr h, uint m, IntPtr w, string l);
  [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr SendMessageRaw(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", EntryPoint="PostMessageW")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr SetFocus(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetComboBoxInfo(IntPtr h, ref COMBOBOXINFO info);
  public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)]
  public struct COMBOBOXINFO {
    public int cbSize;
    public RECT rcItem;
    public RECT rcButton;
    public int stateButton;
    public IntPtr hwndCombo;
    public IntPtr hwndItem;
    public IntPtr hwndList;
  }
}
"@

$results = New-Object System.Collections.Generic.List[object]
function Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}
function Find-Window([string]$class) {
    $script:found = [IntPtr]::Zero
    $cb = [QPTagBatchSmoke+EnumProc]{
        param($h, $unused)
        if (-not (Test-QPTestWindow $h)) { return $true }
        $sb = New-Object System.Text.StringBuilder 256
        [void][QPTagBatchSmoke]::GetClassName($h, $sb, 256)
        if ($sb.ToString() -eq $class) { $script:found = $h; return $false }
        return $true
    }
    [void][QPTagBatchSmoke]::EnumWindows($cb, [IntPtr]::Zero)
    return $script:found
}
function Read-Data {
    return [IO.File]::ReadAllText($dataPath, [Text.Encoding]::UTF8) | ConvertFrom-Json
}

Start-QPTest -Arguments "--manage-test" | Out-Null
Start-Sleep -Milliseconds 1500
$manage = Find-Window "QuickPhrase.ManageWindow"
if ($manage -eq [IntPtr]::Zero) { throw "manage window missing" }
$list = [QPTagBatchSmoke]::GetDlgItem($manage, 202)
$combo = [QPTagBatchSmoke]::GetDlgItem($manage, 217)
$tags = [QPTagBatchSmoke]::GetDlgItem($manage, 211)

$comboCount = [int64][QPTagBatchSmoke]::SendMessageRaw($combo, 0x0146, [IntPtr]::Zero, [IntPtr]::Zero)
Check "tag options include all tags" ($comboCount -eq 4) ("combo=" + $comboCount)
[void][QPTagBatchSmoke]::SetFocus($combo)
[void][QPTagBatchSmoke]::SendMessageRaw($combo, 0x014F, [IntPtr]1, [IntPtr]::Zero)
Start-Sleep -Milliseconds 300
$info = New-Object QPTagBatchSmoke+COMBOBOXINFO
$info.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($info)
[void][QPTagBatchSmoke]::GetComboBoxInfo($combo, [ref]$info)
$listRect = New-Object QPTagBatchSmoke+RECT
if ($info.hwndList -ne [IntPtr]::Zero) { [void][QPTagBatchSmoke]::GetWindowRect($info.hwndList, [ref]$listRect) }
$listHeight = $listRect.Bottom - $listRect.Top
Check "tag combo dropdown opens" (($info.hwndList -ne [IntPtr]::Zero) -and ($listHeight -ge 20)) ("listHeight=" + $listHeight)
[void][QPTagBatchSmoke]::SendMessageRaw($combo, 0x014F, [IntPtr]0, [IntPtr]::Zero)

[void][QPTagBatchSmoke]::SendMessageRaw($combo, 0x014E, [IntPtr]3, [IntPtr]::Zero)
[void][QPTagBatchSmoke]::PostMessage($manage, 0x0111, [IntPtr](217 -bor (1 -shl 16)), [IntPtr]::Zero)
Start-Sleep -Milliseconds 400
$supported = [int64][QPTagBatchSmoke]::SendMessageRaw($list, 0x018B, [IntPtr]::Zero, [IntPtr]::Zero)
Check "tag filter support" ($supported -eq 2) ("list=" + $supported)

[void][QPTagBatchSmoke]::PostMessage($manage, 0x0111, [IntPtr]218, [IntPtr]::Zero)
Start-Sleep -Milliseconds 400
$selectedFiltered = [int64][QPTagBatchSmoke]::SendMessageRaw($list, 0x0190, [IntPtr]::Zero, [IntPtr]::Zero)
Check "select all respects filter" ($selectedFiltered -eq 2) ("selected=" + $selectedFiltered)

[void][QPTagBatchSmoke]::SendMessageRaw($combo, 0x014E, [IntPtr]0, [IntPtr]::Zero)
[void][QPTagBatchSmoke]::PostMessage($manage, 0x0111, [IntPtr](217 -bor (1 -shl 16)), [IntPtr]::Zero)
Start-Sleep -Milliseconds 400
$all = [int64][QPTagBatchSmoke]::SendMessageRaw($list, 0x018B, [IntPtr]::Zero, [IntPtr]::Zero)
Check "tag filter cleared" ($all -eq 3) ("list=" + $all)

[void][QPTagBatchSmoke]::SendMessageRaw($list, 0x0185, [IntPtr]1, [IntPtr]0)
[void][QPTagBatchSmoke]::SendMessageRaw($list, 0x0185, [IntPtr]1, [IntPtr]1)
[void][QPTagBatchSmoke]::PostMessage($manage, 0x0111, [IntPtr](202 -bor (1 -shl 16)), [IntPtr]::Zero)
Start-Sleep -Milliseconds 500

[void][QPTagBatchSmoke]::SendMessageS($tags, 0x000C, [IntPtr]::Zero, "vip")
[void][QPTagBatchSmoke]::SendMessageRaw($manage, 0x0111, [IntPtr]209, [IntPtr]::Zero)
Start-Sleep -Milliseconds 700
$afterAdd = Read-Data
Check "batch add applied to selected" (
    (($afterAdd.snippets[0].tags -contains "vip") -and ($afterAdd.snippets[1].tags -contains "vip"))
) "tags=" + (($afterAdd.snippets | ForEach-Object { $_.tags -join "," }) -join " | ")
Check "batch add skipped unselected" (-not ($afterAdd.snippets[2].tags -contains "vip")) ""

[void][QPTagBatchSmoke]::SendMessageRaw($list, 0x0185, [IntPtr]1, [IntPtr]0)
[void][QPTagBatchSmoke]::SendMessageRaw($list, 0x0185, [IntPtr]1, [IntPtr]1)
[void][QPTagBatchSmoke]::PostMessage($manage, 0x0111, [IntPtr](202 -bor (1 -shl 16)), [IntPtr]::Zero)
Start-Sleep -Milliseconds 500
[void][QPTagBatchSmoke]::SendMessageS($tags, 0x000C, [IntPtr]::Zero, "vip")
[void][QPTagBatchSmoke]::SendMessageRaw($manage, 0x0111, [IntPtr]210, [IntPtr]::Zero)
Start-Sleep -Milliseconds 700
$afterRemove = Read-Data
Check "batch remove applied" (
    (-not ($afterRemove.snippets[0].tags -contains "vip")) -and (-not ($afterRemove.snippets[1].tags -contains "vip"))
) ""
Check "original tags preserved" (
    (($afterRemove.snippets[0].tags -contains "support") -and ($afterRemove.snippets[0].tags -contains "refund"))
) ("tags=" + ($afterRemove.snippets[0].tags -join ","))

[void][QPTagBatchSmoke]::PostMessage($manage, 0x0111, [IntPtr]218, [IntPtr]::Zero)
Start-Sleep -Milliseconds 400
$selectedAll = [int64][QPTagBatchSmoke]::SendMessageRaw($list, 0x0190, [IntPtr]::Zero, [IntPtr]::Zero)
Check "select all filtered rows" ($selectedAll -eq 3) ("selected=" + $selectedAll)

$failed = @($results | Where-Object { -not $_.Ok })
$results | Format-Table -AutoSize
if ($failed.Count -gt 0) { "FAIL: $($failed.Count) check(s)"; exit 1 }
"PASS: $($results.Count) checks"
exit 0
} finally {
    Stop-QPTestRun
}
